use std::io::Cursor;
use std::path::PathBuf;
use std::sync::Arc;

use super::*;
use crate::application::remote_resources::{PublishRemoteResources, RemoteResourcePublishDeps};
use crate::infrastructure::filesystem::{
    Blake3ContentHasher, RootConstrainedFileSystem, RootRegistry,
};

struct Fixture {
    _directory: tempfile::TempDir,
    database: Arc<SqliteDatabase>,
    root_path: PathBuf,
    root: LibraryRootId,
    object: ObjectKey,
    publisher: PublishRemoteResources,
    initial: SharedResourceManifest,
}

impl Fixture {
    fn new() -> Self {
        let (directory, database, root, object, initial) = fixture();
        let root_path = directory.path().join("library");
        std::fs::create_dir(&root_path).unwrap();
        let registry = RootRegistry::new();
        registry.register(root, &root_path);
        let database = Arc::new(database);
        let publisher = PublishRemoteResources::new(RemoteResourcePublishDeps {
            fs: Arc::new(RootConstrainedFileSystem::new(registry.clone())),
            hasher: Arc::new(Blake3ContentHasher::new(registry)),
            journal: database.clone(),
        });
        Self {
            _directory: directory,
            database,
            root_path,
            root,
            object,
            publisher,
            initial,
        }
    }

    fn journal(&self, operation: OperationId) -> RemoteResourcePublication {
        RemoteResourceJournalPort::load(self.database.as_ref(), self.root, operation).unwrap()
    }

    fn prepare(&self, manifest: &SharedResourceManifest) -> OperationId {
        let targets = self
            .database
            .pending_remote_resource_sets(self.root)
            .unwrap();
        let targets = targets
            .into_iter()
            .find(|(object, _)| *object == self.object)
            .unwrap()
            .1;
        assert!(targets.iter().any(|target| target.manifest == *manifest));
        let operation = OperationId::new();
        self.publisher
            .prepare(self.root, operation, self.object, targets)
            .unwrap();
        operation
    }

    fn publish_initial(&self) -> (OperationId, PathBuf) {
        let operation = self.prepare(&self.initial);
        self.publisher
            .stage(self.root, operation, 0, &mut Cursor::new(b"initial audio"))
            .unwrap();
        assert!(
            self.publisher
                .resume(self.root, operation)
                .unwrap()
                .media_ready
        );
        let path = self
            .database
            .local_remote_resource_path(self.root, self.object, &self.initial)
            .unwrap()
            .unwrap();
        (operation, self.root_path.join(path.normalized()))
    }
}

#[test]
fn external_old_file_changes_isolate_replacement_without_overwrite_or_association() {
    let fixture = Fixture::new();
    let (_, old_path) = fixture.publish_initial();
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
    std::fs::write(&old_path, b"user changed this file").unwrap();

    let error = fixture
        .publisher
        .resume(fixture.root, operation)
        .unwrap_err();
    assert_eq!(error.code(), "conflict");
    assert_eq!(std::fs::read(&old_path).unwrap(), b"user changed this file");
    assert!(fixture
        .database
        .local_remote_resource_path(fixture.root, fixture.object, &replacement)
        .unwrap()
        .is_none());
    let journal = fixture.journal(operation);
    assert!(!journal.completed);
    assert_eq!(
        std::fs::read(fixture.root_path.join(&journal.items[0].local_path)).unwrap(),
        b"replacement"
    );
    assert!(!fixture
        .database
        .remote_resource_media_ready(fixture.root, fixture.object)
        .unwrap());
}

#[test]
fn foreign_destination_and_failed_full_hash_keep_existing_bytes_and_retryable_intent() {
    let fixture = Fixture::new();
    let operation = fixture.prepare(&fixture.initial);
    let error = fixture
        .publisher
        .stage(fixture.root, operation, 0, &mut Cursor::new(b"wrong bytes"))
        .unwrap_err();
    assert_eq!(error.code(), "conflict");
    let journal = fixture.journal(operation);
    assert_eq!(journal.items[0].stage, RemoteResourceStage::Planned);
    fixture
        .publisher
        .stage(
            fixture.root,
            operation,
            0,
            &mut Cursor::new(b"initial audio"),
        )
        .unwrap();
    let target = fixture.root_path.join(&journal.items[0].local_path);
    std::fs::create_dir_all(target.parent().unwrap()).unwrap();
    std::fs::write(&target, b"foreign audio").unwrap();
    assert_eq!(
        fixture
            .publisher
            .resume(fixture.root, operation)
            .unwrap_err()
            .code(),
        "conflict"
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"foreign audio");
    assert!(!fixture.journal(operation).completed);
}

