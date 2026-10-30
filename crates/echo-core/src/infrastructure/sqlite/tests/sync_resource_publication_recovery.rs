use super::*;
use crate::application::ports::filesystem::LegacyLibraryFileSystem;
use crate::application::ports::{FileMeta, StagedCopy, StagedResource};
use crate::error::Error;
use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};

struct LockedFileSystem {
    inner: RootConstrainedFileSystem,
    locked: Arc<AtomicBool>,
}

impl LegacyLibraryFileSystem for LockedFileSystem {
    fn enumerate(&self, root: LibraryRootId) -> Result<Vec<RelativeMediaPath>, Error> {
        LegacyLibraryFileSystem::enumerate(&self.inner, root)
    }
    fn is_publication_blocked(&self, error: &Error) -> bool {
        matches!(error, Error::Io { source, .. } if source.kind() == std::io::ErrorKind::WouldBlock)
    }
    fn file_meta(&self, root: LibraryRootId, path: &RelativeMediaPath) -> Result<FileMeta, Error> {
        LegacyLibraryFileSystem::file_meta(&self.inner, root, path)
    }
    fn read_head(
        &self,
        root: LibraryRootId,
        path: &RelativeMediaPath,
        limit: u64,
    ) -> Result<Vec<u8>, Error> {
        LegacyLibraryFileSystem::read_head(&self.inner, root, path, limit)
    }
    fn publish(
        &self,
        root: LibraryRootId,
        staged: &StagedResource,
        target: &RelativeMediaPath,
    ) -> Result<(), Error> {
        if self.locked.load(Ordering::SeqCst) {
            return Err(Error::io(
                "publish",
                std::io::Error::from(std::io::ErrorKind::WouldBlock),
                "locked",
            ));
        }
        LegacyLibraryFileSystem::publish(&self.inner, root, staged, target)
    }
    fn stage(
        &self,
        root: LibraryRootId,
        staged: &StagedResource,
        content: &[u8],
    ) -> Result<(), Error> {
        LegacyLibraryFileSystem::stage(&self.inner, root, staged, content)
    }
    fn stage_stream(
        &self,
        root: LibraryRootId,
        staged: &StagedResource,
        content: &mut dyn Read,
    ) -> Result<StagedCopy, Error> {
        LegacyLibraryFileSystem::stage_stream(&self.inner, root, staged, content)
    }
    fn read_staged(&self, root: LibraryRootId, staged: &StagedResource) -> Result<Vec<u8>, Error> {
        LegacyLibraryFileSystem::read_staged(&self.inner, root, staged)
    }
    fn discard_staged(&self, root: LibraryRootId, staged: &StagedResource) -> Result<(), Error> {
        LegacyLibraryFileSystem::discard_staged(&self.inner, root, staged)
    }
    fn discard_staging_path(
        &self,
        root: LibraryRootId,
        staging_path: &RelativeMediaPath,
    ) -> Result<(), Error> {
        LegacyLibraryFileSystem::discard_staging_path(&self.inner, root, staging_path)
    }
    fn discard_published(
        &self,
        root: LibraryRootId,
        target: &RelativeMediaPath,
    ) -> Result<(), Error> {
        LegacyLibraryFileSystem::discard_published(&self.inner, root, target)
    }
    fn path_exists(&self, root: LibraryRootId, path: &RelativeMediaPath) -> Result<bool, Error> {
        LegacyLibraryFileSystem::path_exists(&self.inner, root, path)
    }
    fn publish_from_staging_path(
        &self,
        root: LibraryRootId,
        staging_path: &RelativeMediaPath,
        target: &RelativeMediaPath,
    ) -> Result<(), Error> {
        if self.locked.load(Ordering::SeqCst) {
            return Err(Error::io(
                "publish",
                std::io::Error::from(std::io::ErrorKind::WouldBlock),
                "locked",
            ));
        }
        LegacyLibraryFileSystem::publish_from_staging_path(&self.inner, root, staging_path, target)
    }
    fn trash_path(
        &self,
        root: LibraryRootId,
        operation: OperationId,
        resource_key: &str,
    ) -> Result<RelativeMediaPath, Error> {
        LegacyLibraryFileSystem::trash_path(&self.inner, root, operation, resource_key)
    }
    fn stage_to_trash(
        &self,
        root: LibraryRootId,
        operation: OperationId,
        source: &RelativeMediaPath,
        resource_key: &str,
    ) -> Result<RelativeMediaPath, Error> {
        LegacyLibraryFileSystem::stage_to_trash(&self.inner, root, operation, source, resource_key)
    }
    fn restore_from_trash(
        &self,
        root: LibraryRootId,
        trash: &RelativeMediaPath,
        target: &RelativeMediaPath,
    ) -> Result<(), Error> {
        LegacyLibraryFileSystem::restore_from_trash(&self.inner, root, trash, target)
    }
    fn write_capable(&self, root: LibraryRootId) -> Result<bool, Error> {
        LegacyLibraryFileSystem::write_capable(&self.inner, root)
    }
    fn establish_write_capability(&self, root: LibraryRootId) -> Result<(), Error> {
        LegacyLibraryFileSystem::establish_write_capability(&self.inner, root)
    }
}

