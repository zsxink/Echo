//! OS-backed secrets and publication checkpoints for LAN synchronization.
//!
//! These values live outside the library database so copying/restoring a
//! library cannot roll back authorization or the independently published log
//! head. There is deliberately no file or in-memory fallback when the OS
//! credential service is unavailable.

use std::fmt;

use echo_core::application::ports::PublicationCheckpointPort;
use echo_core::domain::sync::{CommitCheckpoint, SyncBinding};
use echo_core::error::{Error, Subject};
use keyring::Entry;
use zeroize::Zeroize;

const SERVICE: &str = "com.echo.music.lan-sync.v1";
static CHECKPOINT_GATE: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();

/// Opaque OS credential-store locator. The handle contains no secret bytes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SecretHandle(String);

impl SecretHandle {
    /// Validate a stable, non-secret identifier for a credential.
    ///
    /// # Errors
    /// Returns an error for empty, oversized, or unsafe identifiers.
    pub fn new(value: impl Into<String>) -> Result<Self, Error> {
        let value = value.into();
        if value.is_empty()
            || value.len() > 160
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
        {
            return Err(Error::validation(
                Subject::Other,
                "credential handle",
                "invalid secure-store identifier",
            ));
        }
        Ok(Self(value))
    }

    /// Return the opaque identifier for database references.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Secret bytes held only for the lifetime of one native operation.
/// Formatting and debug output are intentionally unavailable.
pub struct SecretBytes(Vec<u8>);

impl SecretBytes {
    /// Construct a secret value from owned bytes.
    #[must_use]
    pub const fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    /// Borrow the secret for an immediate cryptographic or protocol operation.
    #[must_use]
    pub fn expose(&self) -> &[u8] {
        &self.0
    }
}

impl Drop for SecretBytes {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl fmt::Debug for SecretBytes {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretBytes([REDACTED])")
    }
}

/// Cross-platform native credential service. Keychain, Credential Manager,
/// and Secret Service are selected by keyring's target-specific backend.
pub struct OsSecureStore;

impl OsSecureStore {
    /// Store a secret in the operating system credential manager.
    ///
    /// # Errors
    /// Returns an unavailable error if the native secure store cannot persist it.
    pub fn set(&self, handle: &SecretHandle, secret: &SecretBytes) -> Result<(), Error> {
        let entry = entry(handle)?;
        let mut encoded = encode_hex(secret.expose());
        let result = entry
            .set_password(&encoded)
            .map_err(|_| unavailable("OS secure store could not save the credential"));
        encoded.zeroize();
        result
    }

    /// Load a secret from the operating system credential manager.
    ///
    /// # Errors
    /// Returns unavailable for a missing or unreadable credential and rejects
    /// corrupt stored encodings.
    pub fn get(&self, handle: &SecretHandle) -> Result<SecretBytes, Error> {
        let mut stored = entry(handle)?
            .get_password()
            .map_err(|_| unavailable("OS secure store credential is unavailable"))?;
        let bytes = decode_hex(&stored).ok_or_else(|| Error::InvariantViolation {
            why: "OS secure store credential encoding is corrupt".to_owned(),
        });
        stored.zeroize();
        let bytes = bytes?;
        Ok(SecretBytes::new(bytes))
    }

    /// Remove a credential after revocation or local unpairing.
    ///
    /// # Errors
    /// Returns unavailable if the secure store cannot remove it.
    pub fn delete(&self, handle: &SecretHandle) -> Result<(), Error> {
        entry(handle)?
            .delete_credential()
            .map_err(|_| unavailable("OS secure store could not remove the credential"))
    }
}

/// Independent checkpoint slot tied to one library authority and epoch.
/// The stored value is only the validated high-water and digest chain head.
pub struct SecurePublicationCheckpoint {
    store: OsSecureStore,
    handle: SecretHandle,
    binding: SyncBinding,
}

impl SecurePublicationCheckpoint {
    /// Create a checkpoint slot only when it does not already exist.
    /// Callers must establish the new library binding before initializing it.
    ///
    /// # Errors
    /// Rejects an existing slot or a secure-store failure.
    pub fn initialize_new(binding: SyncBinding) -> Result<Self, Error> {
        let checkpoint = Self::for_binding(binding)?;
        let _guard = checkpoint_gate()
            .lock()
            .map_err(|_| unavailable("checkpoint lock failed"))?;
        let entry = entry(&checkpoint.handle)?;
        match entry.get_password() {
            Ok(_) => Err(Error::conflict(
                "publication checkpoint already exists; explicit recovery is required",
            )),
            Err(keyring::Error::NoEntry) => {
                let genesis = CommitCheckpoint {
                    binding,
                    sequence: echo_core::domain::sync::CommitSeq::GENESIS,
                    digest: [0; 32],
                };
                checkpoint.write(genesis)?;
                Ok(checkpoint)
            }
            Err(_) => Err(unavailable(
                "OS secure store cannot read the publication checkpoint",
            )),
        }
    }

