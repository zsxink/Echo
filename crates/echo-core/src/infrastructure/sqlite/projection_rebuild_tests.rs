use super::*;
use crate::application::ports::{
    ControlPlanePort, LibraryProjectionPort, LibraryRepository, SongRepository,
};
use crate::application::testkit::library_durability::Issue72Fixture;
use crate::domain::entities::LibraryRoot;
use crate::domain::library::durability::{ChangeOrigin, LibraryBatch, LibraryCommit};
use crate::infrastructure::sqlite::SqliteDatabase;

fn replay(library: LibraryId, records: Vec<PortableRecord>) -> LibraryReplay {
    let batch = LibraryBatch::new(
        uuid::Uuid::new_v4(),
        library,
        ChangeOrigin::Recovery,
        records
            .into_iter()
            .map(|record| PortableRecordEnvelope::new(record, None).unwrap())
            .collect(),
    );
    let digest = LibraryCommit::calculate_digest(&batch, 1, None).unwrap();
    LibraryReplay {
        checkpoint: None,
        commits: vec![LibraryCommit {
            batch,
            library_commit_seq: 1,
            predecessor_digest: None,
            digest,
        }],
    }
}

#[test]
fn library_durability_candidate_rebuild_retains_orphans_and_preserves_active_projection() {
    let directory = tempfile::tempdir().unwrap();
    let database = SqliteDatabase::open(directory.path().join("active.sqlite")).unwrap();
    let root = LibraryRootId::new();
    LibraryRepository::upsert(
        &database,
        &LibraryRoot::new(root, directory.path().join("library"), true, true),
    )
    .unwrap();
    let library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let active = replay(library, Issue72Fixture::with_database_evidence().database);
    database.project_library_commits(root, &active).unwrap();
    let before = database.read_recovery_records(root).unwrap();
    let before_status = database.projection_status(root).unwrap();
    let candidate_library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let candidate = replay(candidate_library, Issue72Fixture::orphaned().portable);
    let report = database
        .validate_library_projection(root, candidate_library, &candidate)
        .unwrap();
    assert!(!report.projection.unresolved.is_empty());
    assert_eq!(
        report.retained_records,
        candidate.commits[0].batch.records.len()
    );
    assert_eq!(database.read_recovery_records(root).unwrap(), before);
    assert_eq!(database.projection_status(root).unwrap(), before_status);
    assert!(database
        .validate_library_projection(root, library, &candidate)
        .is_err());
    let mut broken = candidate;
    broken.commits[0].library_commit_seq = 2;
    broken.commits[0].digest =
        LibraryCommit::calculate_digest(&broken.commits[0].batch, 2, None).unwrap();
    assert!(database
        .validate_library_projection(root, candidate_library, &broken)
        .is_err());
    assert_eq!(database.read_recovery_records(root).unwrap(), before);
    assert!(std::fs::read_dir(directory.path())
        .unwrap()
        .all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".projection-candidate-")));
}

#[test]
fn library_durability_orphan_rebuild_projects_valid_subset_and_reports_partial_state() {
    let directory = tempfile::tempdir().unwrap();
    let root = LibraryRootId::new();
    let library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let replay = replay(library, Issue72Fixture::orphaned().portable);
    let connection = project(&directory.path().join("candidate.sqlite"), root, &replay)
        .expect("candidate projection tolerates dangling relations");

    let projected_songs: u64 = connection
        .query_row(
            "SELECT COUNT(*) FROM songs WHERE library_root_uuid=?1",
            [root.to_string()],
            |row| row.get(0),
        )
        .unwrap();
    let retained = durability_projection::read(&connection, root).unwrap();
    let report = durability_projection::status(&connection, root).unwrap();
    assert!(
        projected_songs > 0,
        "valid song records enter the read model"
    );
    assert_eq!(
        retained.records.len(),
        replay.commits[0].batch.records.len()
    );
    assert!(
        !report.unresolved.is_empty(),
        "dangling after-images stay visible as partial recovery"
    );
}

