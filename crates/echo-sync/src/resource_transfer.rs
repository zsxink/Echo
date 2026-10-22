//! Durable, content-addressed resource transfer. Network paths never become disk paths.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use echo_core::domain::sync::{DeviceId, SyncBinding};
use serde::{Deserialize, Serialize};

use crate::protocol::{validate_resource, ResourceRefDto, MAX_RESOURCE_CHUNK_BYTES};

mod local_import;

/// The upper resource size limit also bounds checkpoint metadata (4,096 chunks).
pub const MAX_RESOURCE_BYTES: u64 = 16 * 1024 * 1024 * 1024;
const MAX_ACTIVE_UPLOADS: usize = 32;

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct PrepareResource {
    pub binding: SyncBinding,
    pub resource: ResourceRefDto,
}

#[derive(Clone, Debug, Serialize, Deserialize, Eq, PartialEq)]
pub struct TransferProgress {
    pub upload_id: String,
    pub resource: ResourceRefDto,
    pub chunk_size: usize,
    pub confirmed_chunks: Vec<u32>,
    pub complete: bool,
}

#[derive(Clone, Debug)]
pub struct DownloadChunk {
    pub bytes: Vec<u8>,
    pub total_length: u64,
    pub content_hash: String,
    pub chunk_hash: String,
}

#[derive(Debug)]
pub enum TransferError {
    Invalid,
    Unknown,
    Conflict,
    Capacity,
    Storage(std::io::Error),
}

impl std::fmt::Display for TransferError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Invalid => "invalid resource transfer",
            Self::Unknown => "resource transfer unavailable",
            Self::Conflict => "resource transfer content mismatch",
            Self::Capacity => "resource transfer capacity exceeded",
            Self::Storage(_) => "resource storage unavailable",
        })
    }
}
impl std::error::Error for TransferError {}
impl From<std::io::Error> for TransferError {
    fn from(value: std::io::Error) -> Self {
        Self::Storage(value)
    }
}

/// Storage operations are blocking and are invoked on the blocking executor.
/// Implementations must scope every handle and digest to the supplied binding.
pub trait ResourceTransferPort: Send + Sync {
    /// Declare or resume an upload owned by an authenticated device.
    ///
    /// # Errors
    /// Rejects unsafe metadata or unavailable storage.
    fn prepare(
        &self,
        owner: DeviceId,
        request: &PrepareResource,
    ) -> Result<TransferProgress, TransferError>;
    /// Persist one chunk before acknowledging its checkpoint.
    ///
    /// # Errors
    /// Rejects unknown handles, invalid ranges or inconsistent retries.
    fn write_chunk(
        &self,
        binding: SyncBinding,
        owner: DeviceId,
        upload: &str,
        index: u32,
        hash: &str,
        bytes: &[u8],
    ) -> Result<TransferProgress, TransferError>;
    /// Verify and publish immutable bytes.
    ///
    /// # Errors
    /// Rejects incomplete content or full-file digest mismatches.
    fn complete(
        &self,
        binding: SyncBinding,
        owner: DeviceId,
        upload: &str,
    ) -> Result<TransferProgress, TransferError>;
    /// Verify a declared resource is durably complete in this library partition.
    ///
    /// The default rejects access until an adapter implements content verification.
    ///
    /// # Errors
    /// Returns `Unknown` for absent completion and `Conflict` for damaged content.
    fn verify_complete(
        &self,
        _binding: SyncBinding,
        _resource: &ResourceRefDto,
    ) -> Result<(), TransferError> {
        Err(TransferError::Unknown)
    }

    /// Read a bounded range from a library-scoped completed resource.
    ///
    /// # Errors
    /// Rejects unknown resources, invalid ranges or storage failures.
    fn download(
        &self,
        binding: SyncBinding,
        hash: &str,
        offset: u64,
        length: usize,
    ) -> Result<DownloadChunk, TransferError>;
}

#[derive(Clone, Serialize, Deserialize)]
struct Checkpoint {
    owner: DeviceId,
    binding: SyncBinding,
    progress: TransferProgress,
}

/// App-private immutable content store.
///
/// Only digest-derived filenames are used. User-controlled relative paths are
/// metadata for later Core publication. Transfer checkpoints survive restart
/// for recovery; completed resources are explicitly reclaimed after use.
pub struct FileResourceStore {
    root: PathBuf,
    lock: Mutex<()>,
}

