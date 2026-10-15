use crate::application::ports::{LibraryRepository, LocalSyncQueuePort, SongRepository};
use crate::domain::entities::{LibraryRoot, Song};
use crate::domain::ids::{LibraryRootId, RelativeMediaPath, Revision, SongId};
use crate::domain::sync::{AuthorityId, Epoch, LibraryId, ObjectKey, SyncBinding};
use crate::infrastructure::sqlite::SqliteDatabase;

fn database() -> (tempfile::TempDir, SqliteDatabase, LibraryRootId) {
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
    assert!(database
        .reject_local_mutation(
            queued[0].mutation_id,
            r#"{"accepted":false,"reason":"duplicate terminal"}"#.to_owned(),
        )
        .is_err());
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
    let (_directory, database, root) = database();
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
