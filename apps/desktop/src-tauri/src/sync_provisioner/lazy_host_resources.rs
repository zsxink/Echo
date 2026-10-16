//! Materialize host-local shared resources only when a peer requests their bytes.

use super::*;
use std::fs::File;

pub(super) struct LazyHostResources {
    database: Arc<SqliteDatabase>,
    library: LibraryRoot,
    binding: SyncBinding,
    owner: DeviceId,
    cache: Arc<FileResourceStore>,
}

impl LazyHostResources {
    pub(super) fn new(
        database: Arc<SqliteDatabase>,
        library: LibraryRoot,
        binding: SyncBinding,
        cache: Arc<FileResourceStore>,
    ) -> Self {
        let owner = DeviceId::from_uuid(database.current_device_id().as_uuid());
        Self {
            database,
            library,
            binding,
            owner,
            cache,
        }
    }

    fn materialize(&self, hash: &str) -> Result<(), echo_sync::resource_transfer::TransferError> {
        let manifests = self
            .database
            .local_sync_resource_manifests(self.library.id())
            .map_err(|_| storage_error("read local resource manifests"))?;
        let manifest = manifests
            .into_iter()
            .find(|manifest| manifest.digest.to_hex() == hash)
            .ok_or(echo_sync::resource_transfer::TransferError::Unknown)?;
        let source = super::authorized_source(
            self.library.absolute_path(),
            manifest.relative_path.as_str(),
        )
        .map_err(|_| echo_sync::resource_transfer::TransferError::Invalid)?
        .ok_or(echo_sync::resource_transfer::TransferError::Unknown)?;
        let mut file = File::open(source)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() || metadata.len() != manifest.length {
            return Err(echo_sync::resource_transfer::TransferError::Conflict);
        }
        let request = PrepareResource {
            binding: self.binding,
            resource: ResourceRefDto::from(&manifest),
        };
        self.cache.import_reader(self.owner, &request, &mut file)?;
        Ok(())
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
        self.cache.verify_complete(binding, resource)
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
        match self.cache.download(binding, hash, offset, length) {
            Ok(chunk) => Ok(chunk),
            Err(echo_sync::resource_transfer::TransferError::Unknown) => {
                if binding != self.binding {
                    return Err(echo_sync::resource_transfer::TransferError::Unknown);
                }
                self.materialize(hash)?;
                self.cache.download(binding, hash, offset, length)
            }
            Err(error) => Err(error),
        }
    }
}

fn storage_error(action: &'static str) -> echo_sync::resource_transfer::TransferError {
    echo_sync::resource_transfer::TransferError::Storage(std::io::Error::other(action))
}