impl FileResourceStore {
    /// Open an app-private store. Existing symlink components are rejected.
    ///
    /// # Errors
    /// Reports an unsafe root or unavailable storage.
    pub fn new(root: PathBuf) -> Result<Self, TransferError> {
        safe_directory(&root)?;
        Ok(Self {
            root: fs::canonicalize(root)?,
            lock: Mutex::new(()),
        })
    }

    /// # Errors
    /// Reports unsafe storage, an unusable transfer directory, or two
    /// conflicting writers for the same resource.
    /// Reclaim resources from completed transfers after the caller has safely
    /// consumed them. Incomplete uploads are retained for restart recovery.
    /// A blob shared with an incomplete transfer is retained as well.
    pub fn cleanup_completed(&self, binding: SyncBinding) -> Result<(), TransferError> {
        self.cleanup_completed_resources(binding, None, None)
    }

    /// # Errors
    /// Reports unsafe storage, an unusable transfer directory, or two
    /// conflicting writers for the same resource.
    /// Reclaim selected completed resources after they have been published
    /// into the authoritative library. Other completed transfers are untouched.
    pub fn cleanup_resources(
        &self,
        binding: SyncBinding,
        hashes: &[String],
    ) -> Result<(), TransferError> {
        self.cleanup_completed_resources(binding, Some(hashes), None)
    }

    /// # Errors
    /// Reports unsafe storage, an unusable transfer directory, or two
    /// conflicting writers for the same resource.
    /// Reclaim stale transfers not referenced by any durable library
    /// declaration. Recent uploads are retained because a peer uploads bytes
    /// just before submitting the mutation that declares them.
    pub fn cleanup_stale(
        &self,
        binding: SyncBinding,
        referenced_hashes: &[String],
        max_age: Duration,
    ) -> Result<(), TransferError> {
        let _lock = self.lock.lock().map_err(|_| TransferError::Conflict)?;
        let directory = self.directory(binding)?;
        local_import::cleanup_stale_snapshots(&directory, max_age)?;
        let referenced: std::collections::HashSet<&str> =
            referenced_hashes.iter().map(String::as_str).collect();
        let now = SystemTime::now();
        let mut stale = Vec::new();
        let mut retained_hashes = std::collections::HashSet::new();
        for entry in fs::read_dir(&directory)? {
            let path = entry?.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            regular_file(&path)?;
            let file = File::open(&path)?;
            let checkpoint: Checkpoint = serde_json::from_reader(file.take(2 * 1024 * 1024))
                .map_err(|_| TransferError::Conflict)?;
            if checkpoint.binding != binding {
                return Err(TransferError::Conflict);
            }
            let hash = checkpoint.progress.resource.content_hash;
            let age = fs::metadata(&path)?
                .modified()
                .ok()
                .and_then(|time| now.duration_since(time).ok());
            if !referenced.contains(hash.as_str()) && age.is_some_and(|age| age >= max_age) {
                stale.push((
                    path,
                    hash,
                    checkpoint.progress.upload_id,
                    checkpoint.progress.complete,
                ));
            } else {
                retained_hashes.insert(hash);
            }
        }
        for (checkpoint, hash, upload_id, complete) in stale {
            if !retained_hashes.contains(&hash) && complete {
                let blob = directory.join(format!("{hash}.blob"));
                if blob.exists() {
                    regular_file(&blob)?;
                    fs::remove_file(blob)?;
                }
            }
            let part = directory.join(format!("{upload_id}.part"));
            if part.exists() {
                regular_file(&part)?;
                fs::remove_file(part)?;
            }
            fs::remove_file(checkpoint)?;
        }
        sync_directory(&directory)?;
        Ok(())
    }

    /// # Errors
    /// Reports unsafe storage, an unusable transfer directory, or two
    /// conflicting writers for the same resource.
    /// Reclaim completed resources created by one owner, used for host-local
    /// materializations once the serving session has stopped.
    pub fn cleanup_owner(
        &self,
        binding: SyncBinding,
        owner: DeviceId,
    ) -> Result<(), TransferError> {
        self.cleanup_completed_resources(binding, None, Some(owner))
    }

