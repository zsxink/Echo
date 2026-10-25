//! Renames must use the same ordered projection path as playlist creation.
use super::*;
use crate::application::ports::LibraryProjectionPort;

#[allow(clippy::too_many_lines)]
#[test]
fn library_durability_playlist_rename_saves_before_failed_projection() {
    let stack = ScanStack::new();
    let (_, control, deps) = configured_roots(&stack);
    let edits = DurableLibraryEdits {
        control: deps.control.as_ref(),
        songs: deps.songs.as_ref(),
        playlists: deps.playlists.as_ref(),
        unit: deps.uow.as_ref(),
        device: deps.device_id.current_device_id(),
    };
    let playlist = edits.create_playlist(stack.root, "original name").unwrap();
    stack
        .database
        .writer
        .run(|connection| {
            connection.execute_batch(
            "CREATE TRIGGER fail_rename_projection BEFORE INSERT ON library_projection_watermarks
             BEGIN SELECT RAISE(ABORT, 'injected rename projection failure'); END;",
        ).map_err(storage)
        })
        .unwrap();
    let error = edits
        .rename_playlist(stack.root, playlist, "saved rename")
        .unwrap_err();
    assert!(matches!(
        error,
        crate::error::Error::LibrarySavedPending { .. }
    ));
    assert_eq!(
        deps.playlists.name(playlist).unwrap().as_deref(),
        Some("original name")
    );
    let replay = control.durability().unwrap().recover(stack.root).unwrap();
    assert_eq!(replay.commits.len(), 2);
    let PortableRecord::Playlist(record) = replay.commits[1].batch.records[0].record() else {
        panic!("playlist after-image expected")
    };
    assert_eq!(record.display_name, "saved rename");
    let PortableRecord::Playlist(original) = replay.commits[0].batch.records[0].record() else {
        panic!("original playlist expected")
    };
    assert_eq!(record.created_at, original.created_at);
    assert_eq!(record.revision.as_u64(), 2);
    assert!(record.hlc > original.hlc);
    let frozen: Vec<crate::domain::library::durability::FrozenMutation> = replay
        .states()
        .filter(|state| {
            matches!(
                state.key.as_ref().map(|key| &key.target),
                Some(crate::domain::library::durability::StateTarget::Mutation { .. })
            )
        })
        .map(|state| serde_json::from_value(state.payload["frozen"].clone()).unwrap())
        .collect();
    assert_eq!(frozen.len(), 2);
    assert_eq!(frozen[1].candidate.origin_seq, 2);
    assert_eq!(
        frozen[1].candidate.previous_mutation_id,
        Some(frozen[0].candidate.mutation_id)
    );
    assert_eq!(frozen[1].local_revision.as_u64(), 2);
    assert_eq!(frozen[1].candidate.payload["display_name"], "saved rename");
    assert_eq!(
        frozen[1].candidate.payload["created_at"].as_u64(),
        original.created_at
    );
    stack
        .database
        .writer
        .run(|connection| {
            connection
                .execute_batch("DROP TRIGGER fail_rename_projection")
                .map_err(storage)
        })
        .unwrap();
    edits
        .rename_playlist(stack.root, playlist, "latest rename")
        .unwrap();
    let replay = control.durability().unwrap().recover(stack.root).unwrap();
    for _ in 0..2 {
        crate::application::ports::LibraryProjectionPort::project_library_commits(
            stack.database.as_ref(),
            stack.root,
            &replay,
        )
        .unwrap();
        assert_eq!(
            deps.playlists.name(playlist).unwrap().as_deref(),
            Some("latest rename"),
        );
    }
    // A wiped projection rebuilds the rename from the directory alone.
    let (_wipe, rebuilt) = super::fresh_database(&stack);
    for _ in 0..2 {
        rebuilt
            .project_library_commits(stack.root, &replay)
            .unwrap();
        assert_eq!(
            crate::application::ports::PlaylistRepository::name(&rebuilt, playlist)
                .unwrap()
                .as_deref(),
            Some("latest rename"),
        );
    }
}
