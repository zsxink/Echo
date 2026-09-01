//! IPC error DTO and mapping (task 7.2, design §10).
//!
//! Every command returns `Result<T, IpcErrorDto>`. The DTO carries a stable
//! `code`, a message key safe for the UI, a `retryable` flag, an optional
//! `operationId` and a minimal structured detail — never a Rust debug string
//! and never an absolute path.

use serde::Serialize;

use echo_core::error::Error as CoreError;

/// The wire shape of an IPC error. Deliberately small and stable so the
/// frontend can branch on `code` and surface `messageKey` for localization.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IpcErrorDto {
    /// Stable error code (matches [`CoreError::code`]).
    pub code: &'static str,
    /// A localization key, never raw text. The UI owns translation.
    pub message_key: &'static str,
    /// Whether retrying the same command is safe/expected.
    pub retryable: bool,
    /// A journal operation, when the error is tied to one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<String>,
    /// The validation field, for `validation` errors.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
}

impl IpcErrorDto {
    #[must_use]
    pub const fn new(
        code: &'static str,
        message_key: &'static str,
        retryable: bool,
        operation_id: Option<String>,
        field: Option<String>,
    ) -> Self {
        Self {
            code,
            message_key,
            retryable,
            operation_id,
            field,
        }
    }
}

/// Map a core error to the IPC shape, deriving `retryable` and a UI-safe
/// message key. Absolute paths / debug strings never cross this boundary.
impl From<&CoreError> for IpcErrorDto {
    fn from(error: &CoreError) -> Self {
        let code = error.code();
        let message_key = match code {
            "validation" => "error.validation",
            "permission" => "error.permission",
            "unavailable" => "error.unavailable",
            "conflict" => "error.conflict",
            "unsupported_media" => "error.unsupportedMedia",
            "corrupt_media" => "error.corruptMedia",
            "io" => "error.io",
            "storage" => "error.storage",
            "cancelled" => "error.cancelled",
            "invariant_violation" => "error.internal",
            _ => "error.unknown",
        };
        let retryable = ErrorPolicy::retryable(code);
        let field = match error {
            CoreError::Validation { field, .. } => Some(field.clone()),
            _ => None,
        };
        Self::new(code, message_key, retryable, None, field)
    }
}

impl From<CoreError> for IpcErrorDto {
    fn from(error: CoreError) -> Self {
        Self::from(&error)
    }
}

/// Which core error classes indicate a *user-safe retry* (design §17, task 7.8).
///
/// The single source of truth for `IpcErrorDto.retryable`. Keeping it as one
/// explicit table means a newly introduced core error code cannot silently
/// change the frontend's retry behaviour — the mapping is pinned by the test
/// below.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ErrorPolicy;

impl ErrorPolicy {
    /// Whether an automatic retry of the same command is safe/expected for an
    /// error code.
    ///
    /// - `unavailable`, `io`, `storage` → **true**: transient resource states
    ///   and recoverable I/O/storage failures are worth re-attempting.
    /// - `validation`, `permission`, `conflict`, `unsupported_media`,
    ///   `corrupt_media`, `cancelled`, `invariant_violation` → **false**: the
    ///   input or state must change first, or the operation was deliberately
    ///   aborted, so a blind retry cannot help.
    #[must_use]
    pub fn retryable(code: &str) -> bool {
        matches!(code, "unavailable" | "io" | "storage")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::error::Error as CoreError;

    #[test]
    fn unavailable_is_retryable_validation_is_not_and_codes_stay_stable() {
        let unavailable = CoreError::Unavailable {
            resource: "library".into(),
            hint: "根目录断开".into(),
            source: None,
        };
        let dto = IpcErrorDto::from(&unavailable);
        assert_eq!(dto.code, "unavailable");
        assert!(dto.retryable);
        assert_eq!(dto.message_key, "error.unavailable");
        assert_eq!(dto.operation_id, None, "no operation tied to it");

        let validation = CoreError::validation(
            echo_core::error::Subject::Name,
            "name",
            "名称不能为空".to_owned(),
        );
        let dto = IpcErrorDto::from(&validation);
        assert_eq!(dto.code, "validation");
        assert!(!dto.retryable);
        assert_eq!(dto.field.as_deref(), Some("name"));

        let conflict = CoreError::conflict("duplicate");
        assert_eq!(IpcErrorDto::from(conflict).code, "conflict");
    }

    #[test]
    fn error_policy_marks_only_transient_classes_retryable() {
        // The retryable decision is the single policy table; a newly added
        // core code must be added here explicitly or it is not retryable.
        assert!(ErrorPolicy::retryable("unavailable"));
        assert!(ErrorPolicy::retryable("io"));
        assert!(ErrorPolicy::retryable("storage"));
        assert!(!ErrorPolicy::retryable("validation"));
        assert!(!ErrorPolicy::retryable("permission"));
        assert!(!ErrorPolicy::retryable("conflict"));
        assert!(!ErrorPolicy::retryable("unsupported_media"));
        assert!(!ErrorPolicy::retryable("corrupt_media"));
        assert!(!ErrorPolicy::retryable("cancelled"));
        assert!(!ErrorPolicy::retryable("invariant_violation"));
        assert!(!ErrorPolicy::retryable("not_a_real_code"));
    }

    #[test]
    fn serialization_is_camel_case_and_path_free() {
        let dto = IpcErrorDto::from(&CoreError::Unavailable {
            resource: "library".into(),
            hint: "根目录断开".into(),
            source: None,
        });
        let json = serde_json::to_string(&dto).expect("serialize");
        assert_eq!(
            json, r#"{"code":"unavailable","messageKey":"error.unavailable","retryable":true}"#,
            "camelCase DTO, absolute paths omitted"
        );
    }
}