    /// # Errors
    /// Reports unsafe storage, an unusable transfer directory, or two
    /// conflicting writers for the same resource.
    /// Remove every resource owned by a binding which has been detached.
    pub fn clear_binding(&self, binding: SyncBinding) -> Result<(), TransferError> {
        let _lock = self.lock.lock().map_err(|_| TransferError::Conflict)?;
        let directory = self.directory(binding)?;
        fs::remove_dir_all(&directory)?;
        sync_directory(&self.root)?;
        Ok(())
    }

    fn cleanup_completed_resources(
        &self,
        binding: SyncBinding,
        selected_hashes: Option<&[String]>,
        selected_owner: Option<DeviceId>,
    ) -> Result<(), TransferError> {
        let _lock = self.lock.lock().map_err(|_| TransferError::Conflict)?;
        let directory = self.directory(binding)?;
        let mut completed = Vec::new();
        let mut retained_hashes = std::collections::HashSet::new();
        let selected_hashes: Option<std::collections::HashSet<&str>> =
            selected_hashes.map(|hashes| hashes.iter().map(String::as_str).collect());
        for entry in fs::read_dir(&directory)? {
            let path = entry?.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            regular_file(&path)?;
            let file = File::open(&path)?;
            let checkpoint: Checkpoint = serde_json::from_reader(file.take(2 * 1024 * 1024))
                .map_err(|_| TransferError::Conflict)?;
            if checkpoint.binding != binding {
                return Err(TransferError::Conflict);
            }
            let hash = checkpoint.progress.resource.content_hash;
            if checkpoint.progress.complete
                && selected_hashes
                    .as_ref()
                    .is_none_or(|selected| selected.contains(hash.as_str()))
                && selected_owner.is_none_or(|owner| checkpoint.owner == owner)
            {
                completed.push((path, hash, checkpoint.progress.upload_id));
            } else {
                retained_hashes.insert(hash);
            }
        }

        for (checkpoint, hash, upload_id) in completed {
            // An atomic local import may finish before its old network part
            // was removed. Keep the checkpoint until both files are reclaimed.
            validate_hash(&upload_id)?;
            let part = directory.join(format!("{upload_id}.part"));
            if part.exists() {
                regular_file(&part)?;
                fs::remove_file(part)?;
            }
            if !retained_hashes.contains(&hash) {
                let blob = directory.join(format!("{hash}.blob"));
                if blob.exists() {
                    regular_file(&blob)?;
                    fs::remove_file(blob)?;
                }
            }
            // Keep the checkpoint as the recovery marker until its blob has
            // been removed successfully. A crash or unlink error can then be
            // retried by the next cleanup pass.
            fs::remove_file(checkpoint)?;
        }
        sync_directory(&directory)?;
        Ok(())
    }

    /// Import a library-authorized local reader into immutable storage.
    /// The caller resolves and authorizes the local source through Core.
    ///
    /// # Errors
    /// Rejects changed length/content or unavailable durable storage.
    pub fn import_reader(
        &self,
        owner: DeviceId,
        request: &PrepareResource,
        reader: &mut dyn Read,
    ) -> Result<TransferProgress, TransferError> {
        let progress = self.prepare(owner, request)?;
        if progress.complete {
            return Ok(progress);
        }
        let chunks = request
            .resource
            .length
            .div_ceil(MAX_RESOURCE_CHUNK_BYTES as u64);
        for index in 0..chunks {
            let length = (request.resource.length - index * MAX_RESOURCE_CHUNK_BYTES as u64)
                .min(MAX_RESOURCE_CHUNK_BYTES as u64);
            let mut bytes = vec![0; usize::try_from(length).map_err(|_| TransferError::Invalid)?];
            reader.read_exact(&mut bytes)?;
            let hash = blake3::hash(&bytes).to_hex().to_string();
            self.write_chunk(
                request.binding,
                owner,
                &progress.upload_id,
                u32::try_from(index).map_err(|_| TransferError::Invalid)?,
                &hash,
                &bytes,
            )?;
        }
        if reader.read(&mut [0])? != 0 {
            return Err(TransferError::Conflict);
        }
        self.complete(request.binding, owner, &progress.upload_id)
    }

