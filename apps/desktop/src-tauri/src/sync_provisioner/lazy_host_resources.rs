//! Serve host-local resources directly from the library and stage peer uploads privately.

#[allow(clippy::wildcard_imports)]
use super::*;
use std::fs::File;
use std::time::SystemTime;

const MAX_VERIFIED_FILES: usize = 32;

struct VerifiedFile {
    resource: ResourceRefDto,
    file: Arc<Mutex<File>>,
    length: u64,
    modified: SystemTime,
    last_used: u64,
}

#[derive(Default)]
struct VerifiedFileCache {
    files: HashMap<String, VerifiedFile>,
    clock: u64,
}

pub(super) struct LazyHostResources {
    database: Arc<SqliteDatabase>,
    library: LibraryRoot,
    binding: SyncBinding,
    cache: Arc<FileResourceStore>,
    verified_files: Mutex<VerifiedFileCache>,
}

impl LazyHostResources {
    pub(super) fn new(
        database: Arc<SqliteDatabase>,
        library: LibraryRoot,
        binding: SyncBinding,
        cache: Arc<FileResourceStore>,
    ) -> Self {
        Self {
            database,
            library,
            binding,
            cache,
            verified_files: Mutex::new(VerifiedFileCache::default()),
        }
    }

    fn verify_library_resource(
        &self,
        binding: SyncBinding,
        resource: &ResourceRefDto,
    ) -> Result<Option<bool>, echo_sync::resource_transfer::TransferError> {
        if binding != self.binding {
            return Ok(None);
        }
        {
            let mut cache = self
                .verified_files
                .lock()
                .map_err(|_| echo_sync::resource_transfer::TransferError::Conflict)?;
            let cached = cache.files.get(&resource.content_hash).map(|entry| {
                (
                    entry.resource.clone(),
                    Arc::clone(&entry.file),
                    entry.length,
                    entry.modified,
                )
            });
            if let Some((cached_resource, file, expected_length, expected_modified)) = cached {
                let valid = if cached_resource == *resource {
                    let file = file
                        .lock()
                        .map_err(|_| echo_sync::resource_transfer::TransferError::Conflict)?;
                    let metadata = file.metadata()?;
                    metadata.len() == expected_length
                        && metadata.len() == resource.length
                        && metadata.modified().ok() == Some(expected_modified)
                } else {
                    false
                };
                if valid {
                    cache.clock = cache.clock.wrapping_add(1);
                    let tick = cache.clock;
                    if let Some(entry) = cache.files.get_mut(&resource.content_hash) {
                        entry.last_used = tick;
                    }
                    return Ok(Some(true));
                }
                cache.files.remove(&resource.content_hash);
            }
        }
        let manifest = self
            .database
            .local_library_resource_manifests(self.library.id())
            .map_err(|_| storage_error("read active library resource manifests"))?
            .into_iter()
            .find(|manifest| ResourceRefDto::from(manifest) == *resource);
        let Some(manifest) = manifest else {
            return Ok(None);
        };
        let files =
            super::local_resource_candidates(self.database.as_ref(), &self.library, &manifest)
                .map_err(|_| storage_error("open local library resource"))?;
        if files.is_empty() {
            return Ok(None);
        }
        let file = super::verified_local_resource(files, &resource.content_hash, resource.length)
            .map_err(|_| storage_error("verify local library resource"))?;
        if let Some(file) = file {
            let metadata = file.metadata()?;
            let modified = metadata
                .modified()
                .map_err(|_| storage_error("read local resource modification time"))?;
            let mut cache = self
                .verified_files
                .lock()
                .map_err(|_| echo_sync::resource_transfer::TransferError::Conflict)?;
            cache.clock = cache.clock.wrapping_add(1);
            let tick = cache.clock;
            if !cache.files.contains_key(&resource.content_hash)
                && cache.files.len() >= MAX_VERIFIED_FILES
            {
                if let Some(oldest) = cache
                    .files
                    .iter()
                    .min_by_key(|(_, entry)| entry.last_used)
                    .map(|(hash, _)| hash.clone())
                {
                    cache.files.remove(&oldest);
                }
            }
            cache.files.insert(
                resource.content_hash.clone(),
                VerifiedFile {
                    resource: resource.clone(),
                    file: Arc::new(Mutex::new(file)),
                    length: metadata.len(),
                    modified,
                    last_used: tick,
                },
            );
            Ok(Some(true))
        } else {
            Ok(Some(false))
        }
    }
}

