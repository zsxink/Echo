use super::*;
use crate::application::ports::{
    ControlPlanePort, LibraryProjectionPort, LibraryRepository, SongRepository,
};
use crate::application::testkit::library_durability::Issue72Fixture;
use crate::domain::entities::LibraryRoot;
use crate::domain::library::durability::{ChangeOrigin, LibraryBatch, LibraryCommit};
use crate::infrastructure::sqlite::SqliteDatabase;

fn replay(library: LibraryId, records: Vec<PortableRecord>) -> LibraryReplay {
    chain(library, vec![records])
}

/// An ordered chain of real commits: each batch binds its predecessor digest,
/// so the replay can only be projected by replaying the whole sequence.
fn chain(library: LibraryId, batches: Vec<Vec<PortableRecord>>) -> LibraryReplay {
    let mut commits = Vec::new();
    let mut predecessor: Option<String> = None;
    for (index, records) in batches.into_iter().enumerate() {
        let batch = LibraryBatch::new(
            uuid::Uuid::new_v4(),
            library,
            ChangeOrigin::Recovery,
            records
                .into_iter()
                .map(|record| PortableRecordEnvelope::new(record, None).unwrap())
                .collect(),
        );
        let seq = u64::try_from(index).unwrap() + 1;
        let digest = LibraryCommit::calculate_digest(&batch, seq, predecessor.as_deref()).unwrap();
        commits.push(LibraryCommit {
            batch,
            library_commit_seq: seq,
            predecessor_digest: predecessor,
            digest,
        });
        predecessor = commits.last().map(|commit| commit.digest.clone());
    }
    LibraryReplay {
        checkpoint: None,
        commits,
    }
}

fn song_ids(database: &SqliteDatabase, root: LibraryRootId) -> Vec<uuid::Uuid> {
    SongRepository::all_in_root(database, root)
        .unwrap()
        .into_iter()
        .map(|song| song.id().as_uuid())
        .collect()
}

fn no_candidate_directories(parent: &std::path::Path) -> bool {
    std::fs::read_dir(parent).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".projection-candidate-")
    })
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
    assert!(
        database
            .validate_library_projection(root, library, &candidate)
            .is_err()
    );
    let mut broken = candidate;
    broken.commits[0].library_commit_seq = 2;
    broken.commits[0].digest =
        LibraryCommit::calculate_digest(&broken.commits[0].batch, 2, None).unwrap();
    assert!(
        database
            .validate_library_projection(root, candidate_library, &broken)
            .is_err()
    );
    assert_eq!(database.read_recovery_records(root).unwrap(), before);
    assert!(std::fs::read_dir(directory.path()).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".projection-candidate-")
    }));
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

    assert!(
        database
            .rebuild_library_projection(root, new_library, &replacement)
            .is_err()
    );
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
    assert!(
        database
            .rebuild_library_projection(root, new_library, &replacement)
            .is_err()
    );
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
    assert!(
        database
            .rebuild_library_projection(root, replacement_library, &replacement)
            .is_err()
    );
    let after_ids: Vec<_> = SongRepository::all_in_root(&database, root)
        .unwrap()
        .into_iter()
        .map(|song| song.id())
        .collect();
    assert_eq!(after_ids, before_ids);
    assert_eq!(database.projection_status(root).unwrap(), before_status);
}

/// A database file whose header is unreadable cannot be projected from, so open
/// must move it aside byte for byte and start a fresh projection rather than
/// aborting startup. The damaged bytes stay available for diagnosis.
#[test]
fn library_durability_corrupt_sqlite_open_quarantines_the_original_and_starts_fresh() {
    let directory = tempfile::tempdir().unwrap();
    let database_path = directory.path().join("corrupt.sqlite");
    let damaged = b"synthetic damaged sqlite bytes; not a user database";
    std::fs::write(&database_path, damaged).unwrap();

    let database = SqliteDatabase::open(&database_path).expect("quarantine and reopen");

    let quarantined = database
        .quarantined_database()
        .expect("the damaged file is reported")
        .to_path_buf();
    assert_eq!(std::fs::read(&quarantined).unwrap(), damaged);
    assert!(
        quarantined
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains("quarantined"),
        "quarantine keeps a recognisable, unique name"
    );
    // The replacement is a healthy, migrated projection in the same place.
    assert!(database.quick_check().is_ok());
    assert_eq!(database.path(), database_path);
    assert!(!std::fs::read(&database_path).unwrap().is_empty());
    // The replacement is empty: directory replay, not rescue, rebuilds it.
    assert!(database.active_root().unwrap().is_none());
}

