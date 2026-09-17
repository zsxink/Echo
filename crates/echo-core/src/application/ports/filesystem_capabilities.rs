//! Focused filesystem capabilities consumed by application use cases.

use std::io::Read;

use crate::domain::ids::{LibraryRootId, OperationId, RelativeMediaPath};
use crate::error::Error;

use super::filesystem::{FileMeta, LegacyLibraryFileSystem, StagedCopy, StagedResource};

/// Enumerates and reads root-constrained library files.
pub trait LibraryFileReader: Send + Sync {
    fn enumerate(&self, root: LibraryRootId) -> Result<Vec<RelativeMediaPath>, Error>;
    fn file_meta(&self, root: LibraryRootId, path: &RelativeMediaPath) -> Result<FileMeta, Error>;
    fn read_head(
        &self,
        root: LibraryRootId,
        path: &RelativeMediaPath,
        limit: u64,
    ) -> Result<Vec<u8>, Error>;
}

impl<T: LegacyLibraryFileSystem> LibraryFileReader for T {
    fn enumerate(&self, root: LibraryRootId) -> Result<Vec<RelativeMediaPath>, Error> {
        LegacyLibraryFileSystem::enumerate(self, root)
    }

    fn file_meta(&self, root: LibraryRootId, path: &RelativeMediaPath) -> Result<FileMeta, Error> {
        LegacyLibraryFileSystem::file_meta(self, root, path)
    }

    fn read_head(
        &self,
        root: LibraryRootId,
        path: &RelativeMediaPath,
        limit: u64,
    ) -> Result<Vec<u8>, Error> {
        LegacyLibraryFileSystem::read_head(self, root, path, limit)
    }
}

/// Publishes already-owned staging content without replacing foreign files.
pub trait LibraryFilePublisher: Send + Sync {
    fn publish(
        &self,
        root: LibraryRootId,
        staged: &StagedResource,
        target: &RelativeMediaPath,
    ) -> Result<(), Error>;
}

impl<T: LegacyLibraryFileSystem> LibraryFilePublisher for T {
    fn publish(
        &self,
        root: LibraryRootId,
        staged: &StagedResource,
        target: &RelativeMediaPath,
    ) -> Result<(), Error> {
        LegacyLibraryFileSystem::publish(self, root, staged, target)
    }
}

/// Creates, reads and discards resources in Echo-owned staging space.
pub trait LibraryStagingFileSystem: Send + Sync {
    fn stage(
        &self,
        root: LibraryRootId,
        staged: &StagedResource,
        content: &[u8],
    ) -> Result<(), Error>;
    fn stage_stream(
        &self,
        root: LibraryRootId,
        staged: &StagedResource,
        content: &mut dyn Read,
    ) -> Result<StagedCopy, Error>;
    fn read_staged(&self, root: LibraryRootId, staged: &StagedResource) -> Result<Vec<u8>, Error>;
    fn discard_staged(&self, root: LibraryRootId, staged: &StagedResource) -> Result<(), Error>;
}

impl<T: LegacyLibraryFileSystem> LibraryStagingFileSystem for T {
    fn stage(
        &self,
        root: LibraryRootId,
        staged: &StagedResource,
        content: &[u8],
    ) -> Result<(), Error> {
        LegacyLibraryFileSystem::stage(self, root, staged, content)
    }

    fn stage_stream(
        &self,
        root: LibraryRootId,
        staged: &StagedResource,
        content: &mut dyn Read,
    ) -> Result<StagedCopy, Error> {
        LegacyLibraryFileSystem::stage_stream(self, root, staged, content)
    }

    fn read_staged(&self, root: LibraryRootId, staged: &StagedResource) -> Result<Vec<u8>, Error> {
        LegacyLibraryFileSystem::read_staged(self, root, staged)
    }

    fn discard_staged(&self, root: LibraryRootId, staged: &StagedResource) -> Result<(), Error> {
        LegacyLibraryFileSystem::discard_staged(self, root, staged)
    }
}

