//! Cover payload decoding at the Tauri command boundary.
//!
//! The `WebView` sends cover bytes as a base64 string (design D1): a 5 MiB image
//! is a 6 990 508-character string instead of a 5 242 880-element JSON array
//! plus its serialized text. Base64 is the *wire* format, so it is decoded here
//! — in the shell that owns the wire format — and never reaches `echo-desktop`
//! or `echo-core`, which keep their byte-oriented `Option<&[u8]>` signatures
//! (design D2).
//!
//! `None` keeps its existing meaning: clear the cover (design D5). The argument
//! is deliberately still named `bytes` so a legacy client's `number[]` payload
//! fails argument deserialization instead of turning into a missing key that
//! would silently clear the saved cover.

use base64::Engine;
use echo_core::error::{Error as CoreError, ValidationSubject};
use echo_desktop::ipc::IpcErrorDto;

/// The cover size limit enforced by `echo_core::application::artwork::validate`.
const MAX_COVER_BYTES: usize = 5 * 1024 * 1024;

/// The longest base64 text that can still decode within [`MAX_COVER_BYTES`]:
/// `4 × ⌈5 242 880 / 3⌉` (design D6). A longer payload necessarily decodes past
/// the limit, so it is rejected before a decode buffer is allocated.
#[allow(clippy::redundant_pub_crate)] // the module is private to the binary shell
pub(crate) const MAX_ENCODED_LEN: usize = MAX_COVER_BYTES.div_ceil(3) * 4;

/// Decode the base64 cover payload received from the `WebView`.
///
/// `Ok(None)` means "clear the cover" (absent key or explicit `null`). An empty
/// string, an over-long payload, or text that is not valid base64 is reported
/// as a validation error on the `cover` field — the same error path the
/// frontend already renders, so no new error kind is introduced.
///
/// # Errors
/// Returns a validation `IpcErrorDto` when the payload cannot be accepted.
#[allow(clippy::redundant_pub_crate)] // the module is private to the binary shell
pub(crate) fn decode_cover_bytes(payload: Option<&str>) -> Result<Option<Vec<u8>>, IpcErrorDto> {
    let Some(payload) = payload else {
        return Ok(None);
    };
    validate_encoded_len(payload.len()).map_err(|error| IpcErrorDto::from_mutation(&error))?;
    if payload.is_empty() {
        return Err(validation("封面数据为空"));
    }
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(payload)
        .map_err(|_| validation("封面数据无法解码"))?;
    Ok(Some(decoded))
}

/// Reject encoded text that is longer than any in-limit image can produce.
///
/// Kept separate from decoding so the boundary itself is testable without
/// allocating a multi-megabyte decode buffer (design D6).
fn validate_encoded_len(len: usize) -> Result<(), CoreError> {
    if len > MAX_ENCODED_LEN {
        return Err(CoreError::validation(
            ValidationSubject::Other,
            "cover",
            "encoded cover payload exceeds the 5 MiB limit",
        ));
    }
    Ok(())
}

/// Build the validation error shape the command layer already maps to IPC.
fn validation(reason: &str) -> IpcErrorDto {
    let error = CoreError::validation(ValidationSubject::Other, "cover", reason);
    IpcErrorDto::from_mutation(&error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_desktop::runtime::services::AppServices;
    use tauri::State;

    /// The command shape both cover commands must have: the payload is a
    /// base64 string, and the argument is still named `bytes`.
    type CoverCommand = for<'a> fn(
        State<'a, AppServices>,
        String,
        Option<String>,
        Option<String>,
    ) -> Result<(), IpcErrorDto>;

    fn encode(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    #[test]
    fn decodes_valid_base64_to_original_bytes() {
        let original: Vec<u8> = (0..=255u8).cycle().take(4096).collect();
        let decoded = decode_cover_bytes(Some(&encode(&original))).expect("valid payload");
        assert_eq!(decoded.as_deref(), Some(original.as_slice()));
    }

    #[test]
    fn rejects_malformed_base64_as_a_validation_error() {
        let error = decode_cover_bytes(Some("not*base64!")).expect_err("malformed payload");
        assert_eq!(error.code, "validation");
        assert_eq!(error.message_key, "error.validation");
        assert_eq!(error.field.as_deref(), Some("cover"));
    }

    #[test]
    fn rejects_payload_longer_than_the_encoded_limit_before_decoding() {
        // The boundary itself is decodable: 5 MiB encodes to exactly
        // 6 990 508 characters (2 bytes in the final group, so `==` padding).
        let boundary_bytes = vec![0xAB_u8; MAX_COVER_BYTES];
        let boundary = encode(&boundary_bytes);
        assert_eq!(boundary.len(), MAX_ENCODED_LEN);
        let decoded = decode_cover_bytes(Some(&boundary)).expect("boundary payload");
        assert_eq!(decoded.as_deref(), Some(boundary_bytes.as_slice()));

        // One character past the boundary is refused by the length precheck.
        let over_long = format!("{boundary}A");
        assert_eq!(over_long.len(), MAX_ENCODED_LEN + 1);
        assert!(validate_encoded_len(over_long.len()).is_err());
        let error = decode_cover_bytes(Some(&over_long)).expect_err("over-long payload");
        assert_eq!(error.code, "validation");
        assert_eq!(error.field.as_deref(), Some("cover"));
    }

    #[test]
    fn treats_an_absent_or_null_payload_as_clearing_the_cover() {
        // `null` and a missing key both deserialize to `None` in Tauri's
        // `Option<T>` visitor; clearing is the existing, unchanged semantic.
        assert_eq!(decode_cover_bytes(None).expect("absent payload"), None);
    }

    #[test]
    fn rejects_an_empty_payload_string() {
        let error = decode_cover_bytes(Some("")).expect_err("empty payload");
        assert_eq!(error.code, "validation");
        assert_eq!(error.field.as_deref(), Some("cover"));
    }

    #[test]
    fn both_cover_commands_decode_through_the_same_helper() {
        // The type assertion proves both commands receive the payload as
        // `Option<String>`; decoding the same text through the one helper
        // proves they share the path rather than merely the signature shape.
        let _: CoverCommand = crate::commands::set_playlist_cover;
        let _: CoverCommand = crate::commands::set_artist_cover;

        let payload = encode(b"synthetic cover bytes");
        let from_playlist_path = decode_cover_bytes(Some(&payload)).expect("playlist payload");
        let from_artist_path = decode_cover_bytes(Some(&payload)).expect("artist payload");
        assert_eq!(from_playlist_path, from_artist_path);
        assert_eq!(
            from_playlist_path.as_deref(),
            Some(b"synthetic cover bytes".as_slice())
        );
    }
}
