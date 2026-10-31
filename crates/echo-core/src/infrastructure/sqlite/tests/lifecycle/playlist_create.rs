//! A playlist creation saved in the directory but not yet projected must keep
//! its operation identity when the user retries, instead of looking like a
//! brand-new name conflict.
use super::*;
use crate::application::ports::LibraryProjectionPort;

/// Pins the required behaviour: a creation that is already saved in the
/// directory must keep its original operation identity while the projection is
/// still broken, and must complete that same creation once the projection
/// catches up instead of minting a second playlist.
///
/// The retry currently loses the saved identity: the coordinator reports the
/// raw projection failure, and after the projection is repaired the planning
/// insert hits the unique name index. See
/// `openspec/changes/archive/2026-10-31-enforce-library-first-durability/evidence/handoff-20261025.md`.
#[test]
fn library_durability_playlist_create_retry_after_saved_pending_keeps_operation_identity() {
    let stack = ScanStack::new();
    let (_, control, deps) = configured_roots(&stack);
    let edits = DurableLibraryEdits {
        control: deps.control.as_ref(),
        songs: deps.songs.as_ref(),
        playlists: deps.playlists.as_ref(),
        unit: deps.uow.as_ref(),
        device: deps.device_id.current_device_id(),
    };
    stack
        .database
        .writer
        .run(|connection| {
            connection.execute_batch(
            "CREATE TRIGGER fail_create_projection BEFORE INSERT ON library_projection_watermarks
             BEGIN SELECT RAISE(ABORT, 'injected create projection failure'); END;",
        ).map_err(storage)
        })
        .unwrap();
    let first = edits.create_playlist(stack.root, "same name").unwrap_err();
    let crate::error::Error::LibrarySavedPending { operation, .. } = &first else {
        panic!("expected a saved-pending failure, got {first:?}");
    };
    let operation = *operation;
    // The retry still fails to project, so it must report the very same saved
    // operation instead of a fresh unsaved failure.
    let retry = edits.create_playlist(stack.root, "same name").unwrap_err();
    let crate::error::Error::LibrarySavedPending {
        operation: retried, ..
    } = &retry
    else {
        panic!("retrying a saved creation must report the same saved operation, got {retry:?}");
    };
    assert_eq!(
        *retried, operation,
        "the retry must reuse the original directory operation identity"
    );
    let replay = control.durability().unwrap().recover(stack.root).unwrap();
    assert_eq!(
        replay.commits.len(),
        1,
        "a retry must not add a second directory commit for the same playlist"
    );
    let saved = replay.commits[0].batch.records[0].record().object_uuid();
    // Once the projection catches up, the retry completes the saved creation
    // instead of creating a second playlist under the same name.
    stack
        .database
        .writer
        .run(|connection| {
            connection
                .execute_batch("DROP TRIGGER fail_create_projection")
                .map_err(storage)
        })
        .unwrap();
    let completed = edits.create_playlist(stack.root, "same name").unwrap();
    assert_eq!(
        completed.as_uuid(),
        saved,
        "the retry must return the identity of the saved creation"
    );
    assert_eq!(
        deps.playlists.name(completed).unwrap().as_deref(),
        Some("same name"),
        "the retried creation must be visible in the projection"
    );
    let replay = control.durability().unwrap().recover(stack.root).unwrap();
    assert_eq!(
        replay.commits.len(),
        1,
        "a caught-up retry must not add a second directory commit"
    );
    // A wiped projection rebuilds the empty playlist from the directory alone,
    // keeping the saved identity and name.
    let (_wipe, rebuilt) = super::fresh_database(&stack);
    for _ in 0..2 {
        rebuilt
            .project_library_commits(stack.root, &replay)
            .unwrap();
        assert_eq!(
            crate::application::ports::PlaylistRepository::name(&rebuilt, completed)
                .unwrap()
                .as_deref(),
            Some("same name")
        );
        assert_eq!(
            crate::application::ports::PlaylistRepository::members(&rebuilt, completed)
                .unwrap()
                .len(),
            0
        );
    }
}
