//! Local app-data secrets and publication checkpoints for LAN synchronization.

use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[cfg(windows)]
mod windows;

use echo_core::application::ports::PublicationCheckpointPort;
use echo_core::domain::sync::{CommitCheckpoint, SyncBinding};
use echo_core::error::{Error, Subject};
use zeroize::{Zeroize, Zeroizing};

static CHECKPOINT_GATE: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
static VAULT_GATE: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
static VAULT_ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
static TEMP_SEQUENCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Configure the app-private directory used for LAN sync credentials.
/// Must be called by desktop composition before sync can be started.
///
/// # Errors
/// Fails when the vault root was already configured by an earlier call.
pub fn configure_local_vault(path: PathBuf) -> Result<(), Error> {
    VAULT_ROOT
        .set(path)
        .map_err(|_| unavailable("local sync vault was already configured"))
}

/// Opaque app-data vault locator. The handle contains no secret bytes.
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
    /// Returns an unavailable error if the private vault cannot persist it.
    pub fn set(&self, handle: &SecretHandle, secret: &SecretBytes) -> Result<(), Error> {
        let _guard = vault_gate()
            .lock()
            .map_err(|_| unavailable("vault lock failed"))?;
        set_at(&vault_root()?, handle, secret)
    }

    /// Load a secret from Echo's private app-data vault.
    ///
    /// # Errors
    /// Rejects missing, unreadable, unsafe, or corrupt credential files.
    pub fn get(&self, handle: &SecretHandle) -> Result<SecretBytes, Error> {
        let _guard = vault_gate()
            .lock()
            .map_err(|_| unavailable("vault lock failed"))?;
        get_at(&vault_root()?, handle)
    }

    /// Remove a credential after revocation or local unpairing.
    ///
    /// # Errors
    /// Returns unavailable if the private vault cannot remove it durably.
    pub fn delete(&self, handle: &SecretHandle) -> Result<(), Error> {
        let _guard = vault_gate()
            .lock()
            .map_err(|_| unavailable("vault lock failed"))?;
        let root = vault_root()?;
        validate_private_dir(&root)?;
        let path = credential_path(&root, handle);
        validate_private_file(&path)?;
        fs::remove_file(path)
            .map_err(|_| unavailable("local sync vault could not remove the credential"))?;
        sync_directory(&root)
    }
}

fn set_at(root: &Path, handle: &SecretHandle, secret: &SecretBytes) -> Result<(), Error> {
    set_at_with(root, handle, secret, publish_credential)
}

fn set_at_with(
    root: &Path,
    handle: &SecretHandle,
    secret: &SecretBytes,
    publish: impl FnOnce(&Path, &Path) -> Result<(), Error>,
) -> Result<(), Error> {
    if secret.expose().len() > 1024 * 1024 {
        return Err(unavailable("local sync vault credential is oversized"));
    }
    ensure_private_dir(root)?;
    let path = credential_path(root, handle);
    if credential_exists(&path)? {
        validate_private_file(&path)?;
    }
    let sequence = TEMP_SEQUENCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let temp = root.join(format!(
        ".{}.{}-{sequence}.tmp",
        handle.as_str(),
        std::process::id()
    ));
    let mut encoded = Zeroizing::new(encode_hex(secret.expose()));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        set_private_file_mode(&mut options);
        let mut file = options
            .open(&temp)
            .map_err(|_| unavailable("local sync vault could not create a credential"))?;
        restrict_private_file(&temp)?;
        file.write_all(encoded.as_bytes())
            .map_err(|_| unavailable("local sync vault could not write a credential"))?;
        file.sync_all()
            .map_err(|_| unavailable("local sync vault could not flush a credential"))?;
        publish(&temp, &path)?;
        sync_directory(root)?;
        Ok(())
    })();
    encoded.zeroize();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

