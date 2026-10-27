//! Real directory commits keep physical resource locations after SQLite is lost.
use super::super::*;
use crate::application::ports::{ControlPlanePort, LibraryProjectionPort};
use crate::application::remote_resources::{PublishRemoteResources, RemoteResourcePublishDeps};
use crate::application::testkit::durability_ports::RefusingAuthority;
use crate::domain::library::durability::{
    ChangeOrigin, DurableStateChange, LibraryBatch, StateKind, StateTarget,
};
use crate::domain::library::{LibraryId as DirectoryLibraryId, LibraryManifest};
use crate::infrastructure::filesystem::{
    Blake3ContentHasher, RootConstrainedFileSystem, RootControlPlane, RootRegistry,
};
use std::{io::Cursor, sync::Arc};

#[path = "sync_resource_publication_scan.rs"]
mod scan;

struct DirectoryFixture {
    directory: tempfile::TempDir,
    database: Arc<SqliteDatabase>,
    root: LibraryRootId,
    object: ObjectKey,
    initial: SharedResourceManifest,
    plane: Arc<RootControlPlane>,
    publisher: PublishRemoteResources,
    files: RootConstrainedFileSystem,
}
impl DirectoryFixture {
    fn new() -> Self {
        let (directory, database, root, object, initial) = fixture();
        let path = directory.path().join("library");
        std::fs::create_dir_all(&path).unwrap();
        let registry = RootRegistry::new();
        registry.register(root, &path);
        let plane = Arc::new(RootControlPlane::new(registry.clone()));
        plane
            .write_manifest(
                root,
                &LibraryManifest::new(
                    DirectoryLibraryId::from_uuid(uuid::Uuid::new_v4()),
                    "resource location",
                ),
            )
            .unwrap();
        database.configure_library_control(plane.clone());
        let database = Arc::new(database);
        let files = RootConstrainedFileSystem::new(registry.clone());
        let publisher = PublishRemoteResources::new(RemoteResourcePublishDeps {
            fs: Arc::new(files.clone()),
            hasher: Arc::new(Blake3ContentHasher::new(registry)),
            journal: database.clone(),
        });
        Self {
            directory,
            database,
            root,
            object,
            initial,
            plane,
            publisher,
            files,
        }
    }
    fn path(&self) -> std::path::PathBuf {
        self.directory.path().join("library")
    }
    fn prepare(&self) -> OperationId {
        let targets = self
            .database
            .pending_remote_resource_sets(self.root)
            .unwrap()
            .into_iter()
            .find(|(object, _)| *object == self.object)
            .unwrap()
            .1;
        let operation = OperationId::new();
        self.publisher
            .prepare(self.root, operation, self.object, targets)
            .unwrap();
        operation
    }
    fn publish(&self, bytes: &[u8]) -> OperationId {
        let operation = self.prepare();
        self.publisher
            .stage(self.root, operation, 0, &mut Cursor::new(bytes))
            .unwrap();
        assert!(
            self.publisher
                .resume(self.root, operation)
                .unwrap()
                .media_ready
        );
        operation
    }
    fn publish_files_without_finishing(&self, bytes: &[u8]) -> RemoteResourcePublication {
        let operation = self.prepare();
        self.publisher
            .stage(self.root, operation, 0, &mut Cursor::new(bytes))
            .unwrap();
        let mut publication =
            RemoteResourceJournalPort::load(self.database.as_ref(), self.root, operation).unwrap();
        let staged = crate::application::ports::StagedResource::new(operation, "0").unwrap();
        crate::application::ports::filesystem::LegacyLibraryFileSystem::publish(
            &self.files,
            self.root,
            &staged,
            &RelativeMediaPath::new(&publication.items[0].local_path).unwrap(),
        )
        .unwrap();
        publication.items[0].stage = RemoteResourceStage::Published;
        self.database.save(&publication).unwrap();
        publication
    }
}

