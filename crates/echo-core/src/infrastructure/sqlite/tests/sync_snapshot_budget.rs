//! Regression tests for durable snapshot count and encoded-byte page budgets.

use super::*;
use crate::application::ports::{
    DeviceIdProvider, HostCommitLedgerPort, LibraryRepository, ReplicaSnapshotPage,
};
use crate::domain::entities::LibraryRoot;
use crate::domain::ids::LibraryRootId;
use crate::domain::sync::{AuthorityId, Epoch, LibraryId};

fn host() -> (tempfile::TempDir, SqliteDatabase, SyncBinding) {
    let directory = tempfile::tempdir().unwrap();
    let database = SqliteDatabase::open(directory.path().join("echo.db")).unwrap();
    let root = LibraryRootId::new();
    LibraryRepository::upsert(
        &database,
        &LibraryRoot::new(root, directory.path().join("library"), true, true),
    )
    .unwrap();
    let binding = SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    database.bind_sync_library(binding, true, root).unwrap();
    (directory, database, binding)
}

fn publish_song(
    database: &SqliteDatabase,
    binding: SyncBinding,
    previous: Option<MutationId>,
    sequence: u64,
    audio: bool,
    extra_bytes: usize,
) -> MutationId {
    let song = SongId::new();
    let candidate = MutationCandidate {
        resource_refs: Vec::new(),
        mutation_id: MutationId::new(),
        origin_device_id: DeviceId::from_uuid(database.current_device_id().as_uuid()),
        origin_seq: sequence,
        previous_mutation_id: previous,
        object: ObjectKey::Song(song),
        base_object_version: None,
        payload: serde_json::json!({"song_uuid":song,"relative_path":format!("{song}.flac"),"title":"budget","added_at":1,"extra":"x".repeat(extra_bytes)}),
    };
    let mut checkpoint = database
        .stage_confirmation(binding, &candidate)
        .unwrap()
        .checkpoint
        .unwrap();
    if audio {
        checkpoint.digest = database.writer.run(move |connection| {
            let body: String = connection.query_row("SELECT canonical_json FROM sync_commits WHERE commit_seq=?1", params![sequence], |row| row.get(0)).map_err(storage)?;
            let resource = serde_json::json!([{"kind":"audio","content_hash":"ab".repeat(32),"length":3,"media_type":"audio/flac","relative_path":format!("media/{song}.flac")}]);
            let body = body.replace("\"resource_refs\":[]", &format!("\"resource_refs\":{resource}"));
            let previous: Vec<u8> = connection.query_row("SELECT previous_digest FROM sync_commits WHERE commit_seq=?1", params![sequence], |row| row.get(0)).map_err(storage)?;
            let mut hash = blake3::Hasher::new();
            hash.update(b"echo.lan.commit.v1\0");
            hash.update(&1u16.to_be_bytes());
            hash.update(&sequence.to_be_bytes());
            hash.update(&previous);
            hash.update(&u32::try_from(body.len()).unwrap().to_be_bytes());
            hash.update(body.as_bytes());
            let digest = *hash.finalize().as_bytes();
            connection.execute("UPDATE sync_commits SET canonical_json=?1,commit_digest=?2 WHERE commit_seq=?3", params![body,digest.as_slice(),sequence]).map_err(storage)?;
            Ok(digest)
        }).unwrap();
    }
    database.mark_published(checkpoint).unwrap();
    candidate.mutation_id
}

fn assert_page_budget(page: &HostSnapshotPage) {
    let resources: usize = page
        .records
        .iter()
        .map(|record| match record {
            HostSnapshotRecord::Object { resources, .. } => resources.len(),
            _ => 0,
        })
        .sum();
    assert!(page.records.len() + resources <= 512);
    let replica = ReplicaSnapshotPage {
        snapshot_token: page.session.token.clone(),
        checkpoint: page.session.checkpoint,
        page_index: page.page_index,
        next_page_index: page.next_page_index,
        records: page.records.clone(),
    };
    assert!(serde_json::to_vec(&replica).unwrap().len() <= 16 * 1024 * 1024);
}