#[test]
fn completed_publication_revalidates_files_changed_or_removed_externally() {
    let fixture = Fixture::new();
    let (operation, path) = fixture.publish_initial();
    std::fs::write(&path, b"external edit").unwrap();
    assert_eq!(
        fixture
            .publisher
            .resume(fixture.root, operation)
            .unwrap_err()
            .code(),
        "conflict"
    );
    std::fs::remove_file(&path).unwrap();
    assert_eq!(
        fixture
            .publisher
            .resume(fixture.root, operation)
            .unwrap_err()
            .code(),
        "conflict"
    );
}

#[test]
fn case_and_unicode_equivalent_resource_destinations_cannot_claim_two_files() {
    let fixture = Fixture::new();
    for paths in [
        ["media/Track.flac", "media/track.flac"],
        ["media/Caf\u{e9}.flac", "media/Cafe\u{301}.flac"],
    ] {
        let operation = OperationId::new();
        let resources = paths
            .iter()
            .map(|path| RemoteResourceTarget {
                manifest: manifest(b"audio", path),
                expected_old_digest: None,
            })
            .collect();
        assert_eq!(
            fixture
                .publisher
                .prepare(fixture.root, operation, fixture.object, resources)
                .unwrap_err()
                .code(),
            "conflict"
        );
        assert!(fixture
            .database
            .incomplete(fixture.root)
            .unwrap()
            .is_empty());
    }
}

#[test]
fn foreign_case_or_unicode_spelling_is_never_published_as_a_second_file() {
    for (requested, occupied) in [
        ("media/track.flac", "media/TRACK.flac"),
        ("media/Caf\u{e9}.flac", "media/Cafe\u{301}.flac"),
    ] {
        let fixture = Fixture::new();
        let resource = manifest(b"initial audio", requested);
        fixture
            .database
            .set_local_sync_resources(fixture.object, vec![resource.clone()])
            .unwrap();
        let operation = fixture.prepare(&resource);
        fixture
            .publisher
            .stage(
                fixture.root,
                operation,
                0,
                &mut Cursor::new(b"initial audio"),
            )
            .unwrap();
        let journal = fixture.journal(operation);
        let target = fixture.root_path.join(&journal.items[0].local_path);
        let occupied = target
            .parent()
            .unwrap()
            .join(std::path::Path::new(occupied).file_name().unwrap());
        std::fs::create_dir_all(occupied.parent().unwrap()).unwrap();
        std::fs::write(&occupied, b"foreign bytes").unwrap();
        assert_eq!(
            fixture
                .publisher
                .resume(fixture.root, operation)
                .unwrap_err()
                .code(),
            "conflict"
        );
        assert_eq!(std::fs::read(&occupied).unwrap(), b"foreign bytes");
        assert_eq!(
            std::fs::read_dir(occupied.parent().unwrap())
                .unwrap()
                .count(),
            1
        );
        assert!(!fixture.journal(operation).completed);
    }
}

#[cfg(unix)]
#[test]
fn readonly_staging_root_fails_and_preserves_completed_audio() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new();
    let (_, old_path) = fixture.publish_initial();
    let replacement = manifest(b"replacement", "media/track.flac");
    fixture
        .database
        .set_local_sync_resources(fixture.object, vec![replacement.clone()])
        .unwrap();
    let operation = fixture.prepare(&replacement);
    let staging = fixture.root_path.join("echo/tmp");
    let original = std::fs::metadata(&staging).unwrap().permissions();
    std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o555)).unwrap();
    let result =
        fixture
            .publisher
            .stage(fixture.root, operation, 0, &mut Cursor::new(b"replacement"));
    std::fs::set_permissions(&staging, original).unwrap();
    assert_eq!(result.unwrap_err().code(), "io");
    assert_eq!(std::fs::read(old_path).unwrap(), b"initial audio");
    assert_eq!(
        fixture.journal(operation).items[0].stage,
        RemoteResourceStage::Planned
    );
    fixture
        .publisher
        .stage(fixture.root, operation, 0, &mut Cursor::new(b"replacement"))
        .unwrap();
    assert!(
        fixture
            .publisher
            .resume(fixture.root, operation)
            .unwrap()
            .media_ready
    );
}
