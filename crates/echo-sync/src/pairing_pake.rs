//! SPAKE2 (RFC 9382) over P-256 for the LAN pairing bootstrap.
//!
//! The six-digit pairing code never travels on the wire. Both sides derive a
//! password scalar from it and run one SPAKE2 exchange; the device pins the
//! host certificate only after the host has proved possession of the code
//! through the key confirmation. A relay that does not know the code cannot
//! complete the confirmation, so it can neither get its own certificate
//! pinned nor learn anything an offline dictionary attack could grind against.
//!
//! The password scalar commits to `code` and the requesting `device_id`, so a
//! captured first message cannot be replayed by another device. The host then
//! proves its certificate under the derived session key ([`certificate_proof`]),
//! which is what makes the pinned key the one this handshake authenticated: a
//! relay can forward the SPAKE2 messages, but it cannot produce a proof for its
//! own certificate without the code.
//!
//! The invitation identity cannot enter the scalar: the device does not know
//! `invitation_id` or `binding` before it sends its first message. The host
//! releases both inside the PAKE-authenticated response instead, where a relay
//! cannot forge them without completing the confirmation.
//!
//! The device is SPAKE2 `PartyA` and speaks first; the host is `PartyB`. The
//! host releases nothing (no grant, no credentials) until the device's
//! confirmation MAC verifies, which proves the device holds the same code.

use std::fmt;

use echo_core::domain::sync::DeviceId;
use pakery_core::crypto::CpaceGroup;
use pakery_core::SharedSecret;
use pakery_crypto::{P256Group, Spake2P256};
use pakery_spake2::{PartyA, PartyB, Spake2Error, Spake2Output};
use rand_core::{Infallible, TryCryptoRng, TryRng, UnwrapErr};
use subtle::ConstantTimeEq;

/// Length of a serialized SPAKE2 message: one SEC1 uncompressed P-256 point.
pub const PAKE_MESSAGE_BYTES: usize = 65;

/// Length of a key-confirmation MAC (`Spake2P256` uses HMAC-SHA256).
pub const PAKE_CONFIRMATION_BYTES: usize = 32;

/// Length of the derived session key (`Ke`, half of `NH`).
pub const PAKE_SESSION_KEY_BYTES: usize = 16;

/// Length of a certificate proof: one `blake3` keyed-hash output.
pub const PAKE_CERTIFICATE_PROOF_BYTES: usize = 32;

/// Domain separation for the password scalar and the associated data.
///
/// The version suffix keeps a pairing bootstrap from ever interleaving with a
/// differently versioned protocol that happens to share these primitives.
const DOMAIN: &[u8] = b"echo-lan-pairing-v2";

/// Context string for the `blake3` key derivation that keys the proof.
const PROOF_CONTEXT: &str = "echo-lan-pairing-v2 certificate proof";

/// SPAKE2 identity of the host side of the exchange.
///
/// The host's real device id is not known to the device before the first
/// message; the certificate proof carries the host's identity instead, so this
/// label only separates the two roles.
const HOST_IDENTITY: &[u8] = b"echo-lan-pairing-host";

/// Failure of a SPAKE2 pairing handshake.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PakeError {
    /// The OS random source failed its pre-flight check.
    EntropyUnavailable,
    /// A peer message was malformed: wrong length, or not a valid point.
    InvalidMessage,
    /// The peer did not prove possession of the pairing code.
    ConfirmationFailed,
    /// The host did not prove the certificate it asks the device to pin.
    CertificateUnverified,
    /// The cryptographic library reported an internal failure.
    Internal,
}

impl fmt::Display for PakeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::EntropyUnavailable => "the OS random source is unavailable",
            Self::InvalidMessage => "a pairing handshake message was malformed",
            Self::ConfirmationFailed => "the peer did not prove possession of the pairing code",
            Self::CertificateUnverified => "the host did not prove the certificate to pin",
            Self::Internal => "the pairing handshake failed internally",
        };
        f.write_str(message)
    }
}