#[test]
fn library_durability_candidate_validation_detects_changed_retained_bytes() {
    let directory = tempfile::tempdir().unwrap();
    let library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let root = LibraryRootId::new();
    let source = replay(library, Issue72Fixture::orphaned().portable);
    let connection = project(&directory.path().join("candidate.sqlite"), root, &source).unwrap();
    let envelope = source.commits[0]
        .batch
        .records
        .iter()
        .find(|envelope| matches!(envelope.record(), PortableRecord::Song(_)))
        .unwrap();
    let PortableRecord::Song(mut song) = envelope.record().clone() else {
        unreachable!()
    };
    song.title = Some("changed".to_owned());
    let changed = PortableRecordEnvelope::new(PortableRecord::Song(song), None).unwrap();
    connection
        .execute(
            "UPDATE library_recovery_records SET envelope_json=?1 WHERE object_key=?2",
            params![
                serde_json::to_string(&changed).unwrap(),
                record_key(&changed).unwrap()
            ],
        )
        .unwrap();
    assert!(verify(&connection, root, library, &source).is_err());
}

#[test]
fn library_durability_rebuild_atomically_replaces_one_root_and_preserves_other_roots() {
    use rusqlite::Connection;

    let directory = tempfile::tempdir().unwrap();
    let database_path = directory.path().join("active.sqlite");
    let database = SqliteDatabase::open(&database_path).unwrap();
    let root = LibraryRootId::new();
    let other_root = LibraryRootId::new();
    for (id, active) in [(root, true), (other_root, false)] {
        LibraryRepository::upsert(
            &database,
            &LibraryRoot::new(
                id,
                directory.path().join(format!("library-{id}")),
                active,
                true,
            ),
        )
        .unwrap();
    }
    let old_library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let old = replay(
        old_library,
        Issue72Fixture::with_database_evidence().database,
    );
    database.project_library_commits(root, &old).unwrap();
    let old_count = SongRepository::all_in_root(&database, root).unwrap().len();

    // A row in another root is a sentinel for cross-root isolation.
    let other_song = uuid::Uuid::new_v4().to_string();
    let connection = Connection::open(&database_path).unwrap();
    connection
        .execute(
            "INSERT INTO songs(uuid,library_root_uuid,relative_path,normalized_relative_path,added_at,created_at,updated_at) VALUES(?1,?2,'keep.flac','keep.flac',1,1,1)",
            rusqlite::params![other_song, other_root.to_string()],
        )
        .unwrap();
    drop(connection);

    let new_library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let replacement = replay(new_library, Issue72Fixture::orphaned().portable);
    let expected_ids: std::collections::BTreeSet<_> = replacement.commits[0]
        .batch
        .records
        .iter()
        .filter_map(|envelope| match envelope.record() {
            PortableRecord::Song(song) => Some(song.song_uuid),
            _ => None,
        })
        .collect();
    let report = database
        .rebuild_library_projection(root, new_library, &replacement)
        .unwrap();
    let new_ids: std::collections::BTreeSet<_> = SongRepository::all_in_root(&database, root)
        .unwrap()
        .into_iter()
        .map(|song| song.id().as_uuid())
        .collect();
    assert!(!new_ids.is_empty());
    assert_eq!(new_ids, expected_ids);
    assert!(old_count > new_ids.len());
    assert_eq!(database.projection_status(root).unwrap(), report.projection);
    assert_eq!(
        SongRepository::all_in_root(&database, other_root)
            .unwrap()
            .len(),
        1
    );
}

#[test]
fn library_durability_rebuild_failure_rolls_back_the_active_projection() {
    use rusqlite::Connection;

    let directory = tempfile::tempdir().unwrap();
    let database_path = directory.path().join("active.sqlite");
    let database = SqliteDatabase::open(&database_path).unwrap();
    let root = LibraryRootId::new();
    LibraryRepository::upsert(
        &database,
        &LibraryRoot::new(root, directory.path().join("library"), true, true),
    )
    .unwrap();
    let old_library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let old = replay(
        old_library,
        Issue72Fixture::with_database_evidence().database,
    );
    database.project_library_commits(root, &old).unwrap();
    let old_ids: Vec<_> = SongRepository::all_in_root(&database, root)
        .unwrap()
        .into_iter()
        .map(|song| song.id())
        .collect();
    let old_status = database.projection_status(root).unwrap();

    let new_library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let replacement = replay(new_library, Issue72Fixture::orphaned().portable);
    let connection = Connection::open(&database_path).unwrap();
    connection
        .execute_batch(&format!(
            "CREATE TRIGGER reject_projection_rebuild BEFORE INSERT ON songs WHEN NEW.library_root_uuid='{}' BEGIN SELECT RAISE(ABORT,'injected rebuild failure'); END;",
            root
        ))
        .unwrap();
    drop(connection);

    assert!(database
        .rebuild_library_projection(root, new_library, &replacement)
        .is_err());
    let restored_ids: Vec<_> = SongRepository::all_in_root(&database, root)
        .unwrap()
        .into_iter()
        .map(|song| song.id())
        .collect();
    assert_eq!(restored_ids, old_ids);
    assert_eq!(database.projection_status(root).unwrap(), old_status);

    Connection::open(&database_path)
        .unwrap()
        .execute_batch("DROP TRIGGER reject_projection_rebuild")
        .unwrap();
}