/// Quarantine is only for damage. A released-migration checksum mismatch means
/// the file is intact but was written by an incompatible build; renaming it
/// aside would discard a usable projection, so open still fails and the file
/// stays where it is.
#[test]
fn library_durability_open_never_quarantines_a_healthy_database_it_cannot_upgrade() {
    let directory = tempfile::tempdir().unwrap();
    let database_path = directory.path().join("intact.sqlite");
    drop(SqliteDatabase::open(&database_path).unwrap());
    {
        let connection = rusqlite::Connection::open(&database_path).unwrap();
        connection
            .execute(
                "UPDATE schema_migrations SET checksum='tampered' WHERE version=1",
                [],
            )
            .unwrap();
    }

    let Err(failure) = SqliteDatabase::open(&database_path) else {
        panic!("checksum mismatch must fail open");
    };
    assert!(matches!(failure, Error::InvariantViolation { .. }));
    assert!(database_path.exists(), "the intact file was not moved");
    assert!(!directory.path().read_dir().unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains("quarantined")
    }));
}

#[test]
fn library_durability_rebuild_rejects_a_projection_that_changed_protocol_history() {
    use crate::domain::sync::{AuthorityId, Epoch, SyncBinding};
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
    let binding = SyncBinding {
        library_id: crate::domain::sync::LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    database.bind_sync_library(binding, true, root).unwrap();
    let connection = Connection::open(&database_path).unwrap();
    connection
        .execute(
            "INSERT INTO sync_commits(commit_seq,library_id,authority_id,epoch,origin_device_id,mutation_id,canonical_json,previous_digest,commit_digest,publication_state,created_at) VALUES(1,?1,?2,?3,?4,?5,'{}',?6,?7,'published',1)",
            rusqlite::params![
                binding.library_id.as_uuid().to_string(),
                binding.authority_id.as_uuid().to_string(),
                i64::try_from(binding.epoch.as_u64()).unwrap(),
                uuid::Uuid::new_v4().to_string(),
                uuid::Uuid::new_v4().to_string(),
                vec![0u8; 32],
                vec![3u8; 32],
            ],
        )
        .unwrap();
    // A rebuild that reaches into protocol history must be rejected, not
    // committed: the ledger row is evidence no directory replay can restore.
    connection
        .execute_batch(&format!(
            "CREATE TRIGGER drop_protocol_history AFTER INSERT ON songs WHEN NEW.library_root_uuid='{}' BEGIN DELETE FROM sync_commits; END;",
            root
        ))
        .unwrap();
    drop(connection);
    let before = ProtocolContext::read(&Connection::open(&database_path).unwrap()).unwrap();
    let old_status = database.projection_status(root).unwrap();

    let new_library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let replacement = replay(new_library, Issue72Fixture::orphaned().portable);
    let failure = match database.rebuild_library_projection(root, new_library, &replacement) {
        Ok(_) => panic!("rebuild must reject changed protocol history"),
        Err(error) => error.to_string(),
    };
    assert!(failure.contains("protocol history"), "{failure}");
    let connection = Connection::open(&database_path).unwrap();
    assert_eq!(ProtocolContext::read(&connection).unwrap(), before);
    drop(connection);
    assert_eq!(database.projection_status(root).unwrap(), old_status);
    assert_eq!(
        SongRepository::all_in_root(&database, root).unwrap().len(),
        15
    );
}

#[test]
fn library_durability_rebuild_preserves_sync_context_protocol_history() {
    use crate::domain::sync::{AuthorityId, DeviceId, Epoch, SyncBinding};
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

    // Two roots own sync contexts; `other_root` is archived because the active
    // context belongs to `root`. Both carry protocol evidence a rebuild must
    // not touch: a commit ledger row, a confirmed object version, and an inbox
    // receipt, which no directory replay may re-derive (LD-07 keeps unconfirmed
    // intent and conflicts across a rebuild).
    let root_binding = SyncBinding {
        library_id: crate::domain::sync::LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    let other_binding = SyncBinding {
        library_id: crate::domain::sync::LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    database
        .bind_sync_library(root_binding, true, root)
        .unwrap();
    database
        .bind_sync_library(other_binding, true, other_root)
        .unwrap();
    let connection = Connection::open(&database_path).unwrap();
    for (prefix, binding) in [("", root_binding), ("archived_", other_binding)] {
        let context = if prefix.is_empty() { "root" } else { "other" };
        connection
            .execute(
                &format!(
                    "INSERT INTO {prefix}sync_commits(commit_seq,library_id,authority_id,epoch,origin_device_id,mutation_id,canonical_json,previous_digest,commit_digest,publication_state,created_at) VALUES(1,?1,?2,?3,?4,?5,?6,?7,?8,'published',1)"
                ),
                rusqlite::params![
                    binding.library_id.as_uuid().to_string(),
                    binding.authority_id.as_uuid().to_string(),
                    i64::try_from(binding.epoch.as_u64()).unwrap(),
                    DeviceId::new().as_uuid().to_string(),
                    uuid::Uuid::new_v4().to_string(),
                    format!("{{\"probe\":\"{context}\"}}"),
                    vec![0u8; 32],
                    // Distinct digests: `commit_digest` is UNIQUE.
                    if prefix.is_empty() {
                        vec![1u8; 32]
                    } else {
                        vec![2u8; 32]
                    },
                ],
            )
            .unwrap();
    }
    drop(connection);

    let protocol_before =
        ProtocolContext::read(&Connection::open(&database_path).unwrap()).unwrap();
    assert!(
        protocol_before
            .live
            .iter()
            .any(|(table, count)| *table == "sync_commits" && *count == 1)
    );
    assert!(
        protocol_before
            .archived
            .iter()
            .any(|(table, count)| *table == "sync_commits" && *count == 1)
    );

    let new_library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let replacement = replay(new_library, Issue72Fixture::orphaned().portable);
    let report = database
        .rebuild_library_projection(root, new_library, &replacement)
        .unwrap();
    assert_eq!(database.projection_status(root).unwrap(), report.projection);
    assert_eq!(
        ProtocolContext::read(&Connection::open(&database_path).unwrap()).unwrap(),
        protocol_before
    );
    assert_eq!(
        SongRepository::all_in_root(&database, other_root)
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn library_durability_rebuild_isolates_a_projection_belonging_to_another_library() {
    let directory = tempfile::tempdir().unwrap();
    let database = SqliteDatabase::open(directory.path().join("active.sqlite")).unwrap();
    let root = LibraryRootId::new();
    LibraryRepository::upsert(
        &database,
        &LibraryRoot::new(root, directory.path().join("library"), true, true),
    )
    .unwrap();
    let old_library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    database
        .project_library_commits(
            root,
            &replay(
                old_library,
                Issue72Fixture::with_database_evidence().database,
            ),
        )
        .unwrap();
    let old_ids = song_ids(&database, root);
    assert!(!old_ids.is_empty());

    // The verified directory now names a different library: the retained view
    // cannot be merged with it, so adoption replaces the whole root.
    let new_library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let replacement = replay(new_library, Issue72Fixture::orphaned().portable);
    let incremental = database
        .project_library_commits(root, &replacement)
        .expect_err("a foreign-library projection is not merged in place");
    // The existing watermark already covers this sequence with a different
    // applied operation, so the chain cannot be continued in place.
    assert!(
        incremental
            .to_string()
            .contains("differs from library chain")
            || incremental.to_string().contains("another library"),
        "{incremental}"
    );
    assert_eq!(song_ids(&database, root), old_ids);

    let report = database
        .rebuild_library_projection(root, new_library, &replacement)
        .unwrap();
    assert_eq!(database.projection_status(root).unwrap(), report.projection);
    let adopted: std::collections::BTreeSet<_> = song_ids(&database, root).into_iter().collect();
    let expected: std::collections::BTreeSet<_> = replacement.commits[0]
        .batch
        .records
        .iter()
        .filter_map(|envelope| match envelope.record() {
            PortableRecord::Song(song) => Some(song.song_uuid),
            _ => None,
        })
        .collect();
    assert_eq!(adopted, expected);
}

#[test]
fn library_durability_rebuild_rejects_a_broken_directory_chain() {
    let directory = tempfile::tempdir().unwrap();
    let database = SqliteDatabase::open(directory.path().join("active.sqlite")).unwrap();
    let root = LibraryRootId::new();
    LibraryRepository::upsert(
        &database,
        &LibraryRoot::new(root, directory.path().join("library"), true, true),
    )
    .unwrap();
    let old_library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    database
        .project_library_commits(
            root,
            &replay(
                old_library,
                Issue72Fixture::with_database_evidence().database,
            ),
        )
        .unwrap();
    let before_ids = song_ids(&database, root);
    let before_status = database.projection_status(root).unwrap();

    // Sequence 2 without sequence 1 is a gap, not a chain: the candidate can
    // never be verified against a head the directory cannot prove.
    let new_library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let mut broken = chain(
        new_library,
        vec![
            Issue72Fixture::orphaned().portable,
            Issue72Fixture::orphaned().portable,
        ],
    );
    broken.commits.remove(0);
    let failure = database
        .rebuild_library_projection(root, new_library, &broken)
        .expect_err("a gapped chain must not be adopted");
    assert!(
        failure.to_string().contains("incomplete"),
        "unexpected failure: {failure}"
    );
    assert_eq!(song_ids(&database, root), before_ids);
    assert_eq!(database.projection_status(root).unwrap(), before_status);
    assert!(no_candidate_directories(directory.path()));
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
    assert!(
        !SongRepository::all_in_root(&database, root)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn library_durability_rebuild_preserves_unjournaled_legacy_snapshots() {
    use crate::application::ports::PlaylistRepository;
    use crate::domain::ids::{PlaylistId, SongId};
    use crate::domain::library::LibraryManifest;
    use crate::infrastructure::filesystem::{RootControlPlane, RootRegistry};

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
    let records = Issue72Fixture::with_database_evidence().database;
    let song = records
        .iter()
        .find(|r| matches!(r, PortableRecord::Song(_)))
        .unwrap()
        .clone();
    let playlist = records
        .iter()
        .find(|r| matches!(r, PortableRecord::Playlist(_)))
        .unwrap()
        .clone();
    let favorite = records
        .iter()
        .find(|r| matches!(r, PortableRecord::Favorite(f) if f.song_uuid == song.object_uuid()))
        .unwrap()
        .clone();
    control.write_record(root, &song).unwrap();
    control.write_record(root, &playlist).unwrap();
    let batch = LibraryBatch::new(
        uuid::Uuid::new_v4(),
        library,
        ChangeOrigin::LocalEdit,
        vec![PortableRecordEnvelope::new(favorite, None).unwrap()],
    );
    control.durability().unwrap().commit(root, &batch).unwrap();
    let replay = control.durability().unwrap().recover(root).unwrap();
    let database_path = directory.path().join("active.sqlite");
    let database = SqliteDatabase::open(&database_path).unwrap();
    LibraryRepository::upsert(&database, &LibraryRoot::new(root, library_path, true, true))
        .unwrap();
    database.configure_library_control(std::sync::Arc::new(control));
    database
        .project_legacy_records(
            root,
            &[
                PortableRecordEnvelope::new(song.clone(), None).unwrap(),
                PortableRecordEnvelope::new(playlist.clone(), None).unwrap(),
            ],
        )
        .unwrap();
    database.project_library_commits(root, &replay).unwrap();
    rusqlite::Connection::open(&database_path)
        .unwrap()
        .execute(
            "UPDATE library_projection_watermarks SET commit_seq=99 WHERE library_root_uuid=?1",
            [root.to_string()],
        )
        .unwrap();
    database.project_library_commits(root, &replay).unwrap();
    assert!(
        SongRepository::by_id(&database, SongId::from_uuid(song.object_uuid()))
            .unwrap()
            .unwrap()
            .favorite()
    );
    assert_eq!(
        PlaylistRepository::name(&database, PlaylistId::from_uuid(playlist.object_uuid()))
            .unwrap()
            .as_deref(),
        Some("合成历史歌单")
    );
}

#[test]
fn library_durability_rebuild_adopts_a_projection_behind_the_directory_head() {
    let directory = tempfile::tempdir().unwrap();
    let database = SqliteDatabase::open(directory.path().join("active.sqlite")).unwrap();
    let root = LibraryRootId::new();
    LibraryRepository::upsert(
        &database,
        &LibraryRoot::new(root, directory.path().join("library"), true, true),
    )
    .unwrap();
    let library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let full = chain(
        library,
        vec![
            Issue72Fixture::orphaned().portable,
            Issue72Fixture::orphaned().portable,
        ],
    );
    // The database only knows the first link: it is behind the verified head.
    let behind = LibraryReplay {
        checkpoint: None,
        commits: vec![full.commits[0].clone()],
    };
    database.project_library_commits(root, &behind).unwrap();
    assert_eq!(
        database
            .projection_status(root)
            .unwrap()
            .applied_library_commit_seq,
        1
    );

    let report = database
        .rebuild_library_projection(root, library, &full)
        .unwrap();
    let status = database.projection_status(root).unwrap();
    // The whole chain is projected, not only the newest link, and the adopted
    // head is the directory head the candidate was verified against.
    assert_eq!(status.applied_library_commit_seq, 2);
    assert_eq!(status, report.projection);
    assert_eq!(
        status.applied_head_digest.as_deref(),
        full.commits.last().map(|commit| commit.digest.as_str())
    );
    assert!(!song_ids(&database, root).is_empty());
    assert!(no_candidate_directories(directory.path()));
}

#[test]
fn library_durability_rebuild_rejects_a_candidate_that_changed_after_projection() {
    let directory = tempfile::tempdir().unwrap();
    let database = SqliteDatabase::open(directory.path().join("active.sqlite")).unwrap();
    let root = LibraryRootId::new();
    LibraryRepository::upsert(
        &database,
        &LibraryRoot::new(root, directory.path().join("library"), true, true),
    )
    .unwrap();
    let old_library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    database
        .project_library_commits(
            root,
            &replay(
                old_library,
                Issue72Fixture::with_database_evidence().database,
            ),
        )
        .unwrap();
    let before_ids = song_ids(&database, root);
    let before_status = database.projection_status(root).unwrap();
    assert!(!before_ids.is_empty());

    let new_library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let replacement = replay(new_library, Issue72Fixture::orphaned().portable);
    inject_candidate_corruption(root, |candidate| {
        candidate
            .execute(
                "UPDATE library_recovery_records SET envelope_json='{\"record\":{\"unknown\":1}}' WHERE row_kind='record'",
                [],
            )
            .map_err(storage)?;
        Ok(())
    });
    assert!(
        database
            .rebuild_library_projection(root, new_library, &replacement)
            .is_err()
    );
    // A candidate whose bytes no longer match the directory never replaces the
    // active projection, and no private candidate directory is left behind.
    assert_eq!(song_ids(&database, root), before_ids);
    assert_eq!(database.projection_status(root).unwrap(), before_status);
    assert!(no_candidate_directories(directory.path()));
}

#[test]
fn library_durability_rebuild_clears_search_rows_and_artist_covers_of_the_replaced_root() {
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
    database
        .project_library_commits(
            root,
            &replay(
                old_library,
                Issue72Fixture::with_database_evidence().database,
            ),
        )
        .unwrap();
    let old_ids: std::collections::BTreeSet<String> = song_ids(&database, root)
        .into_iter()
        .map(|id| id.to_string())
        .collect();
    let connection = Connection::open(&database_path).unwrap();
    let indexed = |connection: &Connection, ids: &std::collections::BTreeSet<String>| -> u64 {
        let mut total = 0;
        for id in ids {
            total += connection
                .query_row(
                    "SELECT COUNT(*) FROM song_search WHERE song_uuid=?1",
                    [id],
                    |row| row.get::<_, u64>(0),
                )
                .unwrap();
        }
        total
    };
    assert_eq!(indexed(&connection, &old_ids), old_ids.len() as u64);
    for (root_id, artist) in [(&root, "stale-artist"), (&other_root, "other-artist")] {
        connection
            .execute(
                "INSERT INTO artist_covers(library_root_uuid,artist_key,cover_asset_key,updated_at) VALUES(?1,?2,'cover',0)",
                rusqlite::params![root_id.to_string(), artist],
            )
            .unwrap();
    }
    drop(connection);

    let new_library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let replacement = replay(new_library, Issue72Fixture::orphaned().portable);
    database
        .rebuild_library_projection(root, new_library, &replacement)
        .unwrap();
    let new_ids: std::collections::BTreeSet<String> = song_ids(&database, root)
        .into_iter()
        .map(|id| id.to_string())
        .collect();
    // The fixture reuses deterministic song identities, so only the songs the
    // rebuilt replay dropped are the ones that must stop being searchable.
    let removed: std::collections::BTreeSet<String> =
        old_ids.difference(&new_ids).cloned().collect();
    assert!(!removed.is_empty());
    let connection = Connection::open(&database_path).unwrap();
    // The fts5 mirror has no trigger and no foreign key: without an explicit
    // root-scoped delete the replaced root would answer searches with songs it
    // no longer contains.
    assert_eq!(indexed(&connection, &removed), 0);
    assert_eq!(indexed(&connection, &new_ids), new_ids.len() as u64);
    let covers = |connection: &Connection, root_id: &LibraryRootId| -> u64 {
        connection
            .query_row(
                "SELECT COUNT(*) FROM artist_covers WHERE library_root_uuid=?1",
                [root_id.to_string()],
                |row| row.get(0),
            )
            .unwrap()
    };
    // A rebuilt replay without that cover must not keep the old assignment;
    // other roots keep theirs.
    assert_eq!(covers(&connection, &root), 0);
    assert_eq!(covers(&connection, &other_root), 1);
}