    /// Open an existing slot for the exact library, authority, and epoch.
    ///
    /// # Errors
    /// Rejects missing or malformed state; it never silently resets history.
    pub fn open_existing(binding: SyncBinding) -> Result<Self, Error> {
        let checkpoint = Self::for_binding(binding)?;
        checkpoint.read()?;
        Ok(checkpoint)
    }

    fn for_binding(binding: SyncBinding) -> Result<Self, Error> {
        let handle = SecretHandle::new(format!(
            "checkpoint-{}-{}-{}",
            binding.library_id.as_uuid(),
            binding.authority_id.as_uuid(),
            binding.epoch.as_u64()
        ))?;
        Ok(Self {
            store: OsSecureStore,
            handle,
            binding,
        })
    }

    /// Read and validate the persisted checkpoint.
    ///
    /// # Errors
    /// Returns an error for unavailable or malformed secure-store state.
    pub fn read(&self) -> Result<CommitCheckpoint, Error> {
        let _guard = checkpoint_gate()
            .lock()
            .map_err(|_| unavailable("checkpoint lock failed"))?;
        let secret = self.store.get(&self.handle)?;
        let checkpoint: CommitCheckpoint =
            serde_json::from_slice(secret.expose()).map_err(|_| Error::InvariantViolation {
                why: "publication checkpoint encoding is corrupt".to_owned(),
            })?;
        if checkpoint.binding != self.binding {
            return Err(Error::conflict("publication checkpoint binding differs"));
        }
        Ok(checkpoint)
    }

    fn write(&self, checkpoint: CommitCheckpoint) -> Result<(), Error> {
        let bytes =
            serde_json::to_vec(&checkpoint).map_err(|_| unavailable("checkpoint encode failed"))?;
        let secret = SecretBytes::new(bytes);
        self.store.set(&self.handle, &secret)
    }
}

impl PublicationCheckpointPort for SecurePublicationCheckpoint {
    fn advance(&self, checkpoint: CommitCheckpoint) -> Result<(), Error> {
        if checkpoint.binding != self.binding {
            return Err(Error::conflict("publication checkpoint binding differs"));
        }
        let _guard = checkpoint_gate()
            .lock()
            .map_err(|_| unavailable("checkpoint lock failed"))?;
        let current_secret = self.store.get(&self.handle)?;
        let current: CommitCheckpoint =
            serde_json::from_slice(current_secret.expose()).map_err(|_| {
                Error::InvariantViolation {
                    why: "publication checkpoint encoding is corrupt".to_owned(),
                }
            })?;
        if current.binding != checkpoint.binding {
            return Err(Error::conflict("publication checkpoint binding differs"));
        }
        if checkpoint.sequence < current.sequence {
            return Ok(());
        }
        if checkpoint.sequence == current.sequence {
            return if checkpoint.digest == current.digest {
                Ok(())
            } else {
                Err(Error::conflict(
                    "publication checkpoint digest differs at the same sequence",
                ))
            };
        }
        self.write(checkpoint)
    }
}

fn entry(handle: &SecretHandle) -> Result<Entry, Error> {
    Entry::new(SERVICE, handle.as_str()).map_err(|_| unavailable("OS secure store is unavailable"))
}

fn checkpoint_gate() -> &'static std::sync::Mutex<()> {
    CHECKPOINT_GATE.get_or_init(|| std::sync::Mutex::new(()))
}

fn unavailable(message: &'static str) -> Error {
    Error::unavailable("OS secure store", message)
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(HEX[usize::from(byte >> 4)]));
        output.push(char::from(HEX[usize::from(byte & 0x0f)]));
    }
    output
}

fn decode_hex(value: &str) -> Option<Vec<u8>> {
    if value.len() % 2 != 0 {
        return None;
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let high = (pair[0] as char).to_digit(16)?;
            let low = (pair[1] as char).to_digit(16)?;
            u8::try_from((high << 4) | low).ok()
        })
        .collect()
}