impl std::error::Error for PakeError {}

/// Everything both sides must agree on before the exchange starts.
pub struct PakeContext<'a> {
    /// The six-digit code the host displayed and the user typed.
    pub code: &'a str,
    /// The device requesting the pairing.
    pub device_id: DeviceId,
}

impl fmt::Debug for PakeContext<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // The code is a secret; print the rest so failures stay diagnosable
        // without leaking it into a log.
        f.debug_struct("PakeContext")
            .field("device_id", &self.device_id)
            .finish_non_exhaustive()
    }
}

/// The SPAKE2 session key.
///
/// Wraps the library's zeroizing secret so no third-party type appears in
/// this crate's public API. It authenticates the certificate proof; it is not
/// used for traffic protection.
pub struct SessionKey(SharedSecret);

impl SessionKey {
    /// The raw key bytes, [`PAKE_SESSION_KEY_BYTES`] long.
    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

impl fmt::Debug for SessionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SessionKey(..)")
    }
}

type PartyAState = pakery_spake2::PartyAState<Spake2P256>;

/// Device-side (SPAKE2 `PartyA`) state between the two messages.
pub struct DevicePake {
    state: PartyAState,
}

/// What the device sends back once the host has proved possession of the code.
pub struct DeviceConfirmation {
    /// The device's confirmation MAC, for the host to verify.
    pub confirmation: Vec<u8>,
    /// The session key derived in this exchange.
    pub session_key: SessionKey,
}

impl DevicePake {
    /// Derive the password scalar and produce the first message.
    ///
    /// # Errors
    ///
    /// Returns [`PakeError::EntropyUnavailable`] when the OS random source is
    /// unusable, and [`PakeError::Internal`] when the derivation cannot be
    /// reduced into the group (a bug in the inputs, not in the peer).
    pub fn start(context: &PakeContext<'_>) -> Result<(Vec<u8>, Self), PakeError> {
        preflight_entropy()?;
        let scalar = password_scalar(context)?;
        let aad = associated_data(context);
        let mut rng = PakeRng::new();
        let (message, state) = PartyA::<Spake2P256>::start(
            &scalar,
            context.device_id.as_uuid().as_bytes(),
            HOST_IDENTITY,
            &aad,
            &mut rng,
        )
        .map_err(map_spake_error)?;
        Ok((message, Self { state }))
    }

    /// Process the host's message and require it to prove possession of the code.
    ///
    /// Callers must not pin the host certificate or persist any credential
    /// until this returns `Ok` and the certificate proof verifies.
    ///
    /// # Errors
    ///
    /// Returns [`PakeError::InvalidMessage`] for a malformed or non-canonical
    /// peer message, and [`PakeError::ConfirmationFailed`] when the host's
    /// confirmation MAC does not match — the signature of a relay, a wrong
    /// code, or a substituted certificate.
    pub fn finish(
        self,
        host_message: &[u8],
        host_confirmation: &[u8],
    ) -> Result<DeviceConfirmation, PakeError> {
        check_length(host_message, PAKE_MESSAGE_BYTES)?;
        check_length(host_confirmation, PAKE_CONFIRMATION_BYTES)?;
        let output = self.state.finish(host_message).map_err(map_spake_error)?;
        output
            .verify_peer_confirmation(host_confirmation)
            .map_err(map_spake_error)?;
        let confirmation = output.confirmation_mac.clone();
        Ok(DeviceConfirmation {
            confirmation,
            session_key: SessionKey(output.into_session_key()),
        })
    }
}

/// Host-side (SPAKE2 `PartyB`) state after processing the device's message.
pub struct HostPake {
    output: Spake2Output,
}

