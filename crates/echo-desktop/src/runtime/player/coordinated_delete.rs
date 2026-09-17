//! Player-aware coordinated delete (task 8.11).
//!
//! Deleting the currently-playing song goes through the [`DeletionCoordinator`]
//! so the queue is unloaded from the actor before Core deletes the file and is
//! rolled back on a Core refusal.

use std::sync::{Arc, Mutex};

use echo_core::domain::ids::SongId;
use echo_core::domain::state::PlaybackState;

use super::PlaybackCoordinator;
use super::PlayerPort;
use crate::player::deletion::{DeleteCommit, DeleteExecutor, DeletionCoordinator, UnloadOutcome};

/// The Core-delete boundary for [`DeletionCoordinator`]: the closure performs
/// the real delete and returns the undo-operation id, which is captured into a
/// shared slot the caller reads after a commit (a commit implies the id exists).
struct CoreDelete<F: Fn(SongId) -> Result<String, echo_core::error::Error> + Send + Sync> {
    delete_core: F,
    operation: Arc<Mutex<Option<String>>>,
}

impl<F: Fn(SongId) -> Result<String, echo_core::error::Error> + Send + Sync> DeleteExecutor
    for CoreDelete<F>
{
    fn hide_for_delete(&self, song: SongId) -> Result<(), String> {
        match (self.delete_core)(song) {
            Ok(operation) => {
                if let Ok(mut slot) = self.operation.lock() {
                    *slot = Some(operation);
                }
                Ok(())
            }
            Err(error) => Err(error.to_string()),
        }
    }
}

/// Wait for the actor to confirm the current file is released after the
/// coordinated delete's `Stop` (task 8.11 unload barrier). Checks the live
/// snapshot first (the Stop may already have been processed), then the stream,
/// so a snapshot published between `send` and `subscribe` cannot be missed.
fn wait_unload(port: &Arc<dyn PlayerPort>, timeout: std::time::Duration) -> UnloadOutcome {
    let released = |state: PlaybackState| {
        matches!(
            state,
            PlaybackState::Stopped | PlaybackState::Ended | PlaybackState::Failed
        )
    };
    if released(port.snapshot().state) {
        return UnloadOutcome::Confirmed;
    }
    let rx = port.subscribe_snapshots();
    let deadline = std::time::Instant::now() + timeout;
    while let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) {
        match rx.recv_timeout(remaining) {
            Ok(snap) if released(snap.state) => return UnloadOutcome::Confirmed,
            Ok(_) => {}
            Err(_) => break,
        }
    }
    UnloadOutcome::TimedOut
}

/// Delete one song through the [`DeletionCoordinator`] (task 8.11) instead of
/// calling Core directly (which used to bypass the player entirely: deleting
/// the currently-playing song kept the queue showing a track that no longer
/// exists, with no unload barrier before the file operation).
///
/// The whole coordination happens under the coordinator lock the command layer
/// already uses, so no command can interleave with the snapshot/commit/rollback.
/// `delete_core` runs the real delete (Core `DeleteSongs`) and returns the
/// undo-operation id.
///
/// # Errors
///
/// A rolled-back delete (unload timeout / Core refusal) surfaces as `Err` with
/// the reason — never a fabricated success.
pub fn delete_song_coordinated(
    coordinator: &Arc<Mutex<PlaybackCoordinator<Arc<dyn PlayerPort>>>>,
    song: SongId,
    delete_core: impl Fn(SongId) -> Result<String, echo_core::error::Error> + Send + Sync,
    unload_timeout: std::time::Duration,
) -> Result<String, String> {
    // The player port outlives the guard (used inside the unload closure).
    let port = {
        let coord = coordinator
            .lock()
            .map_err(|_| "player coordinator poisoned")?;
        coord.player().clone()
    };
    let operation: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let executor = CoreDelete {
        delete_core,
        operation: Arc::clone(&operation),
    };
    let mut deletion = DeletionCoordinator::new(port.clone(), executor);
    let mut coord = coordinator
        .lock()
        .map_err(|_| "player coordinator poisoned")?;
    let mode = coord.mode();
    match deletion.delete_song(coord.queue_mut(), mode, song, || {
        wait_unload(&port, unload_timeout)
    }) {
        Ok(DeleteCommit::Committed) => Ok(operation
            .lock()
            .ok()
            .and_then(|mut slot| slot.take())
            .unwrap_or_default()),
        Ok(DeleteCommit::RolledBack) => {
            Err("delete rolled back: the player still holds the file or Core refused".into())
        }
        Err(error) => Err(error),
    }
}