impl ResourceTransferPort for LazyHostResources {
    fn prepare(
        &self,
        owner: DeviceId,
        request: &PrepareResource,
    ) -> Result<
        echo_sync::resource_transfer::TransferProgress,
        echo_sync::resource_transfer::TransferError,
    > {
        self.cache.prepare(owner, request)
    }

    fn write_chunk(
        &self,
        binding: SyncBinding,
        owner: DeviceId,
        upload: &str,
        index: u32,
        hash: &str,
        bytes: &[u8],
    ) -> Result<
        echo_sync::resource_transfer::TransferProgress,
        echo_sync::resource_transfer::TransferError,
    > {
        self.cache
            .write_chunk(binding, owner, upload, index, hash, bytes)
    }

    fn complete(
        &self,
        binding: SyncBinding,
        owner: DeviceId,
        upload: &str,
    ) -> Result<
        echo_sync::resource_transfer::TransferProgress,
        echo_sync::resource_transfer::TransferError,
    > {
        self.cache.complete(binding, owner, upload)
    }

    fn verify_complete(
        &self,
        binding: SyncBinding,
        resource: &ResourceRefDto,
    ) -> Result<(), echo_sync::resource_transfer::TransferError> {
        match self.cache.verify_complete(binding, resource) {
            Ok(()) => Ok(()),
            Err(error) => match self.verify_library_resource(binding, resource)? {
                Some(true) => Ok(()),
                Some(false) => Err(echo_sync::resource_transfer::TransferError::Conflict),
                None => Err(error),
            },
        }
    }

    fn download(
        &self,
        binding: SyncBinding,
        hash: &str,
        offset: u64,
        length: usize,
    ) -> Result<
        echo_sync::resource_transfer::DownloadChunk,
        echo_sync::resource_transfer::TransferError,
    > {
        use std::io::{Read, Seek, SeekFrom};
        if binding != self.binding {
            return self.cache.download(binding, hash, offset, length);
        }
        let manifest = self
            .database
            .local_library_resource_manifests(self.library.id())
            .map_err(|_| storage_error("read active library resource manifests"))?
            .into_iter()
            .find(|manifest| manifest.digest.to_hex() == hash);
        let Some(manifest) = manifest else {
            return self.cache.download(binding, hash, offset, length);
        };
        let resource = ResourceRefDto::from(&manifest);
        let Some(true) = self.verify_library_resource(binding, &resource)? else {
            return self.cache.download(binding, hash, offset, length);
        };
        let end = offset
            .checked_add(length as u64)
            .ok_or(echo_sync::resource_transfer::TransferError::Invalid)?;
        if end > manifest.length || length == 0 {
            return Err(echo_sync::resource_transfer::TransferError::Invalid);
        }
        let mut cache = self
            .verified_files
            .lock()
            .map_err(|_| echo_sync::resource_transfer::TransferError::Conflict)?;
        cache.clock = cache.clock.wrapping_add(1);
        let tick = cache.clock;
        let entry = cache
            .files
            .get_mut(hash)
            .ok_or(echo_sync::resource_transfer::TransferError::Unknown)?;
        if entry.resource != resource {
            return Err(echo_sync::resource_transfer::TransferError::Unknown);
        }
        entry.last_used = tick;
        let file = entry.file.clone();
        drop(cache);
        let mut file = file
            .lock()
            .map_err(|_| echo_sync::resource_transfer::TransferError::Conflict)?;
        file.seek(SeekFrom::Start(offset))?;
        let mut bytes = vec![0; length];
        file.read_exact(&mut bytes)?;
        Ok(echo_sync::resource_transfer::DownloadChunk {
            chunk_hash: blake3::hash(&bytes).to_hex().to_string(),
            bytes,
            total_length: manifest.length,
            content_hash: hash.to_owned(),
        })
    }
}

fn storage_error(action: &'static str) -> echo_sync::resource_transfer::TransferError {
    echo_sync::resource_transfer::TransferError::Storage(std::io::Error::other(action))
}