/// The host's reply to the device's first message.
pub struct HostResponse {
    /// The host's SPAKE2 message.
    pub message: Vec<u8>,
    /// The host's confirmation MAC, sent alongside [`Self::message`].
    pub confirmation: Vec<u8>,
    /// The state needed to verify the device's confirmation MAC.
    pub state: HostPake,
}

impl HostPake {
    /// Derive the password scalar and respond to the device's first message.
    ///
    /// # Errors
    ///
    /// Returns [`PakeError::EntropyUnavailable`] when the OS random source is
    /// unusable, [`PakeError::InvalidMessage`] when `device_message` is not a
    /// canonical point, and [`PakeError::Internal`] when the derivation
    /// cannot be reduced into the group.
    pub fn respond(
        context: &PakeContext<'_>,
        device_message: &[u8],
    ) -> Result<HostResponse, PakeError> {
        preflight_entropy()?;
        check_length(device_message, PAKE_MESSAGE_BYTES)?;
        let scalar = password_scalar(context)?;
        let aad = associated_data(context);
        let mut rng = PakeRng::new();
        let (message, state) = PartyB::<Spake2P256>::start(
            &scalar,
            context.device_id.as_uuid().as_bytes(),
            HOST_IDENTITY,
            &aad,
            &mut rng,
        )
        .map_err(map_spake_error)?;
        let output = state.finish(device_message).map_err(map_spake_error)?;
        let confirmation = output.confirmation_mac.clone();
        Ok(HostResponse {
            message,
            confirmation,
            state: Self { output },
        })
    }

    /// Require the device to prove it holds the same code.
    ///
    /// Nothing may be provisioned — no grant, no bearer token — before this
    /// returns `Ok`.
    ///
    /// # Errors
    ///
    /// Returns [`PakeError::InvalidMessage`] when `confirmation` has the wrong
    /// length, and [`PakeError::ConfirmationFailed`] when the device does not
    /// hold the code this exchange was started for.
    pub fn verify_device_confirmation(&self, confirmation: &[u8]) -> Result<(), PakeError> {
        check_length(confirmation, PAKE_CONFIRMATION_BYTES)?;
        self.output
            .verify_peer_confirmation(confirmation)
            .map_err(map_spake_error)
    }

    /// Prove that this handshake's peer chose `certificate_der` as its pin.
    ///
    /// The proof is keyed by the session key, so only a party that completed
    /// the exchange can produce it for any certificate.
    #[must_use]
    pub fn certificate_proof(&self, certificate_der: &[u8]) -> [u8; PAKE_CERTIFICATE_PROOF_BYTES] {
        proof_bytes(self.output.session_key.as_bytes(), certificate_der)
    }

    /// The session key derived in this exchange.
    #[must_use]
    pub fn session_key(&self) -> &[u8] {
        self.output.session_key.as_bytes()
    }
}

/// Verify the host's certificate proof under the device's session key.
///
/// The device must call this before pinning `certificate_der`. A relay that
/// forwards the SPAKE2 messages still cannot prove its own certificate.
///
/// # Errors
///
/// Returns [`PakeError::InvalidMessage`] for a proof of the wrong length and
/// [`PakeError::CertificateUnverified`] when it does not match.
pub fn verify_certificate_proof(
    session_key: &SessionKey,
    certificate_der: &[u8],
    proof: &[u8],
) -> Result<(), PakeError> {
    check_length(proof, PAKE_CERTIFICATE_PROOF_BYTES)?;
    // The comparison itself stays constant-time; only the accept/reject
    // outcome is declassified, and it is public either way.
    if bool::from(proof_bytes(session_key.as_bytes(), certificate_der).ct_eq(proof)) {
        Ok(())
    } else {
        Err(PakeError::CertificateUnverified)
    }
}

fn proof_bytes(session_key: &[u8], certificate_der: &[u8]) -> [u8; PAKE_CERTIFICATE_PROOF_BYTES] {
    let key = blake3::derive_key(PROOF_CONTEXT, session_key);
    *blake3::keyed_hash(&key, certificate_der).as_bytes()
}

