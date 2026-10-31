//! Member edits freeze identity, ordering and candidates before SQL projection.
use super::*;
use crate::application::ports::LibraryProjectionPort;
use crate::domain::library::durability::{ProjectionTarget, ProjectionUnresolvedReason};
use crate::domain::library::PortableRecordEnvelope;
use crate::domain::sync::ObjectKey;
use rusqlite::OptionalExtension;

#[allow(clippy::too_many_lines)]
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
    assert!(
        batch.state.is_empty(),
        "a standalone member append retains no sync protocol state (SYN-R06-S04)"
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
    // A wiped projection rebuilds both memberships from the directory alone,
    // keeping the frozen member identities, positions and ordering times.
    let (_wipe, rebuilt) = super::fresh_database(&stack);
    for _ in 0..2 {
        rebuilt
            .project_library_commits(stack.root, &replay)
            .unwrap();
        for item in &members {
            let actual = crate::application::ports::PlaylistRepository::members(
                &rebuilt,
                crate::domain::ids::PlaylistId::from_uuid(item.playlist_uuid),
            )
            .unwrap();
            assert_eq!(actual.len(), 1);
            assert_eq!(actual[0].id().as_uuid(), item.item_uuid);
            assert_eq!(actual[0].position(), item.position);
        }
    }
}

#[test]
#[allow(clippy::too_many_lines)]
fn library_durability_retained_member_with_blocked_reorder_keeps_projected_row() {
    let stack = ScanStack::new();
    let (_, control, deps) = configured_roots(&stack);
    let first_song = super::favorite::seed_song_at(&stack, &control, &deps, "media/member-a.flac");
    let second_song = super::favorite::seed_song_at(&stack, &control, &deps, "media/member-b.flac");
    for song in [first_song, second_song] {
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
    }
    let edits = DurableLibraryEdits {
        control: deps.control.as_ref(),
        songs: deps.songs.as_ref(),
        playlists: deps.playlists.as_ref(),
        unit: deps.uow.as_ref(),
        device: deps.device_id.current_device_id(),
    };
    let playlist = edits
        .create_playlist(stack.root, "blocked reorder")
        .unwrap();
    edits
        .add_to_playlists(stack.root, first_song, &[playlist])
        .unwrap();
    edits
        .add_to_playlists(stack.root, second_song, &[playlist])
        .unwrap();
    let members = deps.playlists.members(playlist).unwrap();
    assert_eq!(members.len(), 2);
    let (first, second) = (members[0].id(), members[1].id());
    // Simulate a saved reorder after-image for the first member that now
    // targets the position the second member still occupies: the retained
    // record is pending with an identity conflict, and retrying must keep the
    // projected row instead of discarding a fact it cannot replace (LD-08).
    let root = stack.root;
    stack
        .database
        .writer
        .run(move |c| {
            let key: Option<String> = c
                .query_row(
                    "SELECT object_key FROM library_recovery_records WHERE library_root_uuid=?1 AND row_kind='record' AND envelope_json LIKE '%'||?2||'%'",
                    rusqlite::params![root.to_string(), first.as_uuid().to_string()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(storage)?;
            let key =
                key.ok_or_else(|| storage(rusqlite::Error::QueryReturnedNoRows))?;
            let json: String = c
                .query_row(
                    "SELECT envelope_json FROM library_recovery_records WHERE library_root_uuid=?1 AND object_key=?2",
                    rusqlite::params![root.to_string(), key],
                    |row| row.get(0),
                )
                .map_err(storage)?;
            let envelope: PortableRecordEnvelope =
                serde_json::from_str(&json).map_err(storage)?;
            let PortableRecord::PlaylistItem(mut item) = envelope.record().clone() else {
                panic!("the retained record is not a playlist member");
            };
            item.position = 1;
            let blocked = PortableRecordEnvelope::new(
                PortableRecord::PlaylistItem(item),
                envelope.sync_metadata().cloned(),
            )
            .map_err(storage)?;
            c.execute(
                "UPDATE library_recovery_records SET envelope_json=?3, unresolved_reason=?4 WHERE library_root_uuid=?1 AND object_key=?2",
                rusqlite::params![
                    root.to_string(),
                    key,
                    serde_json::to_string(&blocked).map_err(storage)?,
                    serde_json::to_string(&ProjectionUnresolvedReason::IdentityConflict)
                        .map_err(storage)?,
                ],
            )
            .map_err(storage)?;
            Ok(())
        })
        .unwrap();
    let replay = control.durability().unwrap().recover(stack.root).unwrap();
    for _ in 0..2 {
        stack
            .database
            .project_library_commits(stack.root, &replay)
            .unwrap();
        let after = deps.playlists.members(playlist).unwrap();
        assert_eq!(
            after.len(),
            2,
            "a retained member keeps its projected row while its reorder is blocked"
        );
        assert_eq!(after[0].id().as_uuid(), first.as_uuid());
        assert_eq!(after[0].position(), 0);
        assert_eq!(after[1].id().as_uuid(), second.as_uuid());
        assert_eq!(after[1].position(), 1);
        let status = stack.database.projection_status(stack.root).unwrap();
        assert!(
            status.unresolved.iter().any(|entry| matches!(
                (&entry.target, &entry.reason),
                (
                    ProjectionTarget::Record {
                        object: ObjectKey::PlaylistItem(id)
                    },
                    ProjectionUnresolvedReason::IdentityConflict
                ) if *id == first
            )),
            "{status:?}"
        );
    }
}