    /// Copy a byte-identical blob from another local binding partition into
    /// this binding's partition. The requested digest and length are verified
    /// before import; callers must first authorize the manifest for this binding.
    ///
    /// # Errors
    /// Rejects invalid resource declarations, unsafe cache entries, and storage
    /// failures. Returns `false` when no verified matching blob exists.
    pub fn import_cached_blob(
        &self,
        owner: DeviceId,
        request: &PrepareResource,
    ) -> Result<bool, TransferError> {
        validate_resource(&request.resource).map_err(|_| TransferError::Invalid)?;
        if request.resource.length > MAX_RESOURCE_BYTES {
            return Err(TransferError::Capacity);
        }
        let active = self.directory(request.binding)?;
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if !file_type.is_dir() {
                continue;
            }
            let partition_name = entry.file_name();
            let Some(partition_name) = partition_name.to_str() else {
                continue;
            };
            if validate_hash(partition_name).is_err() || entry.path() == active {
                continue;
            }
            let partition = entry.path();
            if existing_directory(&partition).is_err() {
                continue;
            }
            let blob = partition.join(format!("{}.blob", request.resource.content_hash));
            match verify_file(&blob, &request.resource) {
                Ok(()) => {
                    let mut source = File::open(blob)?;
                    self.import_reader(owner, request, &mut source)?;
                    return Ok(true);
                }
                Err(TransferError::Unknown | TransferError::Invalid | TransferError::Conflict) => {}
                Err(error) => return Err(error),
            }
        }
        Ok(false)
    }

    fn directory(&self, binding: SyncBinding) -> Result<PathBuf, TransferError> {
        let encoded = serde_json::to_vec(&binding).map_err(|_| TransferError::Invalid)?;
        let path = self.root.join(blake3::hash(&encoded).to_hex().as_str());
        safe_directory(&path)?;
        Ok(path)
    }

    /// True only when the immutable blob for this declaration exists and its
    /// length and BLAKE3 digest still match.
    fn blob_matches(directory: &Path, resource: &ResourceRefDto) -> bool {
        validate_hash(&resource.content_hash).is_ok()
            && verify_file(
                &directory.join(format!("{}.blob", resource.content_hash)),
                resource,
            )
            .is_ok()
    }

    /// Remove cache state that can no longer describe the declared resource.
    ///
    /// A truncated/unparsable checkpoint, a mismatched declaration, or a blob
    /// whose bytes no longer match the authoritative hash must never block a
    /// later transfer: the declaration is authoritative, so the invalid local
    /// cache entry is dropped and the resource is fetched again. Only the blob
    /// that fails verification for *this* declaration is removed, so another
    /// transfer sharing the same digest keeps its valid bytes.
    fn discard_corrupt_state(
        directory: &Path,
        resource: &ResourceRefDto,
        upload: &str,
    ) -> Result<(), TransferError> {
        if validate_hash(upload).is_ok() {
            for suffix in ["json", "json.tmp", "part"] {
                let path = directory.join(format!("{upload}.{suffix}"));
                match fs::remove_file(&path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
        }
        if validate_hash(&resource.content_hash).is_ok() {
            let blob = directory.join(format!("{}.blob", resource.content_hash));
            if blob.exists() && verify_file(&blob, resource).is_err() {
                match fs::remove_file(&blob) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
        }
        sync_directory(directory)
    }

    fn load(
        directory: &Path,
        binding: SyncBinding,
        owner: DeviceId,
        upload: &str,
    ) -> Result<Checkpoint, TransferError> {
        validate_hash(upload)?;
        let path = directory.join(format!("{upload}.json"));
        regular_file(&path)?;
        let file = File::open(path)?;
        let checkpoint: Checkpoint = serde_json::from_reader(file.take(2 * 1024 * 1024))
            .map_err(|_| TransferError::Invalid)?;
        if checkpoint.binding != binding
            || checkpoint.owner != owner
            || checkpoint.progress.upload_id != upload
        {
            return Err(TransferError::Unknown);
        }
        validate_resource(&checkpoint.progress.resource).map_err(|_| TransferError::Invalid)?;
        Ok(checkpoint)
    }

    fn save(directory: &Path, checkpoint: &Checkpoint) -> Result<(), TransferError> {
        let path = directory.join(format!("{}.json", checkpoint.progress.upload_id));
        let temp = directory.join(format!("{}.json.tmp", checkpoint.progress.upload_id));
        if temp.exists() {
            regular_file(&temp)?;
            fs::remove_file(&temp)?;
        }
        if path.exists() {
            regular_file(&path)?;
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        serde_json::to_writer(&mut file, checkpoint).map_err(|_| TransferError::Invalid)?;
        file.sync_all()?;
        fs::rename(temp, path)?;
        sync_directory(directory)?;
        Ok(())
    }
}

impl ResourceTransferPort for FileResourceStore {
    fn verify_complete(
        &self,
        binding: SyncBinding,
        resource: &ResourceRefDto,
    ) -> Result<(), TransferError> {
        validate_resource(resource).map_err(|_| TransferError::Invalid)?;
        if resource.length > MAX_RESOURCE_BYTES {
            return Err(TransferError::Capacity);
        }
        let _lock = self.lock.lock().map_err(|_| TransferError::Conflict)?;
        let encoded = serde_json::to_vec(&binding).map_err(|_| TransferError::Invalid)?;
        let directory = self.root.join(blake3::hash(&encoded).to_hex().as_str());
        existing_directory(&directory)?;
        let blob = directory.join(format!("{}.blob", resource.content_hash));
        if regular_file(&blob).is_err() || verify_file(&blob, resource).is_err() {
            // Cache state that contradicts the authoritative declaration is
            // invalid cache, not a permanent binding conflict. Drop it so the
            // resource is transferred again instead of blocking forever.
            Self::discard_corrupt_state(&directory, resource, "")?;
            return Err(content_verification_error(TransferError::Unknown));
        }

        // Upload identities include the owner and the complete declaration. A
        // blob alone cannot prove that a durable completion checkpoint exists.
        let mut completed = false;
        for entry in fs::read_dir(&directory)? {
            let path = entry?.path();
            if path.extension().is_none_or(|extension| extension != "json") {
                continue;
            }
            regular_file(&path).map_err(content_verification_error)?;
            let file = File::open(&path)?;
            if file.metadata()?.len() > 2 * 1024 * 1024 {
                return Err(TransferError::Conflict);
            }
            let checkpoint: Checkpoint = serde_json::from_reader(file.take(2 * 1024 * 1024))
                .map_err(|_| TransferError::Conflict)?;
            if checkpoint.binding != binding {
                return Err(TransferError::Conflict);
            }
            if checkpoint.progress.resource.content_hash != resource.content_hash {
                continue;
            }
            validate_resource(&checkpoint.progress.resource)
                .map_err(|_| TransferError::Conflict)?;
            if checkpoint.progress.resource.length != resource.length
                || checkpoint.progress.resource.media_type != resource.media_type
            {
                return Err(TransferError::Conflict);
            }
            if checkpoint.progress.complete && checkpoint.progress.resource == *resource {
                let request = PrepareResource {
                    binding,
                    resource: resource.clone(),
                };
                let identity = serde_json::to_vec(&(checkpoint.owner, request))
                    .map_err(|_| TransferError::Conflict)?;
                let upload = blake3::hash(&identity).to_hex().to_string();
                if checkpoint.progress.upload_id != upload
                    || path.file_stem().and_then(|name| name.to_str()) != Some(upload.as_str())
                    || checkpoint.progress.chunk_size != MAX_RESOURCE_CHUNK_BYTES
                {
                    return Err(TransferError::Conflict);
                }
                completed = true;
            }
        }
        if !completed {
            return Err(TransferError::Unknown);
        }
        verify_file(&blob, resource).map_err(content_verification_error)
    }

    fn prepare(
        &self,
        owner: DeviceId,
        request: &PrepareResource,
    ) -> Result<TransferProgress, TransferError> {
        validate_resource(&request.resource).map_err(|_| TransferError::Invalid)?;
        if request.resource.length > MAX_RESOURCE_BYTES
            || (request.resource.kind != echo_core::domain::sync::SharedResourceKind::Audio
                && request.resource.length > 32 * 1024 * 1024)
        {
            return Err(TransferError::Capacity);
        }
        let _lock = self.lock.lock().map_err(|_| TransferError::Conflict)?;
        let directory = self.directory(request.binding)?;
        let encoded = serde_json::to_vec(&(owner, request)).map_err(|_| TransferError::Invalid)?;
        let upload = blake3::hash(&encoded).to_hex().to_string();
        if let Ok(checkpoint) = Self::load(&directory, request.binding, owner, &upload) {
            if checkpoint.progress.resource == request.resource
                && (!checkpoint.progress.complete
                    || Self::blob_matches(&directory, &request.resource))
            {
                return Ok(checkpoint.progress);
            }
        }
        // A damaged checkpoint, a partially written `.part`, or a "complete"
        // checkpoint whose blob no longer matches the declaration cannot be
        // resumed. The declaration is authoritative, so discard that state and
        // start the transfer over instead of blocking this resource forever.
        Self::discard_corrupt_state(&directory, &request.resource, &upload)?;
        let blob = directory.join(format!("{}.blob", request.resource.content_hash));
        let complete = blob.exists() && Self::blob_matches(&directory, &request.resource);
        if !complete && blob.exists() {
            Self::discard_corrupt_state(&directory, &request.resource, "")?;
        }
        let mut active = 0;
        for entry in fs::read_dir(&directory)? {
            if entry?
                .path()
                .extension()
                .is_some_and(|extension| extension == "part")
            {
                active += 1;
            }
        }
        if !complete && active >= MAX_ACTIVE_UPLOADS {
            return Err(TransferError::Capacity);
        }
        let progress = TransferProgress {
            upload_id: upload,
            resource: request.resource.clone(),
            chunk_size: MAX_RESOURCE_CHUNK_BYTES,
            confirmed_chunks: Vec::new(),
            complete,
        };
        if !complete {
            let part = directory.join(format!("{}.part", progress.upload_id));
            if part.exists() {
                regular_file(&part)?;
            } else {
                OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(part)?
                    .sync_all()?;
            }
        }
        Self::save(
            &directory,
            &Checkpoint {
                owner,
                binding: request.binding,
                progress: progress.clone(),
            },
        )?;
        Ok(progress)
    }

    fn write_chunk(
        &self,
        binding: SyncBinding,
        owner: DeviceId,
        upload: &str,
        index: u32,
        hash: &str,
        bytes: &[u8],
    ) -> Result<TransferProgress, TransferError> {
        validate_hash(hash)?;
        if bytes.is_empty()
            || bytes.len() > MAX_RESOURCE_CHUNK_BYTES
            || blake3::hash(bytes).to_hex().as_str() != hash
        {
            return Err(TransferError::Invalid);
        }
        let _lock = self.lock.lock().map_err(|_| TransferError::Conflict)?;
        let directory = self.directory(binding)?;
        let mut checkpoint = Self::load(&directory, binding, owner, upload)?;
        let offset = u64::from(index) * MAX_RESOURCE_CHUNK_BYTES as u64;
        let expected = checkpoint
            .progress
            .resource
            .length
            .saturating_sub(offset)
            .min(MAX_RESOURCE_CHUNK_BYTES as u64);
        if offset >= checkpoint.progress.resource.length || bytes.len() as u64 != expected {
            return Err(TransferError::Invalid);
        }
        let suffix = if checkpoint.progress.complete {
            "blob"
        } else {
            "part"
        };
        let key = if checkpoint.progress.complete {
            checkpoint.progress.resource.content_hash.as_str()
        } else {
            upload
        };
        let path = directory.join(format!("{key}.{suffix}"));
        regular_file(&path)?;
        let mut file = OpenOptions::new()
            .read(true)
            .write(!checkpoint.progress.complete)
            .open(path)?;
        file.seek(SeekFrom::Start(offset))?;
        if checkpoint.progress.complete || checkpoint.progress.confirmed_chunks.contains(&index) {
            let mut stored = vec![0; bytes.len()];
            file.read_exact(&mut stored)?;
            if stored != bytes {
                return Err(TransferError::Conflict);
            }
            return Ok(checkpoint.progress);
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        checkpoint.progress.confirmed_chunks.push(index);
        checkpoint.progress.confirmed_chunks.sort_unstable();
        Self::save(&directory, &checkpoint)?;
        Ok(checkpoint.progress)
    }

    fn complete(
        &self,
        binding: SyncBinding,
        owner: DeviceId,
        upload: &str,
    ) -> Result<TransferProgress, TransferError> {
        let _lock = self.lock.lock().map_err(|_| TransferError::Conflict)?;
        let directory = self.directory(binding)?;
        let checkpoint = Self::load(&directory, binding, owner, upload)?;
        let mut checkpoint = checkpoint;
        if checkpoint.progress.complete {
            return Ok(checkpoint.progress);
        }
        let count = checkpoint
            .progress
            .resource
            .length
            .div_ceil(MAX_RESOURCE_CHUNK_BYTES as u64);
        if checkpoint.progress.confirmed_chunks.len() as u64 != count
            || checkpoint
                .progress
                .confirmed_chunks
                .iter()
                .enumerate()
                .any(|(index, chunk)| u32::try_from(index).ok() != Some(*chunk))
        {
            return Err(TransferError::Conflict);
        }
        let part = directory.join(format!("{upload}.part"));
        let blob = directory.join(format!(
            "{}.blob",
            checkpoint.progress.resource.content_hash
        ));
        // A crash after publishing but before saving the checkpoint is replayable.
        if blob.exists() {
            verify_file(&blob, &checkpoint.progress.resource)?;
            if part.exists() {
                regular_file(&part)?;
                fs::remove_file(part)?;
            }
        } else {
            verify_file(&part, &checkpoint.progress.resource)?;
            // Hard-link publishes without replacing any preexisting immutable blob.
            fs::hard_link(&part, &blob)?;
            sync_directory(&directory)?;
            fs::remove_file(part)?;
        }
        checkpoint.progress.complete = true;
        Self::save(&directory, &checkpoint)?;
        Ok(checkpoint.progress)
    }

    fn download(
        &self,
        binding: SyncBinding,
        hash: &str,
        offset: u64,
        length: usize,
    ) -> Result<DownloadChunk, TransferError> {
        validate_hash(hash)?;
        if length == 0 || length > MAX_RESOURCE_CHUNK_BYTES {
            return Err(TransferError::Invalid);
        }
        let _lock = self.lock.lock().map_err(|_| TransferError::Conflict)?;
        let path = self.directory(binding)?.join(format!("{hash}.blob"));
        regular_file(&path)?;
        let mut file = File::open(path)?;
        let total_length = file.metadata()?.len();
        if offset > total_length || (offset == total_length && total_length != 0) {
            return Err(TransferError::Invalid);
        }
        let length = length.min(usize::try_from(total_length - offset).unwrap_or(usize::MAX));
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = vec![0; length];
        file.read_exact(&mut bytes)?;
        let chunk_hash = blake3::hash(&bytes).to_hex().to_string();
        Ok(DownloadChunk {
            bytes,
            total_length,
            content_hash: hash.to_owned(),
            chunk_hash,
        })
    }
}

pub(crate) fn validate_hash(value: &str) -> Result<(), TransferError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(TransferError::Invalid);
    }
    Ok(())
}

fn regular_file(path: &Path) -> Result<(), TransferError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_file() => Ok(()),
        Ok(_) => Err(TransferError::Invalid),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(TransferError::Unknown),
        Err(error) => Err(error.into()),
    }
}

