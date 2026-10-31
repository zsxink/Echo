//! A playlist cascade is saved as one set before the ordered SQL projection.
use super::*;
use crate::application::ports::LibraryProjectionPort;
use crate::domain::library::durability::{ChangeOrigin, LibraryBatch, LibraryReplay};
use crate::domain::library::{HybridLogicalClock, PlaylistItemRecord, PortableRecordEnvelope};

fn assert_fresh_projection(
    stack: &ScanStack,
    replay: &LibraryReplay,
    playlist: crate::domain::ids::PlaylistId,
) {
    let directory = tempfile::tempdir().unwrap();
    let rebuilt =
        crate::infrastructure::sqlite::SqliteDatabase::open(directory.path().join("fresh.sqlite"))
            .unwrap();
    LibraryRepository::upsert(
        &rebuilt,
        &LibraryRoot::new(stack.root, stack.library_dir.clone(), true, true),
    )
    .unwrap();
    for _ in 0..2 {
        rebuilt.project_library_commits(stack.root, replay).unwrap();
        assert!(
            crate::application::ports::PlaylistRepository::name(&rebuilt, playlist)
                .unwrap()
                .is_none()
        );
        let counts: (u64, u64, u64) = rebuilt.with_reader(|c| {
            c.query_row("SELECT (SELECT COUNT(*) FROM playlist_songs), (SELECT COUNT(*) FROM sync_mutations), (SELECT COUNT(*) FROM sync_outbox)", [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?))).map_err(storage)
        }).unwrap();
        assert_eq!(counts, (0, 0, 0));
    }
}

#[test]
fn library_durability_playlist_delete_saves_cascade_before_projection() {
    let stack = ScanStack::new();
    let (_, control, deps) = configured_roots(&stack);
    let edits = DurableLibraryEdits {
        control: deps.control.as_ref(),
        songs: deps.songs.as_ref(),
        playlists: deps.playlists.as_ref(),
        unit: deps.uow.as_ref(),
        device: deps.device_id.current_device_id(),
    };
    let playlist = edits.create_playlist(stack.root, "delete this").unwrap();
    let library = control
        .read_manifest(stack.root)
        .unwrap()
        .unwrap()
        .library_id;
    let member = crate::domain::ids::PlaylistItemId::new();
    let record = PortableRecord::PlaylistItem(PlaylistItemRecord {
        item_uuid: member.as_uuid(),
        playlist_uuid: playlist.as_uuid(),
        song_uuid: uuid::Uuid::new_v4(),
        position: 3,
        added_at: Some(123),
        revision: crate::domain::ids::Revision::from_u64(1),
        hlc: HybridLogicalClock::new(1, 0),
        updated_by_device_id: deps.device_id.current_device_id(),
    });
    control
        .durability()
        .unwrap()
        .commit(
            stack.root,
            &LibraryBatch::new(
                uuid::Uuid::new_v4(),
                library,
                ChangeOrigin::LocalEdit,
                vec![PortableRecordEnvelope::new(record, None).unwrap()],
            ),
        )
        .unwrap();
    let replay = control.durability().unwrap().recover(stack.root).unwrap();
    stack
        .database
        .project_library_commits(stack.root, &replay)
        .unwrap();
    stack.database.writer.run(|c| c.execute_batch(
        "CREATE TRIGGER fail_delete_projection BEFORE INSERT ON library_projection_watermarks BEGIN SELECT RAISE(ABORT,'injected delete projection failure'); END;"
    ).map_err(storage)).unwrap();
    assert!(matches!(
        edits.delete_playlist(stack.root, playlist),
        Err(crate::error::Error::LibrarySavedPending { .. })
    ));
    assert!(deps.playlists.name(playlist).unwrap().is_some());
    let replay = control.durability().unwrap().recover(stack.root).unwrap();
    let saved = &replay.commits.last().unwrap().batch;
    for (kind, id) in [
        (
            crate::domain::library::RecordKind::Playlist,
            playlist.as_uuid(),
        ),
        (
            crate::domain::library::RecordKind::PlaylistItem,
            member.as_uuid(),
        ),
    ] {
        assert!(saved.records.iter().any(|envelope| matches!(envelope.record(), PortableRecord::Tombstone(t) if t.deleted_kind == kind && t.object_uuid == id)));
    }
    assert!(
        saved.state.is_empty(),
        "a standalone cascade delete retains no sync protocol state (SYN-R06-S04)"
    );
    stack
        .database
        .writer
        .run(|c| {
            c.execute_batch("DROP TRIGGER fail_delete_projection")
                .map_err(storage)
        })
        .unwrap();
    edits.create_playlist(stack.root, "later edit").unwrap();
    let replay = control.durability().unwrap().recover(stack.root).unwrap();
    assert_fresh_projection(&stack, &replay, playlist);
    assert!(deps.playlists.name(playlist).unwrap().is_none());
    for _ in 0..2 {
        stack
            .database
            .project_library_commits(
                stack.root,
                &control.durability().unwrap().recover(stack.root).unwrap(),
            )
            .unwrap();
        assert!(deps.playlists.name(playlist).unwrap().is_none());
    }
}
