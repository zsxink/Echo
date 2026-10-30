//! LD-R11-S01: a root that was never bound to LAN sync must not keep the
//! protocol facts an earlier release projected into SQLite. Reopening the same
//! verified head purges the detached queue, cursors, tombstones and sync state
//! evidence, while the catalog records and the media tree stay untouched.
use super::*;
use crate::domain::entities::SongAvailability;
use crate::domain::library::durability::{DurableStateChange, StateKind};

const AUDIO: &[u8] = b"STANDALONE AUDIO BYTES";
const MEDIA_PATH: &str = "media/歌手/晴天.flac";

const PROTOCOL_TABLES: [&str; 4] = [
    "sync_outbox",
    "sync_mutations",
    "sync_local_origin_cursors",
    "sync_epoch_tombstones",
];

fn count_rows(db: &SqliteDatabase, root: LibraryRootId, table: &str) -> u64 {
    db.with_reader(|connection| {
        connection
            .query_row(
                &format!("SELECT COUNT(*) FROM {table} WHERE local_root_uuid=?1"),
                [root.to_string()],
                |row| row.get(0),
            )
            .map_err(storage)
    })
    .unwrap()
}

fn retained_state_rows(db: &SqliteDatabase, root: LibraryRootId) -> u64 {
    db.with_reader(|connection| {
        connection
            .query_row(
                "SELECT COUNT(*) FROM library_recovery_records WHERE library_root_uuid=?1 AND row_kind='state'",
                [root.to_string()],
                |row| row.get(0),
            )
            .map_err(storage)
    })
    .unwrap()
}

struct Catalog {
    audio: &'static [u8],
    media: std::path::PathBuf,
    song_uuid: uuid::Uuid,
    playlist_uuid: uuid::Uuid,
    member_uuid: uuid::Uuid,
    replay: LibraryReplay,
}

fn seed_catalog(
    directory: &tempfile::TempDir,
    db: &SqliteDatabase,
    root: LibraryRootId,
) -> Catalog {
    let library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let device = DeviceId::from_uuid(uuid::Uuid::from_u128(1));
    let song_uuid = uuid::Uuid::from_u128(100);
    let playlist_uuid = uuid::Uuid::from_u128(500);
    let member_uuid = uuid::Uuid::from_u128(700);
    let media = directory.path().join("library").join(MEDIA_PATH);
    std::fs::create_dir_all(media.parent().unwrap()).unwrap();
    std::fs::write(&media, AUDIO).unwrap();

    let mut replay = LibraryReplay::default();
    append(
        &mut replay,
        library,
        vec![
            PortableRecord::Song(SongRecord {
                song_uuid,
                revision: Revision::INITIAL,
                updated_by_device_id: device,
                hlc: HybridLogicalClock::new(1_700_000_000, 0),
                media_path: LibraryRelativePath::new(MEDIA_PATH).unwrap(),
                content_hash: blake3::hash(AUDIO).to_hex().to_string(),
                added_at: 1_700_000_000_000,
                title: Some("晴天".into()),
                artist: Some("周杰伦".into()),
                album: None,
            }),
            PortableRecord::Favorite(crate::domain::library::FavoriteRecord {
                song_uuid,
                revision: Revision::INITIAL,
                updated_by_device_id: device,
                hlc: HybridLogicalClock::new(1_700_000_000, 0),
                is_favorite: true,
                favorited_at: Some(1_700_000_000_000),
            }),
            PortableRecord::Playlist(PlaylistRecord {
                playlist_uuid,
                revision: Revision::INITIAL,
                updated_by_device_id: device,
                hlc: HybridLogicalClock::new(1_700_000_000, 0),
                display_name: "通勤".into(),
                created_at: Some(1_700_000_000_000),
                cover_asset: None,
            }),
            PortableRecord::PlaylistItem(PlaylistItemRecord {
                item_uuid: member_uuid,
                revision: Revision::INITIAL,
                updated_by_device_id: device,
                hlc: HybridLogicalClock::new(1_700_000_000, 0),
                playlist_uuid,
                song_uuid,
                position: 0,
                added_at: Some(1_700_000_050_000),
            }),
        ],
    );
    db.project_library_commits(root, &replay).unwrap();
    db.writer
        .run(move |connection| {
            connection
                .execute(
                    "UPDATE songs SET availability='available' WHERE uuid=?1",
                    [song_uuid.to_string()],
                )
                .map_err(storage)?;
            Ok(())
        })
        .unwrap();
    Catalog {
        audio: AUDIO,
        media,
        song_uuid,
        playlist_uuid,
        member_uuid,
        replay,
    }
}