#[test]
fn library_durability_resource_publication_rebuilds_media_and_managed_locations_offline() {
    for managed in [false, true] {
        let f = DirectoryFixture::new();
        if managed {
            std::fs::create_dir_all(f.path().join("media")).unwrap();
            std::fs::write(f.path().join("media/track.flac"), b"foreign existing music").unwrap();
        }
        let operation = f.publish(b"initial audio");
        let local = f
            .database
            .local_remote_resource_path(f.root, f.object, &f.initial)
            .unwrap()
            .unwrap();
        assert_eq!(local.normalized().starts_with("echo/media/"), managed);
        let ObjectKey::Song(song) = f.object else {
            unreachable!()
        };
        let library = f.plane.read_manifest(f.root).unwrap().unwrap().library_id;
        // Cross a real compaction window, leaving the latest pointer in coverage.
        for index in 0..33 {
            let mut batch = LibraryBatch::new(
                uuid::Uuid::new_v4(),
                library,
                ChangeOrigin::LocalEdit,
                vec![],
            );
            batch.state.push(DurableStateChange::new(
                StateKind::Claim,
                uuid::Uuid::new_v4(),
                serde_json::json!({"test":index}),
            ));
            f.plane
                .durability()
                .unwrap()
                .commit(f.root, &batch)
                .unwrap();
        }
        let replay = f.plane.durability().unwrap().recover(f.root).unwrap();
        let pointer = replay.states().into_iter().find(|state| matches!(state.key.as_ref().map(|key| &key.target), Some(StateTarget::ResourceLocation { object }) if *object == f.object)).unwrap();
        pointer.validate().unwrap();
        assert_eq!(pointer.payload["operation"], operation.to_string());
        let fresh_dir = tempfile::tempdir().unwrap();
        let fresh = SqliteDatabase::open(fresh_dir.path().join("fresh.sqlite")).unwrap();
        let relocated_root = LibraryRootId::new();
        LibraryRepository::upsert(
            &fresh,
            &LibraryRoot::new(relocated_root, f.path(), true, true),
        )
        .unwrap();
        for _ in 0..2 {
            fresh
                .project_library_commits(relocated_root, &replay)
                .unwrap();
        }
        let restored = SongRepository::by_id(&fresh, song).unwrap().unwrap();
        assert_eq!(restored.id(), song);
        assert_eq!(restored.path(), &local);
        assert_eq!(restored.availability(), SongAvailability::Missing);
        assert_eq!(
            SongRepository::published_file_paths(&fresh, relocated_root).unwrap(),
            vec![(
                local.clone(),
                RelativeMediaPath::new("media/track.flac").unwrap()
            )]
        );
        assert_eq!(
            SongRepository::published_logical_path(&fresh, relocated_root, &local)
                .unwrap()
                .unwrap()
                .normalized(),
            "media/track.flac"
        );
        fresh.with_reader(|c| { let counts:(u64,u64,u64)=c.query_row("SELECT (SELECT COUNT(*) FROM sync_binding),(SELECT COUNT(*) FROM sync_mutations),(SELECT COUNT(*) FROM sync_outbox)",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(storage)?; assert_eq!(counts,(0,0,0)); Ok(()) }).unwrap();
        assert_eq!(
            std::fs::read(f.path().join(local.normalized())).unwrap(),
            b"initial audio"
        );
    }
}

#[test]
fn library_durability_resource_completion_refusal_keeps_the_old_association_and_retries() {
    let f = DirectoryFixture::new();
    f.publish(b"initial audio");
    let replacement = manifest(b"replacement", "media/track.flac");
    f.database
        .set_local_sync_resources(f.object, vec![replacement.clone()])
        .unwrap();
    let publication = f.publish_files_without_finishing(b"replacement");
    let refused = Arc::new(RefusingAuthority::new(f.plane.clone()));
    refused.set_refused(true);
    f.database.configure_library_control(refused.clone());
    assert!(f.database.finish(&publication).is_err());
    assert!(
        f.database
            .local_remote_resource_path(f.root, f.object, &replacement)
            .unwrap()
            .is_none()
    );
    assert!(
        f.database
            .local_remote_resource_path(f.root, f.object, &f.initial)
            .unwrap()
            .is_some()
    );
    assert!(
        !RemoteResourceJournalPort::load(f.database.as_ref(), f.root, publication.operation)
            .unwrap()
            .completed
    );
    refused.set_refused(false);
    f.database.finish(&publication).unwrap();
    assert!(
        f.database
            .local_remote_resource_path(f.root, f.object, &replacement)
            .unwrap()
            .is_some()
    );
    assert!(
        f.database
            .local_remote_resource_path(f.root, f.object, &f.initial)
            .unwrap()
            .is_none()
    );
}

#[test]
fn library_durability_resource_completion_sql_failure_retains_the_same_directory_operation() {
    let f = DirectoryFixture::new();
    let publication = f.publish_files_without_finishing(b"initial audio");
    f.database.writer.run(|c| c.execute_batch("CREATE TABLE fail_resource_finish(parent TEXT REFERENCES playlists(uuid) DEFERRABLE INITIALLY DEFERRED); CREATE TRIGGER fail_resource_finish AFTER UPDATE OF completed ON sync_remote_publications WHEN NEW.completed=1 BEGIN INSERT INTO fail_resource_finish VALUES('missing'); END;").map_err(storage)).unwrap();
    let error = f.database.finish(&publication).unwrap_err();
    assert_eq!(error.code(), "library_saved_pending_projection");
    assert!(
        f.database
            .local_remote_resource_path(f.root, f.object, &f.initial)
            .unwrap()
            .is_none()
    );
    let before = f.plane.durability().unwrap().recover(f.root).unwrap();
    let head = before.head().unwrap().0;
    assert!(
        before
            .states()
            .any(|state| state.payload["schema"] == "echo.library.resource-location.v1")
    );
    f.database
        .writer
        .run(|c| {
            c.execute_batch("DROP TRIGGER fail_resource_finish; DROP TABLE fail_resource_finish;")
                .map_err(storage)
        })
        .unwrap();
    f.database.finish(&publication).unwrap();
    assert_eq!(
        f.plane
            .durability()
            .unwrap()
            .recover(f.root)
            .unwrap()
            .head()
            .unwrap()
            .0,
        head
    );
    assert!(
        f.publisher
            .resume(f.root, publication.operation)
            .unwrap()
            .media_ready
    );
}

#[test]
fn library_durability_resource_location_uses_latest_set_when_only_the_sidecar_changes() {
    let f = DirectoryFixture::new();
    let lyrics = |bytes: &[u8]| {
        SharedResourceManifest::new(
            crate::domain::sync::SharedResourceKind::LyricsSidecar,
            ResourceDigest::from_bytes(*blake3::hash(bytes).as_bytes()),
            bytes.len() as u64,
            "text/plain".to_owned(),
            "media/track.lrc",
        )
        .unwrap()
    };
    let first = lyrics(b"first lyrics");
    f.database
        .set_local_sync_resources(f.object, vec![f.initial.clone(), first])
        .unwrap();
    let publish_set = |sidecar: &[u8]| {
        let operation = f.prepare();
        f.publisher
            .stage(f.root, operation, 0, &mut Cursor::new(b"initial audio"))
            .unwrap();
        f.publisher
            .stage(f.root, operation, 1, &mut Cursor::new(sidecar))
            .unwrap();
        assert!(f.publisher.resume(f.root, operation).unwrap().media_ready);
        operation
    };
    publish_set(b"first lyrics");
    let second = lyrics(b"second lyrics");
    f.database
        .set_local_sync_resources(f.object, vec![f.initial.clone(), second.clone()])
        .unwrap();
    let latest = publish_set(b"second lyrics");
    let expected = f
        .database
        .local_remote_resource_path(f.root, f.object, &second)
        .unwrap()
        .unwrap();
    assert!(expected.normalized().starts_with("echo/media/"));
    let replay = f.plane.durability().unwrap().recover(f.root).unwrap();
    let fresh_dir = tempfile::tempdir().unwrap();
    let fresh = SqliteDatabase::open(fresh_dir.path().join("fresh.sqlite")).unwrap();
    LibraryRepository::upsert(&fresh, &LibraryRoot::new(f.root, f.path(), true, true)).unwrap();
    fresh.project_library_commits(f.root, &replay).unwrap();
    let (path, operation):(String,String) = fresh.with_reader(|c| c.query_row(
        "SELECT local_relative_path,operation_uuid FROM sync_remote_resource_locations WHERE local_root_uuid=?1 AND library_relative_path='media/track.lrc'",
        [f.root.to_string()],|r|Ok((r.get(0)?,r.get(1)?))).map_err(storage)).unwrap();
    assert_eq!(path, expected.normalized());
    assert_eq!(operation, latest.to_string());
    assert_eq!(
        std::fs::read(f.path().join("media/track.lrc")).unwrap(),
        b"first lyrics"
    );
    assert_eq!(
        std::fs::read(f.path().join(path)).unwrap(),
        b"second lyrics"
    );
}

#[test]
fn library_durability_resource_location_rejects_wrong_owner_scope_and_incomplete_resources() {
    let f = DirectoryFixture::new();
    std::fs::create_dir_all(f.path().join("media")).unwrap();
    std::fs::write(f.path().join("media/track.flac"), b"foreign file").unwrap();
    f.publish(b"initial audio");
    let replay = f.plane.durability().unwrap().recover(f.root).unwrap();
    let state = replay
        .states()
        .find(|state| state.payload["schema"] == "echo.library.resource-location.v1")
        .unwrap()
        .clone();
    let library = f.plane.read_manifest(f.root).unwrap().unwrap().library_id;
    let mut no_resources =
        LibraryBatch::new(uuid::Uuid::new_v4(), library, ChangeOrigin::Remote, vec![]);
    no_resources.state.push(state.clone());
    assert!(no_resources.validate().is_err());
    let mut wrong_scope = state.key.clone().unwrap();
    wrong_scope.namespace = crate::domain::library::durability::StateNamespace::Library {
        library_id: DirectoryLibraryId::from_uuid(uuid::Uuid::new_v4()),
    };
    let foreign = DurableStateChange::scoped(wrong_scope, state.payload.clone()).unwrap();
    let mut batch = replay
        .commits
        .iter()
        .find(|commit| commit.batch.state.iter().any(|other| other.id == state.id))
        .unwrap()
        .batch
        .clone();
    batch.state.retain(|other| other.id != state.id);
    batch.state.push(foreign);
    assert!(batch.validate().is_err());
    for wrong_path in [
        format!(
            "echo/media/song/{}/{}/media/track.flac",
            uuid::Uuid::new_v4(),
            "a".repeat(64)
        ),
        format!(
            "echo/media/song/{}/{}/media/track.flac",
            match f.object {
                ObjectKey::Song(id) => id.as_uuid(),
                _ => unreachable!(),
            },
            "b".repeat(64)
        ),
    ] {
        let mut payload = state.payload.clone();
        payload["publication"]["items"][0]["local_path"] = serde_json::json!(wrong_path);
        payload["original_body"] =
            serde_json::json!(serde_json::to_string(&payload["publication"]).unwrap());
        assert!(DurableStateChange::scoped(state.key.clone().unwrap(), payload).is_err());
    }
}
