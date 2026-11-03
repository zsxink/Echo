//! The `player_sync` command — the catch-up entry point of the split playback
//! streams (issue #112).
//!
//! Tauri events are only delivered while a listener exists. The frontend
//! registers its listeners after the runtime starts, so the first queue /
//! effects events can be missed; a drift between the frontend's revision and
//! the backend's needs the same repair. `player_sync` is that repair: it
//! re-publishes the current queue projection and the current effects snapshot
//! without changing any state.
//!
//! The command returns nothing — the state arrives on the events, so the
//! command response stays small (a 480-entry projection would otherwise be
//! serialised twice per startup).
//!
//! The Tauri `#[tauri::command]` macro requires owned (by-value) arguments, so
//! `needless_pass_by_value` does not apply to the command signature.
//! `unnecessary_wraps` applies to every never-failing player command: the
//! frontend bridge types each command as `T | IpcErrorDto`, so a failure path
//! belongs here when one appears.
#![allow(clippy::needless_pass_by_value, clippy::unnecessary_wraps)]

use std::sync::Arc;

use echo_desktop::effects_service::EffectsService;
use echo_desktop::ipc::IpcErrorDto;
use echo_desktop::runtime::player::{self, QueueMetadataResolver};
use tauri::{AppHandle, Emitter, State};

use crate::commands::PlayerHandle;

/// Re-publish the current queue projection and effects snapshot.
///
/// The frontend calls it once, right after it registered its three event
/// listeners. Applying the answer is idempotent: the frontend compares the
/// revision / sequence and ignores anything that is not newer, so the command
/// and the events may arrive in any order.
#[tauri::command]
pub fn player_sync(
    app: AppHandle,
    state: State<'_, PlayerHandle>,
    metadata: State<'_, Arc<QueueMetadataResolver>>,
    effects: State<'_, Arc<EffectsService>>,
) -> Result<(), IpcErrorDto> {
    // The projection is built under one coordinator lock, so its revision and
    // its entries always belong together.
    let projection = player::queue_projection(&state.coordinator);
    let _ = app.emit(
        player::PLAYER_QUEUE_EVENT,
        player::ui_queue_projection(&projection, &metadata),
    );
    let _ = app.emit(
        player::PLAYER_EFFECTS_EVENT,
        echo_desktop::ipc::effects::EffectsSnapshotDto::from(effects.snapshot().snapshot()),
    );
    Ok(())
}