#[test]
fn library_durability_rebuild_commit_failure_keeps_the_previous_projection() {
    use rusqlite::Connection;

    let directory = tempfile::tempdir().unwrap();
    let database_path = directory.path().join("active.sqlite");
    let database = SqliteDatabase::open(&database_path).unwrap();
    let root = LibraryRootId::new();
    LibraryRepository::upsert(
        &database,
        &LibraryRoot::new(root, directory.path().join("library"), true, true),
    )
    .unwrap();
    let old_library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let old = replay(
        old_library,
        Issue72Fixture::with_database_evidence().database,
    );
    database.project_library_commits(root, &old).unwrap();
    let old_ids: Vec<_> = SongRepository::all_in_root(&database, root)
        .unwrap()
        .into_iter()
        .map(|song| song.id())
        .collect();
    let old_status = database.projection_status(root).unwrap();

    // The deferred foreign key allows the replacement statements to execute,
    // then rejects the transaction at COMMIT. This exercises the final atomic
    // boundary rather than an earlier INSERT/DELETE failure.
    Connection::open(&database_path)
        .unwrap()
        .execute_batch(&format!(
            "CREATE TABLE rebuild_commit_probe(song_uuid TEXT REFERENCES songs(uuid) DEFERRABLE INITIALLY DEFERRED);\
             CREATE TRIGGER fail_rebuild_at_commit AFTER DELETE ON songs WHEN OLD.library_root_uuid='{}' BEGIN INSERT INTO rebuild_commit_probe(song_uuid) VALUES('missing-song'); END;",
            root
        ))
        .unwrap();

    let new_library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let replacement = replay(new_library, Issue72Fixture::orphaned().portable);
    assert!(database
        .rebuild_library_projection(root, new_library, &replacement)
        .is_err());
    let restored_ids: Vec<_> = SongRepository::all_in_root(&database, root)
        .unwrap()
        .into_iter()
        .map(|song| song.id())
        .collect();
    assert_eq!(restored_ids, old_ids);
    assert_eq!(database.projection_status(root).unwrap(), old_status);

    Connection::open(&database_path)
        .unwrap()
        .execute_batch("DROP TRIGGER fail_rebuild_at_commit; DROP TABLE rebuild_commit_probe;")
        .unwrap();
}

#[test]
fn library_durability_read_only_projection_database_keeps_the_active_view() {
    let directory = tempfile::tempdir().unwrap();
    let database = SqliteDatabase::open(directory.path().join("active.sqlite")).unwrap();
    let root = LibraryRootId::new();
    LibraryRepository::upsert(
        &database,
        &LibraryRoot::new(root, directory.path().join("library"), true, true),
    )
    .unwrap();
    let library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let active = replay(library, Issue72Fixture::with_database_evidence().database);
    database.project_library_commits(root, &active).unwrap();
    let before_ids: Vec<_> = SongRepository::all_in_root(&database, root)
        .unwrap()
        .into_iter()
        .map(|song| song.id())
        .collect();
    let before_status = database.projection_status(root).unwrap();

    // SQLite query_only models an active database opened on a read-only volume.
    database
        .writer
        .run(|connection| {
            connection
                .execute_batch("PRAGMA query_only=ON;")
                .map_err(storage)
        })
        .unwrap();
    let replacement_library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let replacement = replay(replacement_library, Issue72Fixture::orphaned().portable);
    assert!(database
        .rebuild_library_projection(root, replacement_library, &replacement)
        .is_err());
    let after_ids: Vec<_> = SongRepository::all_in_root(&database, root)
        .unwrap()
        .into_iter()
        .map(|song| song.id())
        .collect();
    assert_eq!(after_ids, before_ids);
    assert_eq!(database.projection_status(root).unwrap(), before_status);
}

