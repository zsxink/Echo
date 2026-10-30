use crate::application::ports::{LibraryRepository, LocalSyncQueuePort, SongRepository};
use crate::domain::entities::{LibraryRoot, Song};
use crate::domain::ids::{LibraryRootId, RelativeMediaPath, Revision, SongId};
use crate::domain::sync::{AuthorityId, Epoch, LibraryId, ObjectKey, SyncBinding};
use crate::infrastructure::sqlite::SqliteDatabase;

fn unbound_database() -> (tempfile::TempDir, SqliteDatabase, LibraryRootId) {
    let directory = tempfile::tempdir().unwrap();
    let database = SqliteDatabase::open(directory.path().join("echo.db")).unwrap();
    let root = LibraryRootId::new();
    LibraryRepository::upsert(
        &database,
        &LibraryRoot::new(root, directory.path().join("library"), true, true),
    )
    .unwrap();
    (directory, database, root)
}

/// Queue tests exercise sync-bound libraries: standalone edits deliberately do
/// not mint sync mutations (sync-foundation: 单机编辑不创建同步候选).
fn database() -> (tempfile::TempDir, SqliteDatabase, LibraryRootId) {
    let (directory, database, root) = unbound_database();
    database
        .bind_sync_library(
            SyncBinding {
                library_id: LibraryId::new(),
                authority_id: AuthorityId::new(),
                epoch: Epoch::INITIAL,
            },
            true,
            root,
        )
        .unwrap();
    (directory, database, root)
}

#[test]
fn local_queue_is_ordered_and_terminal_rejection_does_not_block_later_edits() {
    let (_directory, database, root) = database();
    let song_id = SongId::new();
    let mut song = Song::new(
        song_id,
        root,
        RelativeMediaPath::new("artist/song.flac").unwrap(),
        Revision::INITIAL,
    );
    song.apply_metadata(
        Some("Song".to_owned()),
        Some("Artist".to_owned()),
        None,
        None,
    );
    SongRepository::upsert(&database, &song).unwrap();
    SongRepository::set_favorite(&database, song_id, true).unwrap();
    SongRepository::set_favorite(&database, song_id, false).unwrap();

    let queued = database.pending_local_mutations(10).unwrap();
    assert_eq!(queued.len(), 3, "song plus favorite edits remain queued");
    assert_eq!(queued[0].origin_seq, 1);
    assert_eq!(queued[1].origin_seq, 2);
    assert_eq!(queued[2].origin_seq, 3);
    assert_eq!(queued[1].previous_mutation_id, Some(queued[0].mutation_id));
    assert_eq!(queued[2].previous_mutation_id, Some(queued[1].mutation_id));
    assert_eq!(queued[0].object, ObjectKey::Song(song_id));
    assert_eq!(queued[1].object, ObjectKey::Favorite(song_id));

    database
        .reject_local_mutation(
            queued[0].mutation_id,
            r#"{"accepted":false,"reason":"stale object"}"#.to_owned(),
        )
        .unwrap();
    let retryable = database.pending_local_mutations(10).unwrap();
    assert_eq!(retryable.len(), 2);
    assert_eq!(retryable[0].origin_seq, 2);
    assert_eq!(retryable[1].origin_seq, 3);
    // A second acknowledgement for the same event is what a retried peer cycle
    // looks like. It must be a no-op — the terminal row stays terminal and keeps
    // the outcome it was retired under — instead of surfacing a failure the
    // caller reports as "the local ledger could not be read", which is what
    // mapping the absent `status='pending'` row to `Storage` used to do.
    database
        .reject_local_mutation(
            queued[0].mutation_id,
            r#"{"accepted":false,"reason":"duplicate terminal"}"#.to_owned(),
        )
        .expect("a repeated acknowledgement is idempotent");
    let after = database.pending_local_mutations(10).unwrap();
    assert_eq!(after.len(), 2, "later edits are still queued for push");
    assert!(
        after
            .iter()
            .all(|event| event.mutation_id != queued[0].mutation_id),
        "a retired event never re-enters the pending chain"
    );
    assert_eq!(after[0].mutation_id, retryable[0].mutation_id);
}