/// Recovery and cleanup operations for persisted staging and published files.
pub trait LibraryFileRecovery: Send + Sync {
    fn discard_staging_path(
        &self,
        root: LibraryRootId,
        staging_path: &RelativeMediaPath,
    ) -> Result<(), Error>;
    fn discard_published(
        &self,
        root: LibraryRootId,
        target: &RelativeMediaPath,
    ) -> Result<(), Error>;
    fn path_exists(&self, root: LibraryRootId, path: &RelativeMediaPath) -> Result<bool, Error>;
    fn publish_from_staging_path(
        &self,
        root: LibraryRootId,
        staging_path: &RelativeMediaPath,
        target: &RelativeMediaPath,
    ) -> Result<(), Error>;
}

impl<T: LegacyLibraryFileSystem> LibraryFileRecovery for T {
    fn discard_staging_path(
        &self,
        root: LibraryRootId,
        staging_path: &RelativeMediaPath,
    ) -> Result<(), Error> {
        LegacyLibraryFileSystem::discard_staging_path(self, root, staging_path)
    }

    fn discard_published(
        &self,
        root: LibraryRootId,
        target: &RelativeMediaPath,
    ) -> Result<(), Error> {
        LegacyLibraryFileSystem::discard_published(self, root, target)
    }

    fn path_exists(&self, root: LibraryRootId, path: &RelativeMediaPath) -> Result<bool, Error> {
        LegacyLibraryFileSystem::path_exists(self, root, path)
    }

    fn publish_from_staging_path(
        &self,
        root: LibraryRootId,
        staging_path: &RelativeMediaPath,
        target: &RelativeMediaPath,
    ) -> Result<(), Error> {
        LegacyLibraryFileSystem::publish_from_staging_path(self, root, staging_path, target)
    }
}

/// Moves files through Echo's controlled per-operation trash area.
pub trait LibraryTrashFileSystem: Send + Sync {
    fn trash_path(
        &self,
        root: LibraryRootId,
        operation: OperationId,
        resource_key: &str,
    ) -> Result<RelativeMediaPath, Error>;
    fn stage_to_trash(
        &self,
        root: LibraryRootId,
        operation: OperationId,
        source: &RelativeMediaPath,
        resource_key: &str,
    ) -> Result<RelativeMediaPath, Error>;
    fn restore_from_trash(
        &self,
        root: LibraryRootId,
        trash: &RelativeMediaPath,
        target: &RelativeMediaPath,
    ) -> Result<(), Error>;
}

impl<T: LegacyLibraryFileSystem> LibraryTrashFileSystem for T {
    fn trash_path(
        &self,
        root: LibraryRootId,
        operation: OperationId,
        resource_key: &str,
    ) -> Result<RelativeMediaPath, Error> {
        LegacyLibraryFileSystem::trash_path(self, root, operation, resource_key)
    }

    fn stage_to_trash(
        &self,
        root: LibraryRootId,
        operation: OperationId,
        source: &RelativeMediaPath,
        resource_key: &str,
    ) -> Result<RelativeMediaPath, Error> {
        LegacyLibraryFileSystem::stage_to_trash(self, root, operation, source, resource_key)
    }

    fn restore_from_trash(
        &self,
        root: LibraryRootId,
        trash: &RelativeMediaPath,
        target: &RelativeMediaPath,
    ) -> Result<(), Error> {
        LegacyLibraryFileSystem::restore_from_trash(self, root, trash, target)
    }
}

/// Checks and establishes the library's write capability.
pub trait LibraryWriteCapability: Send + Sync {
    fn write_capable(&self, root: LibraryRootId) -> Result<bool, Error>;
    fn establish_write_capability(&self, root: LibraryRootId) -> Result<(), Error>;
}

impl<T: LegacyLibraryFileSystem> LibraryWriteCapability for T {
    fn write_capable(&self, root: LibraryRootId) -> Result<bool, Error> {
        LegacyLibraryFileSystem::write_capable(self, root)
    }

    fn establish_write_capability(&self, root: LibraryRootId) -> Result<(), Error> {
        LegacyLibraryFileSystem::establish_write_capability(self, root)
    }
}

/// Stable aggregate entry point retained for existing application imports.
pub trait LibraryFileSystem:
    LibraryFileReader
    + LibraryFilePublisher
    + LibraryStagingFileSystem
    + LibraryFileRecovery
    + LibraryTrashFileSystem
    + LibraryWriteCapability
{
}

impl<T: LegacyLibraryFileSystem> LibraryFileSystem for T {}