#[test]
fn library_durability_corrupt_sqlite_open_preserves_the_original_file() {
    let directory = tempfile::tempdir().unwrap();
    let database_path = directory.path().join("corrupt.sqlite");
    let damaged = b"synthetic damaged sqlite bytes; not a user database";
    std::fs::write(&database_path, damaged).unwrap();

    assert!(SqliteDatabase::open(&database_path).is_err());
    assert_eq!(std::fs::read(&database_path).unwrap(), damaged);
}

#[test]
fn library_durability_rebuild_refuses_sync_context_without_changing_the_old_view() {
    use rusqlite::Connection;

    let directory = tempfile::tempdir().unwrap();
    let database_path = directory.path().join("active.sqlite");
    let database = SqliteDatabase::open(&database_path).unwrap();
    let root = LibraryRootId::new();
    LibraryRepository::upsert(
        &database,
        &LibraryRoot::new(root, directory.path().join("library"), true, true),
    )
    .unwrap();
    let old_library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let old = replay(
        old_library,
        Issue72Fixture::with_database_evidence().database,
    );
    database.project_library_commits(root, &old).unwrap();
    let before = database.projection_status(root).unwrap();
    Connection::open(&database_path)
        .unwrap()
        .execute(
            "INSERT INTO sync_root_contexts(local_root_uuid) VALUES(?1)",
            [root.to_string()],
        )
        .unwrap();

    let new_library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let replacement = replay(new_library, Issue72Fixture::orphaned().portable);
    assert!(database
        .rebuild_library_projection(root, new_library, &replacement)
        .is_err());
    assert_eq!(database.projection_status(root).unwrap(), before);
    assert_eq!(
        SongRepository::all_in_root(&database, root).unwrap().len(),
        15
    );
}

#[test]
fn library_durability_projection_ahead_of_directory_rebuilds_from_the_manifest_library() {
    use crate::domain::library::LibraryManifest;
    use crate::infrastructure::filesystem::{RootControlPlane, RootRegistry};
    use rusqlite::Connection;

    let directory = tempfile::tempdir().unwrap();
    let library_path = directory.path().join("library");
    std::fs::create_dir_all(&library_path).unwrap();
    let root = LibraryRootId::new();
    let library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let registry = RootRegistry::new();
    registry.register(root, &library_path);
    let control = RootControlPlane::new(registry);
    control
        .write_manifest(root, &LibraryManifest::new(library, "test"))
        .unwrap();
    let records = Issue72Fixture::orphaned().portable;
    let batch = LibraryBatch::new(
        uuid::Uuid::new_v4(),
        library,
        ChangeOrigin::Recovery,
        records
            .into_iter()
            .map(|record| PortableRecordEnvelope::new(record, None).unwrap())
            .collect(),
    );
    control.durability().unwrap().commit(root, &batch).unwrap();
    let replay = control.durability().unwrap().recover(root).unwrap();

    let database_path = directory.path().join("active.sqlite");
    let database = SqliteDatabase::open(&database_path).unwrap();
    LibraryRepository::upsert(&database, &LibraryRoot::new(root, library_path, true, true))
        .unwrap();
    database.configure_library_control(std::sync::Arc::new(control));
    database.project_library_commits(root, &replay).unwrap();
    Connection::open(&database_path)
        .unwrap()
        .execute(
            "UPDATE library_projection_watermarks SET commit_seq=99 WHERE library_root_uuid=?1",
            [root.to_string()],
        )
        .unwrap();

    database.project_library_commits(root, &replay).unwrap();
    assert_eq!(
        database
            .projection_status(root)
            .unwrap()
            .applied_library_commit_seq,
        1
    );
    assert!(!SongRepository::all_in_root(&database, root)
        .unwrap()
        .is_empty());
}