#[test]
fn terminal_rejections_repair_a_legacy_local_origin_cursor_gap() {
    use crate::application::ports::DeviceIdProvider;

    let (_directory, database, root) = database();
    let song_id = SongId::new();
    let song = Song::new(
        song_id,
        root,
        RelativeMediaPath::new("artist/song.flac").unwrap(),
        Revision::INITIAL,
    );
    SongRepository::upsert(&database, &song).unwrap();
    SongRepository::set_favorite(&database, song_id, true).unwrap();
    SongRepository::set_favorite(&database, song_id, false).unwrap();
    let queued = database.pending_local_mutations(10).unwrap();
    let device = database.current_device_id().to_string();
    let rejected_id = queued[0].mutation_id.as_uuid().to_string();
    let expected_id = rejected_id.clone();

    database
        .writer
        .run(move |connection| {
            connection
                .execute(
                    "UPDATE sync_mutations SET status='rejected',outcome_json='{}' WHERE origin_device_id=?1 AND mutation_id=?2",
                    rusqlite::params![device, rejected_id],
                )
                .map_err(crate::infrastructure::sqlite::support::storage)?;
            Ok(())
        })
        .unwrap();

    assert_eq!(database.advance_rejected_local_mutations().unwrap(), 1);
    assert_eq!(database.advance_rejected_local_mutations().unwrap(), 0);
    let cursor: (i64, String) = database
        .writer
        .run(|connection| {
            connection
                .query_row(
                    "SELECT origin_seq,last_mutation_id FROM sync_origin_cursors",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(crate::infrastructure::sqlite::support::storage)
        })
        .unwrap();
    assert_eq!(cursor.0, 1);
    assert_eq!(cursor.1, expected_id);
    assert_eq!(database.pending_local_mutations(10).unwrap().len(), 2);
}

#[test]
fn replica_checkpoint_loads_the_persisted_host_cursor_and_genesis() {
    let (_directory, database, root) = unbound_database();
    let host = crate::domain::sync::DeviceId::new();
    let binding = SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    database.bind_sync_library(binding, false, root).unwrap();
    assert_eq!(
        database.replica_checkpoint(host).unwrap(),
        crate::domain::sync::CommitCheckpoint {
            binding,
            sequence: crate::domain::sync::CommitSeq::GENESIS,
            digest: [0; 32],
        }
    );
}

#[test]
fn queued_resource_declarations_are_frozen_across_later_edits_and_restart() {
    use crate::domain::sync::{ResourceDigest, SharedResourceKind, SharedResourceManifest};
    let (directory, database, root) = database();
    let song_id = SongId::new();
    let song = Song::new(
        song_id,
        root,
        RelativeMediaPath::new("song.flac").unwrap(),
        Revision::INITIAL,
    );
    let first = SharedResourceManifest::new(
        SharedResourceKind::Audio,
        ResourceDigest::from_bytes([1; 32]),
        17,
        "audio/flac".to_owned(),
        "media/song.flac",
    )
    .unwrap();
    database
        .set_local_sync_resources(ObjectKey::Song(song_id), vec![first.clone()])
        .unwrap();
    SongRepository::upsert(&database, &song).unwrap();
    let second = SharedResourceManifest::new(
        SharedResourceKind::Audio,
        ResourceDigest::from_bytes([2; 32]),
        23,
        "audio/flac".to_owned(),
        "media/song.flac",
    )
    .unwrap();
    database
        .set_local_sync_resources(ObjectKey::Song(song_id), vec![second.clone()])
        .unwrap();
    SongRepository::upsert(&database, &song).unwrap();
    drop(database);
    let database = SqliteDatabase::open(directory.path().join("echo.db")).unwrap();
    let queue = database.pending_local_mutations(8).unwrap();
    assert_eq!(queue[0].resource_refs, vec![first]);
    assert_eq!(queue[1].resource_refs, vec![second]);
}

#[test]
fn conflicting_resource_metadata_rolls_back_the_previous_object_declarations() {
    use crate::domain::sync::{ResourceDigest, SharedResourceKind, SharedResourceManifest};
    let (_directory, database, root) = database();
    let song_id = SongId::new();
    let resource = SharedResourceManifest::new(
        SharedResourceKind::Audio,
        ResourceDigest::from_bytes([3; 32]),
        17,
        "audio/flac".to_owned(),
        "media/song.flac",
    )
    .unwrap();
    database
        .set_local_sync_resources(ObjectKey::Song(song_id), vec![resource.clone()])
        .unwrap();
    let invalid = SharedResourceManifest {
        length: 99,
        ..resource.clone()
    };
    assert!(database
        .set_local_sync_resources(ObjectKey::Song(song_id), vec![invalid])
        .is_err());
    let song = Song::new(
        song_id,
        root,
        RelativeMediaPath::new("song.flac").unwrap(),
        Revision::INITIAL,
    );
    SongRepository::upsert(&database, &song).unwrap();
    assert_eq!(
        database.pending_local_mutations(8).unwrap()[0].resource_refs,
        vec![resource]
    );
}

#[test]
fn queued_request_body_stays_identical_when_local_audio_goes_missing() {
    use crate::domain::entities::SongAvailability;
    use crate::domain::sync::{ResourceDigest, SharedResourceKind, SharedResourceManifest};

    let (_directory, database, root) = database();
    let id = SongId::new();
    let resource = SharedResourceManifest::new(
        SharedResourceKind::Audio,
        ResourceDigest::from_bytes([7; 32]),
        17,
        "audio/flac".into(),
        "song.flac",
    )
    .unwrap();
    database
        .set_local_sync_resources(ObjectKey::Song(id), vec![resource])
        .unwrap();
    let song = Song::new(
        id,
        root,
        RelativeMediaPath::new("song.flac").unwrap(),
        Revision::INITIAL,
    );
    SongRepository::upsert(&database, &song).unwrap();
    let before = serde_json::to_string(&database.pending_local_mutations(100).unwrap()).unwrap();
    SongRepository::set_availability(&database, id, SongAvailability::Missing).unwrap();
    assert_eq!(
        serde_json::to_string(&database.pending_local_mutations(100).unwrap()).unwrap(),
        before
    );
}

#[test]
fn queue_summary_counts_the_entire_backlog_and_excludes_terminal_mutations() {
    let (_directory, database, root) = database();
    let id = SongId::new();
    let song = Song::new(
        id,
        root,
        RelativeMediaPath::new("song.flac").unwrap(),
        Revision::INITIAL,
    );
    SongRepository::upsert(&database, &song).unwrap();
    for index in 0..514 {
        SongRepository::set_favorite(&database, id, index % 2 == 0).unwrap();
    }
    assert_eq!(database.pending_local_mutations(512).unwrap().len(), 512);
    assert_eq!(database.pending_local_sync_summary().unwrap().0, 515);
    let first = database.pending_local_mutations(1).unwrap()[0].mutation_id;
    database
        .reject_local_mutation(first, r#"{"accepted":false}"#.to_owned())
        .unwrap();
    assert_eq!(database.pending_local_sync_summary().unwrap().0, 514);
    let other = LibraryRootId::new();
    LibraryRepository::upsert(
        &database,
        &LibraryRoot::new(other, std::path::PathBuf::from("/other"), true, true),
    )
    .unwrap();
    database.select_sync_context(other).unwrap();
    assert_eq!(database.pending_local_sync_summary().unwrap().0, 0);
    database.select_sync_context(root).unwrap();
    assert_eq!(database.pending_local_sync_summary().unwrap().0, 514);
}

#[test]
fn queue_summary_deduplicates_immutable_resource_content() {
    use crate::domain::sync::{ResourceDigest, SharedResourceKind, SharedResourceManifest};
    let (_directory, database, root) = database();
    let id = SongId::new();
    let resource = SharedResourceManifest::new(
        SharedResourceKind::Audio,
        ResourceDigest::from_bytes([9; 32]),
        17,
        "audio/flac".into(),
        "song.flac",
    )
    .unwrap();
    database
        .set_local_sync_resources(ObjectKey::Song(id), vec![resource.clone()])
        .unwrap();
    let mut song = Song::new(
        id,
        root,
        RelativeMediaPath::new("song.flac").unwrap(),
        Revision::INITIAL,
    );
    SongRepository::upsert(&database, &song).unwrap();
    song.apply_metadata(Some("updated".into()), None, None, None);
    SongRepository::upsert(&database, &song).unwrap();
    let (count, resources) = database.pending_local_sync_summary().unwrap();
    assert_eq!(count, 2);
    assert_eq!(resources, vec![resource]);
}

/// A local sequence space can never be authoritative: the host's confirmed
/// cursor is. Re-anchoring renumbers the pending chain onto it without touching
/// a single library row or event identity.
#[test]
fn rebase_reanchors_the_pending_chain_onto_the_host_cursor() {
    use crate::domain::sync::{MutationId, OriginAnchor};

    let (_directory, database, root) = database();
    let song_id = SongId::new();
    let mut song = Song::new(
        song_id,
        root,
        RelativeMediaPath::new("artist/song.flac").unwrap(),
        Revision::INITIAL,
    );
    song.apply_metadata(Some("Song".to_owned()), None, None, None);
    SongRepository::upsert(&database, &song).unwrap();
    SongRepository::set_favorite(&database, song_id, true).unwrap();
    SongRepository::set_favorite(&database, song_id, false).unwrap();
    let queued = database.pending_local_mutations(10).unwrap();
    assert_eq!(queued.len(), 3);

    // The host already confirmed 57 events this database never saw, e.g. because
    // its ledger was rebuilt or the library root was re-bound.
    let host_last = MutationId::new();
    let anchor = OriginAnchor {
        origin_seq: 57,
        last_mutation_id: Some(host_last),
    };
    assert_eq!(database.rebase_local_chain(anchor).unwrap(), 3);

    let pending = database.pending_local_mutations(10).unwrap();
    assert_eq!(pending.len(), 3);
    assert_eq!(pending[0].origin_seq, 58);
    assert_eq!(pending[0].previous_mutation_id, Some(host_last));
    assert_eq!(pending[1].origin_seq, 59);
    assert_eq!(
        pending[1].previous_mutation_id,
        Some(pending[0].mutation_id)
    );
    assert_eq!(pending[2].origin_seq, 60);
    assert_eq!(
        pending[2].previous_mutation_id,
        Some(pending[1].mutation_id)
    );
    assert_eq!(pending[0].mutation_id, queued[0].mutation_id);
    assert_eq!(pending[0].payload, queued[0].payload);

    // Later local writes continue the re-anchored chain instead of restarting
    // the old, unreachable one.
    SongRepository::set_favorite(&database, song_id, true).unwrap();
    let continued = database.pending_local_mutations(10).unwrap();
    assert_eq!(continued.len(), 4);
    assert_eq!(continued[3].origin_seq, 61);
    assert_eq!(
        continued[3].previous_mutation_id,
        Some(pending[2].mutation_id)
    );
}

/// Re-anchoring must survive a target range that overlaps the current one.
///
/// `sync_mutations` is UNIQUE(local_root_uuid, origin_device_id, origin_seq)
/// across every status, so the naive "walk the pending rows and assign
/// `anchor + n`" renumbering collides the moment a target number is still held
/// by another row — exactly what happens when the host cursor is behind the
/// local chain, or when a rejected row sits inside the target range. The repair
/// then aborted with `UNIQUE constraint failed`, which is the worst possible
/// failure here: it is the code path that exists to end the stall.
#[test]
fn rebase_survives_a_target_range_that_overlaps_existing_sequences() {
    use crate::domain::sync::{MutationId, OriginAnchor};

    let (_directory, database, root) = database();
    let song_id = SongId::new();
    let mut song = Song::new(
        song_id,
        root,
        RelativeMediaPath::new("artist/overlap.flac").unwrap(),
        Revision::INITIAL,
    );
    song.apply_metadata(Some("Overlap".to_owned()), None, None, None);
    SongRepository::upsert(&database, &song).unwrap();
    SongRepository::set_favorite(&database, song_id, true).unwrap();
    SongRepository::set_favorite(&database, song_id, false).unwrap();
    let queued = database.pending_local_mutations(10).unwrap();
    assert_eq!(queued.len(), 3);

    // Retire the middle event so a terminal row sits inside the target range.
    database
        .reject_local_mutation(
            queued[1].mutation_id,
            serde_json::json!({"accepted": false, "reason": "Superseded"}).to_string(),
        )
        .unwrap();

    // The host is behind this chain: seq 1 is its head, so the two surviving
    // events must become 2 and 3 — both of which are already taken.
    let host_last = MutationId::new();
    let anchor = OriginAnchor {
        origin_seq: 1,
        last_mutation_id: Some(host_last),
    };
    assert_eq!(database.rebase_local_chain(anchor).unwrap(), 2);

    let pending = database.pending_local_mutations(10).unwrap();
    assert_eq!(pending.len(), 2);
    assert_eq!(pending[0].origin_seq, 2);
    assert_eq!(pending[0].previous_mutation_id, Some(host_last));
    assert_eq!(pending[1].origin_seq, 3);
    assert_eq!(
        pending[1].previous_mutation_id,
        Some(pending[0].mutation_id)
    );
    // Identity and content survive the renumbering; only the envelope moved.
    assert_eq!(pending[0].mutation_id, queued[0].mutation_id);
    assert_eq!(pending[0].payload, queued[0].payload);
    assert_eq!(pending[1].mutation_id, queued[2].mutation_id);
    assert_eq!(pending[1].payload, queued[2].payload);
    assert!(pending
        .iter()
        .all(|event| event.mutation_id != queued[1].mutation_id));
}

/// Re-recording a terminal event is a no-op, not a ledger failure.
///
/// The host can acknowledge the same mutation twice (a retried cycle, a
/// duplicate response). Looking up the row with `status='pending'` then finds
/// nothing, and mapping that absence to the storage variant told the user the
/// local ledger could not be read — while nothing was wrong at all.
#[test]
fn rejecting_an_already_terminal_mutation_is_idempotent() {
    let (_directory, database, root) = database();
    let song_id = SongId::new();
    let mut song = Song::new(
        song_id,
        root,
        RelativeMediaPath::new("artist/idempotent.flac").unwrap(),
        Revision::INITIAL,
    );
    song.apply_metadata(Some("Idempotent".to_owned()), None, None, None);
    SongRepository::upsert(&database, &song).unwrap();
    SongRepository::set_favorite(&database, song_id, true).unwrap();
    let queued = database.pending_local_mutations(10).unwrap();
    assert!(!queued.is_empty());
    let outcome = serde_json::json!({"accepted": false, "reason": "Superseded"}).to_string();

    database
        .reject_local_mutation(queued[0].mutation_id, outcome.clone())
        .unwrap();
    let remaining = database.pending_local_mutations(10).unwrap().len();
    // Second acknowledgement: must succeed and change nothing.
    database
        .reject_local_mutation(queued[0].mutation_id, outcome)
        .unwrap();
    let after = database.pending_local_mutations(10).unwrap();
    assert_eq!(after.len(), remaining);
    assert!(after
        .iter()
        .all(|event| event.mutation_id != queued[0].mutation_id));
}

#[test]
fn unchanged_reupsert_does_not_mint_another_candidate_or_advance_the_revision() {
    let (_directory, database, root) = database();
    let song_id = SongId::new();
    let mut song = Song::new(
        song_id,
        root,
        RelativeMediaPath::new("artist/unchanged.flac").unwrap(),
        Revision::INITIAL,
    );
    song.apply_metadata(
        Some("Unchanged".to_owned()),
        Some("Artist".to_owned()),
        None,
        None,
    );
    SongRepository::upsert(&database, &song).unwrap();
    let first = database.pending_local_mutations(10).unwrap();
    assert_eq!(first.len(), 1);

    // An unchanged rescan replays the same snapshot upsert. It must not mint
    // a second candidate, advance the object's local revision, or append
    // another outbox payload for the object.
    SongRepository::upsert(&database, &song).unwrap();

    let queued = database.pending_local_mutations(10).unwrap();
    assert_eq!(
        queued.len(),
        1,
        "an unchanged reupsert mints no new candidate"
    );
    assert_eq!(queued[0].mutation_id, first[0].mutation_id);
    let uuid = song_id.to_string();
    let ledger: (i64, i64) = database
        .writer
        .run(move |connection| {
            connection
                .query_row(
                    "SELECT (SELECT local_revision FROM sync_local_highwater WHERE object_type='song' AND object_uuid=?1), (SELECT COUNT(*) FROM sync_outbox WHERE object_type='song' AND object_uuid=?1)",
                    rusqlite::params![uuid],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(crate::infrastructure::sqlite::support::storage)
        })
        .unwrap();
    assert_eq!(
        ledger,
        (1, 1),
        "the highwater and outbox stay at the first candidate"
    );

    // Dedup is payload-exact, not a throttle: a real metadata change must
    // still mint a fresh candidate chained onto the first.
    let mut edited = song;
    edited.apply_metadata(Some("Renamed".to_owned()), None, None, None);
    SongRepository::upsert(&database, &edited).unwrap();
    let after = database.pending_local_mutations(10).unwrap();
    assert_eq!(after.len(), 2, "a changed reupsert still mints a candidate");
    assert_eq!(after[1].origin_seq, 2);
    assert_eq!(after[1].previous_mutation_id, Some(after[0].mutation_id));
}
