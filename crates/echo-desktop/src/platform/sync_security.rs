//! Local app-data secrets and publication checkpoints for LAN synchronization.

use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

use echo_core::application::ports::PublicationCheckpointPort;
use echo_core::domain::sync::{CommitCheckpoint, SyncBinding};
use echo_core::error::{Error, Subject};
use zeroize::Zeroize;

static CHECKPOINT_GATE: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
static VAULT_GATE: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
static VAULT_ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
static TEMP_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Configure the app-private directory used for LAN sync credentials.
/// Must be called by desktop composition before sync can be started.
pub fn configure_local_vault(path: PathBuf) -> Result<(), Error> {
    VAULT_ROOT
        .set(path)
        .map_err(|_| unavailable("local sync vault was already configured"))
}

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

/// Credentials stored outside the library database in Echo's app-data vault.
/// On Unix the directory and files are owner-only (0700/0600).
pub struct OsSecureStore;

impl OsSecureStore {
    /// Store a secret in Echo's local app-data vault.
    ///
    /// # Errors
    /// Returns an unavailable error if the native secure store cannot persist it.
    pub fn set(&self, handle: &SecretHandle, secret: &SecretBytes) -> Result<(), Error> {
        let _guard = vault_gate()
            .lock()
            .map_err(|_| unavailable("vault lock failed"))?;
        let root = vault_root()?;
        ensure_private_dir(&root)?;
        let path = credential_path(&root, handle);
        let sequence = TEMP_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let temp = root.join(format!(
            ".{}.{}-{sequence}.tmp",
            handle.as_str(),
            std::process::id()
        ));
        let mut encoded = encode_hex(secret.expose());
        let result = (|| {
            let mut options = OpenOptions::new();
            options.write(true).create_new(true);
            set_private_file_mode(&mut options);
            let mut file = options
                .open(&temp)
                .map_err(|_| unavailable("local sync vault could not create a credential"))?;
            file.write_all(encoded.as_bytes())
                .map_err(|_| unavailable("local sync vault could not write a credential"))?;
            file.sync_all()
                .map_err(|_| unavailable("local sync vault could not flush a credential"))?;
            #[cfg(windows)]
            if path.exists() {
                fs::remove_file(&path)
                    .map_err(|_| unavailable("local sync vault could not replace a credential"))?;
            }
            fs::rename(&temp, &path)
                .map_err(|_| unavailable("local sync vault could not publish a credential"))?;
            Ok(())
        })();
        encoded.zeroize();
        if result.is_err() {
            let _ = fs::remove_file(temp);
        }
        result
    }

    /// Load a secret from the operating system credential manager.
    ///
    /// # Errors
    /// Returns unavailable for a missing or unreadable credential and rejects
    /// corrupt stored encodings.
    pub fn get(&self, handle: &SecretHandle) -> Result<SecretBytes, Error> {
        let _guard = vault_gate()
            .lock()
            .map_err(|_| unavailable("vault lock failed"))?;
        let mut stored = fs::read_to_string(credential_path(&vault_root()?, handle))
            .map_err(|_| unavailable("local sync vault credential is unavailable"))?;
        let bytes = decode_hex(&stored).ok_or_else(|| Error::InvariantViolation {
            why: "local sync vault credential encoding is corrupt".to_owned(),
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
        let _guard = vault_gate()
            .lock()
            .map_err(|_| unavailable("vault lock failed"))?;
        fs::remove_file(credential_path(&vault_root()?, handle))
            .map_err(|_| unavailable("local sync vault could not remove the credential"))
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
        if credential_path(&vault_root()?, &checkpoint.handle).exists() {
            return Err(Error::conflict(
                "publication checkpoint already exists; explicit recovery is required",
            ));
        }
        let genesis = CommitCheckpoint {
            binding,
            sequence: echo_core::domain::sync::CommitSeq::GENESIS,
            digest: [0; 32],
        };
        checkpoint.write(genesis)?;
        Ok(checkpoint)
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

fn vault_root() -> Result<PathBuf, Error> {
    VAULT_ROOT
        .get()
        .cloned()
        .ok_or_else(|| unavailable("local sync vault is not configured"))
}

fn credential_path(root: &std::path::Path, handle: &SecretHandle) -> PathBuf {
    root.join(format!("{}.secret", handle.as_str()))
}

fn ensure_private_dir(root: &std::path::Path) -> Result<(), Error> {
    fs::create_dir_all(root)
        .map_err(|_| unavailable("local sync vault directory is unavailable"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(root, fs::Permissions::from_mode(0o700))
            .map_err(|_| unavailable("local sync vault permissions could not be set"))?;
    }
    Ok(())
}

fn set_private_file_mode(options: &mut OpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    #[cfg(not(unix))]
    let _ = options;
}

fn checkpoint_gate() -> &'static std::sync::Mutex<()> {
    CHECKPOINT_GATE.get_or_init(|| std::sync::Mutex::new(()))
}

fn vault_gate() -> &'static std::sync::Mutex<()> {
    VAULT_GATE.get_or_init(|| std::sync::Mutex::new(()))
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
    if !value.len().is_multiple_of(2) {
        return None;
    }
    value
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| {
            let high = (pair[0] as char).to_digit(16)?;
            let low = (pair[1] as char).to_digit(16)?;
            u8::try_from((high << 4) | low).ok()
        })
        .collect()
}