/// Derive the password scalar from the code and the requesting device.
///
/// `blake3` in extendable-output mode supplies the 64 wide bytes
/// `P256Group::scalar_from_wide_bytes` requires; the reduction is the RFC 9382
/// `w = Hash(password) mod p` step with the device identity mixed in.
fn password_scalar(context: &PakeContext<'_>) -> Result<P256Scalar, PakeError> {
    let mut hasher = blake3::Hasher::new();
    hasher.update(DOMAIN);
    hasher.update(b"password");
    hasher.update(context.code.as_bytes());
    hasher.update(context.device_id.as_uuid().as_bytes());
    let mut wide = [0u8; 64];
    hasher.finalize_xof().fill(&mut wide);
    let scalar = P256Group::scalar_from_wide_bytes(&wide).map_err(|_| PakeError::Internal);
    // Best-effort scrub of the stack copy; the scalar keeps its own zeroizing
    // copy inside the party state.
    wide.fill(0);
    scalar
}

type P256Scalar = <P256Group as CpaceGroup>::Scalar;

/// Build the associated data the key confirmation commits to.
///
/// Both sides must produce byte-identical output or the confirmation MACs
/// disagree. The device id is the only context both sides hold before the
/// first message; the certificate is bound afterwards, by
/// [`HostPake::certificate_proof`].
fn associated_data(context: &PakeContext<'_>) -> Vec<u8> {
    let mut aad = Vec::with_capacity(48);
    aad.extend_from_slice(DOMAIN);
    aad.extend_from_slice(b"aad");
    aad.extend_from_slice(context.device_id.as_uuid().as_bytes());
    aad
}

/// Prove the OS entropy source works before the exchange commits to it.
///
/// [`PakeRng`] turns later failures into panics because the traits it adapts
/// to have an infallible error type; this check is what keeps that promise
/// from being a guess.
fn preflight_entropy() -> Result<(), PakeError> {
    let mut probe = [0u8; 32];
    getrandom::fill(&mut probe).map_err(|_| PakeError::EntropyUnavailable)
}

const fn check_length(bytes: &[u8], expected: usize) -> Result<(), PakeError> {
    if bytes.len() == expected {
        Ok(())
    } else {
        Err(PakeError::InvalidMessage)
    }
}

// `map_err` hands the error over by value; taking a reference would only add
// a closure at every call site without saving a byte.
#[allow(clippy::needless_pass_by_value)]
const fn map_spake_error(error: Spake2Error) -> PakeError {
    match error {
        Spake2Error::ConfirmationFailed => PakeError::ConfirmationFailed,
        Spake2Error::InvalidPoint | Spake2Error::IdentityPoint => PakeError::InvalidMessage,
        Spake2Error::InternalError(_) => PakeError::Internal,
    }
}

/// `UnwrapErr(SysRng)` adapted to `rand_core` 0.10's infallible traits.
///
/// The blanket impls give this newtype `Rng` and `CryptoRng`, which is what
/// the SPAKE2 state machines take. Its `Infallible` error type records a fact
/// established by [`preflight_entropy`] moments earlier, rather than
/// swallowing a failure that can still happen.
struct PakeRng(UnwrapErr<getrandom::SysRng>);

impl PakeRng {
    const fn new() -> Self {
        Self(UnwrapErr(getrandom::SysRng))
    }
}

impl TryRng for PakeRng {
    type Error = Infallible;

    fn try_next_u32(&mut self) -> Result<u32, Self::Error> {
        self.0.try_next_u32()
    }

    fn try_next_u64(&mut self) -> Result<u64, Self::Error> {
        self.0.try_next_u64()
    }

    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), Self::Error> {
        self.0.try_fill_bytes(dest)
    }
}

impl TryCryptoRng for PakeRng {}

#[cfg(test)]
mod tests {
    use super::*;