#[test]
fn snapshot_combines_object_and_resource_count_budgets_with_stable_retries() {
    let (directory, database, binding) = host();
    let mut previous = None;
    for sequence in 1..=257 {
        previous = Some(publish_song(
            &database, binding, previous, sequence, true, 0,
        ));
    }
    let session = database.create_host_snapshot(binding, 512, 60_000).unwrap();
    let first = database
        .host_snapshot_page(binding, &session.token, 0)
        .unwrap();
    let last = database
        .host_snapshot_page(binding, &session.token, 1)
        .unwrap();
    assert_eq!(first.records.len(), 256);
    assert_eq!(first.next_page_index, Some(1));
    assert_eq!(last.records.len(), 1);
    assert_eq!(last.next_page_index, None);
    assert_page_budget(&first);
    assert_page_budget(&last);
    publish_song(&database, binding, previous, 258, true, 0);
    assert_eq!(
        database
            .host_snapshot_page(binding, &session.token, 0)
            .unwrap(),
        first
    );
    drop(database);
    let database = SqliteDatabase::open(directory.path().join("echo.db")).unwrap();
    assert_eq!(
        database
            .host_snapshot_page(binding, &session.token, 1)
            .unwrap(),
        last
    );
    assert!(
        database
            .host_snapshot_page(binding, &session.token, 2)
            .is_err()
    );
}

#[test]
fn snapshot_splits_large_payloads_before_encoded_page_limit_and_survives_restart() {
    let (directory, database, binding) = host();
    let mut previous = None;
    for sequence in 1..=20 {
        previous = Some(publish_song(
            &database, binding, previous, sequence, false, 950_000,
        ));
    }
    let session = database.create_host_snapshot(binding, 512, 60_000).unwrap();
    let first = database
        .host_snapshot_page(binding, &session.token, 0)
        .unwrap();
    let last = database
        .host_snapshot_page(binding, &session.token, 1)
        .unwrap();
    assert!(first.records.len() < 20);
    assert_eq!(first.records.len() + last.records.len(), 20);
    assert_eq!(first.next_page_index, Some(1));
    assert_eq!(last.next_page_index, None);
    assert_page_budget(&first);
    assert_page_budget(&last);
    assert_eq!(
        database
            .host_snapshot_page(binding, &session.token, 0)
            .unwrap(),
        first
    );
    drop(database);
    let database = SqliteDatabase::open(directory.path().join("echo.db")).unwrap();
    assert_eq!(
        database
            .host_snapshot_page(binding, &session.token, 0)
            .unwrap(),
        first
    );
    assert_eq!(
        database
            .host_snapshot_page(binding, &session.token, 1)
            .unwrap(),
        last
    );
}

#[test]
fn page_budget_migration_invalidates_legacy_sessions_without_changing_library() {
    let (directory, database, binding) = host();
    publish_song(&database, binding, None, 1, true, 0);
    let session = database.create_host_snapshot(binding, 512, 60_000).unwrap();
    let expected = database
        .host_snapshot_page(binding, &session.token, 0)
        .unwrap();
    database.writer.run(|connection| {
        connection.execute_batch("DROP TABLE sync_snapshot_pages; DELETE FROM schema_migrations WHERE version=15;").map_err(storage)
    }).unwrap();
    drop(database);
    let database = SqliteDatabase::open(directory.path().join("echo.db")).unwrap();
    assert!(
        database
            .host_snapshot_page(binding, &session.token, 0)
            .is_err()
    );
    let fresh = database.create_host_snapshot(binding, 512, 60_000).unwrap();
    let page = database
        .host_snapshot_page(binding, &fresh.token, 0)
        .unwrap();
    assert_eq!(page.records, expected.records);
    assert_eq!(page.session.checkpoint, expected.session.checkpoint);
    database
        .with_reader(|connection| {
            let count: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM sync_snapshot_sessions WHERE token=?1",
                    params![session.token],
                    |row| row.get(0),
                )
                .map_err(storage)?;
            assert_eq!(count, 0);
            let count: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM songs WHERE uuid=?1",
                    params![match &expected.records[0] {
                        HostSnapshotRecord::Object { candidate, .. } => match candidate.object {
                            ObjectKey::Song(song) => song.as_uuid().to_string(),
                            _ => unreachable!(),
                        },
                        _ => unreachable!(),
                    }],
                    |row| row.get(0),
                )
                .map_err(storage)?;
            assert_eq!(count, 1);
            Ok(())
        })
        .unwrap();
}