fn publisher_with_lock(fixture: &Fixture, locked: Arc<AtomicBool>) -> PublishRemoteResources {
    let registry = RootRegistry::new();
    registry.register(fixture.root, &fixture.root_path);
    PublishRemoteResources::new(RemoteResourcePublishDeps {
        fs: Arc::new(LockedFileSystem {
            inner: RootConstrainedFileSystem::new(registry.clone()),
            locked,
        }),
        hasher: Arc::new(Blake3ContentHasher::new(registry)),
        journal: fixture.database.clone(),
    })
}

#[test]
fn occupied_publication_preserves_playback_and_recovers_after_adapter_restart() {
    let fixture = Fixture::new();
    let (_, old_path) = fixture.publish_initial();
    let mut playing = std::fs::File::open(&old_path).unwrap();
    let binding = fixture
        .database
        .current_sync_configuration()
        .unwrap()
        .unwrap()
        .binding;
    let checkpoint = fixture
        .database
        .latest_published_sync_checkpoint(binding)
        .unwrap();
    let replacement = manifest(b"replacement", "media/track.flac");
    fixture
        .database
        .set_local_sync_resources(fixture.object, vec![replacement.clone()])
        .unwrap();
    let operation = fixture.prepare(&replacement);
    fixture
        .publisher
        .stage(fixture.root, operation, 0, &mut Cursor::new(b"replacement"))
        .unwrap();
    let locked = Arc::new(AtomicBool::new(true));
    let publisher = publisher_with_lock(&fixture, locked.clone());
    let pending = publisher.resume(fixture.root, operation).unwrap();
    assert!(!pending.media_ready);
    assert_eq!(pending.remaining_resources, 1);
    assert!(!fixture.journal(operation).completed);
    assert_eq!(
        fixture.journal(operation).items[0].stage,
        RemoteResourceStage::Validated
    );
    assert!(!fixture
        .database
        .remote_resource_media_ready(fixture.root, fixture.object)
        .unwrap());
    assert_eq!(
        fixture
            .database
            .latest_published_sync_checkpoint(binding)
            .unwrap(),
        checkpoint
    );
    let mut audible = Vec::new();
    playing.read_to_end(&mut audible).unwrap();
    assert_eq!(audible, b"initial audio");
    assert_eq!(std::fs::read(&old_path).unwrap(), audible);
    drop(publisher);
    locked.store(false, Ordering::SeqCst);
    // This fresh filesystem adapter has no in-memory staged handles: only
    // the durable journal path allows the publication to recover.
    let restarted = publisher_with_lock(&fixture, locked);
    let results = restarted.recover(fixture.root).unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].0, operation);
    assert!(results[0].1.as_ref().unwrap().media_ready);
    assert!(fixture.journal(operation).completed);
    assert!(fixture
        .database
        .remote_resource_media_ready(fixture.root, fixture.object)
        .unwrap());
    assert_eq!(std::fs::read(old_path).unwrap(), b"initial audio");
}

#[test]
fn completed_publication_reclaims_only_verified_redundant_journal_staging() {
    let fixture = Fixture::new();
    let (operation, _) = fixture.publish_initial();
    let journal = fixture.journal(operation);
    let staging = fixture.root_path.join(&journal.items[0].staging_path);
    std::fs::write(&staging, b"initial audio").unwrap();
    let unknown_operation = staging
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join(OperationId::new().to_string());
    std::fs::create_dir_all(&unknown_operation).unwrap();
    let referenced_conflict = unknown_operation.join("conflict");
    std::fs::write(&referenced_conflict, b"conflict candidate").unwrap();
    assert!(
        fixture
            .publisher
            .resume(fixture.root, operation)
            .unwrap()
            .media_ready
    );
    assert!(!staging.exists());
    assert_eq!(
        std::fs::read(&referenced_conflict).unwrap(),
        b"conflict candidate"
    );
    // Even a journal-owned path may have been externally replaced; hash and
    // length must match before cleanup can discard anything.
    std::fs::write(&staging, b"external candidate").unwrap();
    assert!(
        fixture
            .publisher
            .resume(fixture.root, operation)
            .unwrap()
            .media_ready
    );
    assert_eq!(std::fs::read(staging).unwrap(), b"external candidate");
}

#[test]
fn native_file_lock_classification_does_not_hide_unrelated_io_errors() {
    let registry = RootRegistry::new();
    let fs = RootConstrainedFileSystem::new(registry);
    for native_code in [32, 33] {
        let error = Error::io(
            "publish",
            std::io::Error::from_raw_os_error(native_code),
            "target",
        );
        assert_eq!(
            LegacyLibraryFileSystem::is_publication_blocked(&fs, &error),
            std::env::consts::OS == "windows"
        );
    }
    for kind in [
        std::io::ErrorKind::PermissionDenied,
        std::io::ErrorKind::StorageFull,
    ] {
        let error = Error::io("publish", std::io::Error::from(kind), "target");
        assert!(!LegacyLibraryFileSystem::is_publication_blocked(
            &fs, &error
        ));
    }
}