    fn context(code: &str, device_id: DeviceId) -> PakeContext<'_> {
        PakeContext { code, device_id }
    }

    #[test]
    fn matching_code_completes_and_both_sides_agree() {
        let device = DeviceId::new();
        let context = context("123456", device);

        let (device_message, device_state) = DevicePake::start(&context).expect("device start");
        assert_eq!(device_message.len(), PAKE_MESSAGE_BYTES);

        let response = HostPake::respond(&context, &device_message).expect("host respond");
        assert_eq!(response.message.len(), PAKE_MESSAGE_BYTES);
        assert_eq!(response.confirmation.len(), PAKE_CONFIRMATION_BYTES);

        let finished = device_state
            .finish(&response.message, &response.confirmation)
            .expect("the host proved possession of the code");
        response
            .state
            .verify_device_confirmation(&finished.confirmation)
            .expect("the device proved possession of the code");
        assert_eq!(
            finished.session_key.as_bytes(),
            response.state.session_key(),
            "both sides derive the same session key"
        );
        assert_eq!(
            finished.session_key.as_bytes().len(),
            PAKE_SESSION_KEY_BYTES
        );

        let certificate = b"host certificate der";
        let proof = response.state.certificate_proof(certificate);
        verify_certificate_proof(&finished.session_key, certificate, &proof)
            .expect("the host proved the certificate under the session key");
    }

    #[test]
    fn wrong_code_fails_the_confirmation() {
        let device = DeviceId::new();
        let device_context = context("123456", device);
        let host_context = context("654321", device);

        let (device_message, device_state) =
            DevicePake::start(&device_context).expect("device start");
        let response = HostPake::respond(&host_context, &device_message).expect("host respond");
        assert_eq!(
            device_state
                .finish(&response.message, &response.confirmation)
                .err(),
            Some(PakeError::ConfirmationFailed),
            "a different code derives a different key and the host MAC does not verify"
        );
    }

    #[test]
    fn another_device_cannot_replay_the_first_message() {
        let device_context = context("123456", DeviceId::new());
        let host_context = context("123456", DeviceId::new());

        let (device_message, device_state) =
            DevicePake::start(&device_context).expect("device start");
        let response = HostPake::respond(&host_context, &device_message).expect("host respond");
        assert_eq!(
            device_state
                .finish(&response.message, &response.confirmation)
                .err(),
            Some(PakeError::ConfirmationFailed),
            "the scalar commits to the requesting device"
        );
    }

    #[test]
    fn a_relay_cannot_prove_its_own_certificate() {
        let device = DeviceId::new();
        let context = context("123456", device);
        let (device_message, device_state) = DevicePake::start(&context).expect("device start");
        let response = HostPake::respond(&context, &device_message).expect("host respond");
        let finished = device_state
            .finish(&response.message, &response.confirmation)
            .expect("host confirmation");

        // The host proves its own certificate; the relay cannot prove another.
        let host_proof = response.state.certificate_proof(b"host certificate der");
        assert_eq!(
            verify_certificate_proof(&finished.session_key, b"relay certificate der", &host_proof),
            Err(PakeError::CertificateUnverified)
        );
    }

    #[test]
    fn malformed_peer_messages_are_rejected_before_any_derivation() {
        let device = DeviceId::new();
        let context = context("123456", device);

        assert_eq!(
            HostPake::respond(&context, &[0u8; PAKE_MESSAGE_BYTES - 1]).err(),
            Some(PakeError::InvalidMessage),
            "compressed or truncated encodings are not accepted"
        );

        let (device_message, device_state) = DevicePake::start(&context).expect("device start");
        let response = HostPake::respond(&context, &device_message).expect("host respond");
        assert_eq!(
            device_state
                .finish(&response.message, &response.confirmation[..31])
                .err(),
            Some(PakeError::InvalidMessage),
            "a short confirmation MAC is malformed, not merely wrong"
        );
    }
}
