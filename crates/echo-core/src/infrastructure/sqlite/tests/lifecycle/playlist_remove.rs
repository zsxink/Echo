//! A removed membership keeps its original identity and cannot delete a re-add.
use super::*;
use crate::application::ports::LibraryProjectionPort;

#[test]
fn library_durability_member_remove_is_saved_before_projection_and_readd_is_new() {
    let stack = ScanStack::new();
    let (_, control, deps) = configured_roots(&stack);
    let song = super::favorite::seed_song(&stack, &control, &deps);
    stack
        .database
        .writer
        .run(move |c| {
            c.execute(
                "UPDATE songs SET availability='available' WHERE uuid=?1",
                [song.to_string()],
            )
            .map_err(storage)?;
            Ok(())
        })
        .unwrap();
    let edits = DurableLibraryEdits {
        control: deps.control.as_ref(),
        songs: deps.songs.as_ref(),
        playlists: deps.playlists.as_ref(),
        unit: deps.uow.as_ref(),
        device: deps.device_id.current_device_id(),
    };
    let playlist = edits.create_playlist(stack.root, "remove target").unwrap();
    edits
        .add_to_playlists(stack.root, song, &[playlist])
        .unwrap();
    let original = deps.playlists.members(playlist).unwrap()[0].id();
    stack.database.writer.run(|c| c.execute_batch(
        "CREATE TRIGGER fail_remove_projection BEFORE INSERT ON library_projection_watermarks BEGIN SELECT RAISE(ABORT,'injected removal projection failure'); END;"
    ).map_err(storage)).unwrap();
    assert!(matches!(
        edits.remove_playlist_song(stack.root, playlist, song),
        Err(crate::error::Error::LibrarySavedPending { .. })
    ));
    assert_eq!(deps.playlists.members(playlist).unwrap()[0].id(), original);
    let replay = control.durability().unwrap().recover(stack.root).unwrap();
    let batch = &replay.commits.last().unwrap().batch;
    assert!(batch.records.iter().any(|envelope| matches!(envelope.record(), PortableRecord::Tombstone(t)
        if t.deleted_kind == crate::domain::library::RecordKind::PlaylistItem && t.object_uuid == original.as_uuid())));
    assert!(
        batch.state.is_empty(),
        "a standalone member removal retains no sync protocol state (SYN-R06-S04)"
    );
    stack
        .database
        .writer
        .run(|c| {
            c.execute_batch("DROP TRIGGER fail_remove_projection")
                .map_err(storage)
        })
        .unwrap();
    edits
        .create_playlist(stack.root, "catch up remove")
        .unwrap();
    assert!(deps.playlists.members(playlist).unwrap().is_empty());
    let before = control.durability().unwrap().recover(stack.root).unwrap();
    edits
        .remove_playlist_song(stack.root, playlist, song)
        .unwrap();
    let after = control.durability().unwrap().recover(stack.root).unwrap();
    assert_eq!(after, before, "removing an absent member is a no-op");
    edits
        .add_to_playlists(stack.root, song, &[playlist])
        .unwrap();
    let added = deps.playlists.members(playlist).unwrap()[0].id();
    assert_ne!(original, added);
    let replay = control.durability().unwrap().recover(stack.root).unwrap();
    for _ in 0..2 {
        stack
            .database
            .project_library_commits(stack.root, &replay)
            .unwrap();
        assert_eq!(deps.playlists.members(playlist).unwrap()[0].id(), added);
    }
    // A wiped projection replays the removal then the re-add: the rebuilt
    // membership carries the re-added identity, never the removed one.
    let (_wipe, rebuilt) = super::fresh_database(&stack);
    for _ in 0..2 {
        rebuilt
            .project_library_commits(stack.root, &replay)
            .unwrap();
        let actual =
            crate::application::ports::PlaylistRepository::members(&rebuilt, playlist).unwrap();
        assert_eq!(actual.len(), 1);
        assert_eq!(actual[0].id().as_uuid(), added.as_uuid());
    }
}