fn get_at(root: &Path, handle: &SecretHandle) -> Result<SecretBytes, Error> {
    validate_private_dir(root)?;
    let path = credential_path(root, handle);
    validate_private_file(&path)?;
    let file =
        File::open(path).map_err(|_| unavailable("local sync vault credential is unavailable"))?;
    let mut stored = Zeroizing::new(String::new());
    file.take(2 * 1024 * 1024 + 1)
        .read_to_string(&mut stored)
        .map_err(|_| unavailable("local sync vault credential is unavailable"))?;
    let bytes = if stored.len() > 2 * 1024 * 1024 {
        None
    } else {
        decode_hex(&stored)
    };
    stored.zeroize();
    bytes
        .map(SecretBytes::new)
        .ok_or_else(|| Error::InvariantViolation {
            why: "local sync vault credential encoding is corrupt".to_owned(),
        })
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
        let root = vault_root()?;
        ensure_private_dir(&root)?;
        if credential_exists(&credential_path(&root, &checkpoint.handle))? {
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

fn ensure_private_dir(root: &Path) -> Result<(), Error> {
    let missing = !credential_exists(root)?;
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(root)
        .map_err(|_| unavailable("local sync vault directory is unavailable"))?;
    validate_private_dir(root)?;
    if missing {
        // Make the new vault entry durable in the existing app-data directory.
        if let Some(parent) = root.parent() {
            sync_directory(parent)?;
        }
    }
    Ok(())
}

// Only NotFound means absence. Permission errors and symlink slots must never
// cause a checkpoint to be silently initialized over unknown existing history.
fn credential_exists(path: &Path) -> Result<bool, Error> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(_) => Err(unavailable("local sync vault credential is unavailable")),
    }
}

fn validate_private_dir(path: &Path) -> Result<(), Error> {
    validate_private_path(path, true)
}

fn validate_private_file(path: &Path) -> Result<(), Error> {
    validate_private_path(path, false)
}

fn validate_private_path(path: &Path, directory: bool) -> Result<(), Error> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| unavailable("local sync vault path is unavailable"))?;
    if metadata.file_type().is_symlink()
        || (directory && !metadata.is_dir())
        || (!directory && !metadata.is_file())
    {
        return Err(unavailable("local sync vault path is unsafe"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let expected = if directory { 0o700 } else { 0o600 };
        if metadata.uid() != rustix::process::geteuid().as_raw()
            || metadata.mode() & 0o777 != expected
            || (!directory && metadata.nlink() != 1)
        {
            return Err(unavailable("local sync vault permissions are unsafe"));
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(unavailable("local sync vault reparse point is unsafe"));
        }
        windows::restrict_to_current_user(path, directory)?;
    }
    Ok(())
}

fn restrict_private_file(path: &Path) -> Result<(), Error> {
    #[cfg(windows)]
    windows::restrict_to_current_user(path, false)?;
    validate_private_file(path)
}

fn publish_credential(temp: &Path, path: &Path) -> Result<(), Error> {
    #[cfg(windows)]
    return windows::replace(temp, path);
    #[cfg(not(windows))]
    fs::rename(temp, path)
        .map_err(|_| unavailable("local sync vault could not publish a credential"))
}

#[cfg(unix)]
fn sync_directory(root: &Path) -> Result<(), Error> {
    File::open(root)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| unavailable("local sync vault could not flush its directory"))
}

// Windows has no std directory-fsync operation. The file is flushed before
// the safe Windows adapter atomically replaces the directory entry.
#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps)]
const fn sync_directory(_root: &Path) -> Result<(), Error> {
    Ok(())
}