/// Rows an earlier synced release could have left in a root that is standalone
/// today: a detached queue, cursor, tombstone and the legacy sync state
/// evidence, plus one completed publication that must survive.
fn insert_detached_protocol(db: &SqliteDatabase, root: LibraryRootId, song_uuid: uuid::Uuid) {
    let legacy_state = serde_json::to_string(&DurableStateChange::new(
        StateKind::Pending,
        uuid::Uuid::from_u128(0xaa),
        serde_json::json!({"schema":"echo.sync.mutation.v1","payload":{"title":"legacy"}}),
    ))
    .unwrap();
    let remote_publication = uuid::Uuid::from_u128(0xcc).to_string();
    db.writer
        .run(move |connection| {
            let mutation_id = uuid::Uuid::from_u128(0xbb).to_string();
            connection
                .execute(
                    "INSERT INTO sync_outbox(object_type,object_uuid,revision,operation,payload_json,created_at,mutation_id,local_root_uuid) VALUES('song',?1,1,'upsert','{}',1,?2,?3)",
                    rusqlite::params![song_uuid.to_string(), mutation_id, root.to_string()],
                )
                .map_err(storage)?;
            connection
                .execute(
                    "INSERT INTO sync_mutations(mutation_id,origin_device_id,origin_seq,object_type,object_uuid,operation,local_revision,payload_json,payload_digest,status,created_at,local_root_uuid) VALUES(?1,'device-a',1,'song',?2,'upsert',1,'{}',?3,'pending',1,?4)",
                    rusqlite::params![
                        mutation_id,
                        song_uuid.to_string(),
                        "0".repeat(64),
                        root.to_string()
                    ],
                )
                .map_err(storage)?;
            connection
                .execute(
                    "INSERT INTO sync_local_origin_cursors(local_root_uuid,origin_device_id,origin_seq,last_mutation_id) VALUES(?1,'device-a',1,?2)",
                    rusqlite::params![root.to_string(), mutation_id],
                )
                .map_err(storage)?;
            connection
                .execute(
                    "INSERT INTO sync_epoch_tombstones(local_root_uuid,object_type,object_uuid) VALUES(?1,'song',?2)",
                    rusqlite::params![root.to_string(), song_uuid.to_string()],
                )
                .map_err(storage)?;
            // A completed publication records where synced media actually
            // lives and must survive the standalone purge (LD-08).
            connection
                .execute(
                    "INSERT INTO sync_remote_publications(operation_uuid,local_root_uuid,object_type,object_uuid,declaration_json,publication_json,completed) VALUES(?1,?2,'song',?3,'{}','{}',1)",
                    rusqlite::params![
                        remote_publication,
                        root.to_string(),
                        song_uuid.to_string()
                    ],
                )
                .map_err(storage)?;
            connection
                .execute(
                    "INSERT INTO library_recovery_records(library_root_uuid,object_key,envelope_json,row_kind) VALUES(?1,'legacy-sync-state',?2,'state')",
                    rusqlite::params![root.to_string(), legacy_state],
                )
                .map_err(storage)?;
            Ok(())
        })
        .unwrap();
    for table in PROTOCOL_TABLES {
        assert!(count_rows(db, root, table) > 0, "{table} fixture");
    }
}

fn assert_detached_protocol_purged(db: &SqliteDatabase, root: LibraryRootId) {
    for table in PROTOCOL_TABLES {
        assert_eq!(
            count_rows(db, root, table),
            0,
            "{table} must not keep detached protocol facts"
        );
    }
    assert_eq!(retained_state_rows(db, root), 0);
}

fn assert_catalog_preserved(db: &SqliteDatabase, root: LibraryRootId, catalog: &Catalog) {
    // Completed publications and the media tree are catalog evidence, not
    // protocol work: both survive byte for byte.
    let publications: u64 = db
        .with_reader(|connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM sync_remote_publications WHERE local_root_uuid=?1 AND completed=1",
                    [root.to_string()],
                    |row| row.get(0),
                )
                .map_err(storage)
        })
        .unwrap();
    assert_eq!(publications, 1);
    assert_eq!(std::fs::read(&catalog.media).unwrap(), catalog.audio);

    let song = SongRepository::by_id(db, SongId::from_uuid(catalog.song_uuid))
        .unwrap()
        .unwrap();
    assert_eq!(song.title(), Some("晴天"));
    assert!(song.favorite());
    assert_eq!(song.availability(), SongAvailability::Available);
    let members =
        PlaylistRepository::members(db, PlaylistId::from_uuid(catalog.playlist_uuid)).unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].id().as_uuid(), catalog.member_uuid);
    assert_eq!(members[0].song().as_uuid(), catalog.song_uuid);
}

#[test]
fn library_durability_standalone_startup_purges_legacy_sync_projection() {
    let (directory, db, root) = database();
    let catalog = seed_catalog(&directory, &db, root);
    let before = db.read_recovery_records(root).unwrap();
    insert_detached_protocol(&db, root, catalog.song_uuid);
    assert_eq!(retained_state_rows(&db, root), 1, "sync state fixture");

    // Reopening the same verified head is what a standalone startup does: the
    // projection has nothing new to apply, so `retained::retry` runs the purge.
    db.project_library_commits(root, &catalog.replay).unwrap();

    assert_detached_protocol_purged(&db, root);
    let after = db.read_recovery_records(root).unwrap();
    assert_eq!(after.records, before.records);
    assert_eq!(after, before);
    assert_catalog_preserved(&db, root, &catalog);
}