fn content_verification_error(error: TransferError) -> TransferError {
    match error {
        TransferError::Invalid => TransferError::Conflict,
        other => other,
    }
}

/// Verification never creates a missing library partition and rejects symlinks
/// at every existing directory component, including a replaced private root.
fn existing_directory(path: &Path) -> Result<(), TransferError> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        existing_directory(parent)?;
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => Err(TransferError::Conflict),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(TransferError::Unknown),
        Err(error) => Err(error.into()),
    }
}

fn safe_directory(path: &Path) -> Result<(), TransferError> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        safe_directory(parent)?;
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_dir() => Ok(()),
        Ok(_) => Err(TransferError::Invalid),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path)?;
            Ok(())
        }
        Err(error) => Err(error.into()),
    }
}

#[cfg_attr(
    windows,
    allow(clippy::missing_const_for_fn, clippy::unnecessary_wraps)
)]
fn sync_directory(path: &Path) -> Result<(), TransferError> {
    #[cfg(not(windows))]
    File::open(path)?.sync_all()?;
    #[cfg(windows)]
    let _ = path;
    Ok(())
}

fn verify_file(path: &Path, resource: &ResourceRefDto) -> Result<(), TransferError> {
    regular_file(path)?;
    let mut file = File::open(path)?;
    if file.metadata()?.len() != resource.length {
        return Err(TransferError::Conflict);
    }
    let mut hash = blake3::Hasher::new();
    let mut buffer = vec![0; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hash.update(&buffer[..read]);
    }
    if hash.finalize().to_hex().as_str() != resource.content_hash {
        return Err(TransferError::Conflict);
    }
    Ok(())
}

#[cfg(test)]
mod tests;