#[cfg(unix)]
fn set_private_file_mode(options: &mut OpenOptions) {
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
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
    // A malformed suffix must also erase the already-decoded secret prefix.
    let mut output = Zeroizing::new(Vec::with_capacity(value.len() / 2));
    for pair in value.as_bytes().as_chunks::<2>().0 {
        let high = (pair[0] as char).to_digit(16)?;
        let low = (pair[1] as char).to_digit(16)?;
        output.push(u8::try_from((high << 4) | low).ok()?);
    }
    Some(std::mem::take(&mut *output))
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::domain::sync::{AuthorityId, CommitSeq, Epoch, LibraryId};

    fn binding() -> SyncBinding {
        SyncBinding {
            library_id: LibraryId::new(),
            authority_id: AuthorityId::new(),
            epoch: Epoch::INITIAL,
        }
    }

    #[test]
    fn vault_round_trip_atomic_replace_and_failed_publish_keep_prior_secret() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("vault");
        let handle = SecretHandle::new("test-secret").unwrap();
        let previous = SecretBytes::new(b"original bearer credential".to_vec());
        set_at(&root, &handle, &previous).unwrap();
        assert_eq!(get_at(&root, &handle).unwrap().expose(), previous.expose());
        let next = SecretBytes::new(b"replacement bearer credential".to_vec());
        let failure = set_at_with(&root, &handle, &next, |_, _| {
            Err(unavailable("injected publication failure"))
        });
        assert!(failure.is_err());
        assert_eq!(get_at(&root, &handle).unwrap().expose(), previous.expose());
        assert_eq!(
            fs::read_dir(&root).unwrap().count(),
            1,
            "failed staging is removed"
        );
        set_at(&root, &handle, &next).unwrap();
        assert_eq!(get_at(&root, &handle).unwrap().expose(), next.expose());
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
    }

    #[test]
    fn unavailable_vault_has_no_fallback_and_diagnostics_redact_secrets() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("not-a-directory");
        fs::write(&root, "occupied").unwrap();
        let handle = SecretHandle::new("test-secret").unwrap();
        let secret = SecretBytes::new(b"never-log-this-bearer".to_vec());
        let error = set_at(&root, &handle, &secret).unwrap_err();
        let diagnostics = format!("{secret:?} {error:?}");
        assert!(!diagnostics.contains("never-log-this-bearer"));
        assert!(diagnostics.contains("REDACTED"));
        assert_eq!(fs::read(&root).unwrap(), b"occupied");
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 1);
        assert!(get_at(&root, &handle).is_err());
    }

    #[test]
    fn corrupt_and_oversized_vault_values_fail_closed() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("vault");
        let handle = SecretHandle::new("test-secret").unwrap();
        set_at(&root, &handle, &SecretBytes::new(vec![1])).unwrap();
        let path = credential_path(&root, &handle);
        fs::write(&path, "not-valid-hex").unwrap();
        assert!(get_at(&root, &handle).is_err());
        fs::write(&path, vec![b'0'; 2 * 1024 * 1024 + 1]).unwrap();
        assert!(get_at(&root, &handle).is_err());
        assert!(set_at(&root, &handle, &SecretBytes::new(vec![0; 1024 * 1024 + 1])).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn unsafe_permissions_symlinks_and_hardlinks_cannot_expose_or_replace_secrets() {
        use std::os::unix::fs::{symlink, PermissionsExt};
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("vault");
        let handle = SecretHandle::new("test-secret").unwrap();
        let secret = SecretBytes::new(b"private".to_vec());
        set_at(&root, &handle, &secret).unwrap();
        let path = credential_path(&root, &handle);
        assert_eq!(
            fs::metadata(&root).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(get_at(&root, &handle).is_err());
        assert!(set_at(&root, &handle, &secret).is_err());
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        let outside = directory.path().join("outside");
        fs::hard_link(&path, &outside).unwrap();
        assert!(get_at(&root, &handle).is_err());
        fs::remove_file(&path).unwrap();
        symlink(&outside, &path).unwrap();
        assert!(get_at(&root, &handle).is_err());
        assert!(set_at(&root, &handle, &secret).is_err());
        let alias = directory.path().join("alias");
        symlink(&root, &alias).unwrap();
        assert!(set_at(&alias, &handle, &secret).is_err());
        assert_eq!(
            fs::read(&outside).unwrap(),
            encode_hex(secret.expose()).as_bytes()
        );
    }

    #[test]
    fn independent_checkpoint_reopens_monotonically_and_never_resets_missing_history() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("independent-vault");
        configure_local_vault(root.clone()).unwrap();
        let binding = binding();
        let checkpoint = SecurePublicationCheckpoint::initialize_new(binding).unwrap();
        let committed = CommitCheckpoint {
            binding,
            sequence: CommitSeq::from_u64(3),
            digest: [7; 32],
        };
        checkpoint.advance(committed).unwrap();
        drop(checkpoint);
        let reopened = SecurePublicationCheckpoint::open_existing(binding).unwrap();
        assert_eq!(reopened.read().unwrap(), committed);
        let stale = CommitCheckpoint {
            sequence: CommitSeq::from_u64(2),
            ..committed
        };
        reopened.advance(stale).unwrap();
        assert_eq!(reopened.read().unwrap(), committed);
        assert!(reopened
            .advance(CommitCheckpoint {
                digest: [8; 32],
                ..committed
            })
            .is_err());
        assert!(SecurePublicationCheckpoint::initialize_new(binding).is_err());
        let path = credential_path(&root, &reopened.handle);
        fs::write(&path, "corrupt").unwrap();
        assert!(SecurePublicationCheckpoint::open_existing(binding).is_err());
        assert!(SecurePublicationCheckpoint::initialize_new(binding).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "corrupt");
        fs::remove_file(path).unwrap();
        assert!(SecurePublicationCheckpoint::open_existing(binding).is_err());
        assert!(reopened.advance(committed).is_err());
    }
}
