//! Member edits freeze identity, ordering and candidates before SQL projection.
use super::*;
use crate::application::ports::LibraryProjectionPort;
use crate::domain::library::durability::{FrozenMutation, StateTarget};

fn frozen_mutations(
    batch: &crate::domain::library::durability::LibraryBatch,
) -> Vec<FrozenMutation> {
    batch
        .state
        .iter()
        .filter(|state| {
            matches!(
                state.key.as_ref().map(|key| &key.target),
                Some(StateTarget::Mutation { .. })
            )
        })
        .map(|state| serde_json::from_value(state.payload["frozen"].clone()).unwrap())
        .collect()
}

#[test]
fn library_durability_member_append_is_saved_before_failed_projection() {
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
    let first = edits.create_playlist(stack.root, "first target").unwrap();
    let second = edits.create_playlist(stack.root, "second target").unwrap();
    stack.database.writer.run(|c| c.execute_batch(
        "CREATE TRIGGER fail_member_projection BEFORE INSERT ON library_projection_watermarks BEGIN SELECT RAISE(ABORT,'injected member projection failure'); END;"
    ).map_err(storage)).unwrap();
    assert!(matches!(
        edits.add_to_playlists(stack.root, song, &[first, second, first]),
        Err(crate::error::Error::LibrarySavedPending { .. })
    ));
    assert!(deps.playlists.members(first).unwrap().is_empty());
    let replay = control.durability().unwrap().recover(stack.root).unwrap();
    let batch = &replay.commits.last().unwrap().batch;
    let members: Vec<_> = batch
        .records
        .iter()
        .filter_map(|envelope| match envelope.record() {
            PortableRecord::PlaylistItem(item) => Some(item.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(members.len(), 2);
    assert!(members
        .iter()
        .all(|item| item.position == 0 && item.added_at.is_some()));
    assert_ne!(members[0].item_uuid, members[1].item_uuid);
    assert_eq!(batch.state.len(), 4);
    let frozen = frozen_mutations(batch);
    assert_eq!(frozen.len(), 2);
    assert_eq!(frozen[0].candidate.origin_seq, 3);
    assert_eq!(frozen[1].candidate.origin_seq, 4);
    assert_eq!(
        frozen[1].candidate.previous_mutation_id,
        Some(frozen[0].candidate.mutation_id)
    );
    stack
        .database
        .writer
        .run(|c| {
            c.execute_batch("DROP TRIGGER fail_member_projection")
                .map_err(storage)
        })
        .unwrap();
    edits.create_playlist(stack.root, "catch up").unwrap();
    let before = control
        .durability()
        .unwrap()
        .recover(stack.root)
        .unwrap()
        .commits
        .len();
    edits
        .add_to_playlists(stack.root, song, &[first, second, first])
        .unwrap();
    let replay = control.durability().unwrap().recover(stack.root).unwrap();
    assert_eq!(
        replay.commits.len(),
        before,
        "duplicate append is not a new edit"
    );
    for _ in 0..2 {
        stack
            .database
            .project_library_commits(stack.root, &replay)
            .unwrap();
        for item in &members {
            let actual = deps
                .playlists
                .members(crate::domain::ids::PlaylistId::from_uuid(
                    item.playlist_uuid,
                ))
                .unwrap();
            assert_eq!(actual.len(), 1);
            assert_eq!(actual[0].id().as_uuid(), item.item_uuid);
            assert_eq!(actual[0].position(), item.position);
        }
    }
}
