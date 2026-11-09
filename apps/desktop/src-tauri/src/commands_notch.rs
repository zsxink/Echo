//! macOS notch player transport: the settings-row state, the persisted
//! preference and the window-growth handshake.
//!
//! Split out of `commands.rs` to keep that file inside the project's 1000-line
//! limit, and because all three commands are about one companion window rather
//! than the library/playback surface.
#![allow(clippy::needless_pass_by_value)] // Tauri extracts owned command arguments across IPC.

use std::sync::Arc;

use echo_desktop::ipc::IpcErrorDto;
use echo_desktop::platform::local_state::DesktopStateStore;
use tauri::State;

use crate::notch::{self, NotchPlayer};

/// The macOS notch player's settings-row state: hardware availability plus the
/// persisted preference. Pure read — never rewrites the preference.
#[tauri::command]
pub fn get_notch_player_state(
    notch: State<'_, NotchPlayer>,
) -> echo_desktop::ipc::dto::NotchPlayerStateDto {
    echo_desktop::ipc::dto::NotchPlayerStateDto {
        available: notch.available(),
        enabled: notch.enabled(),
        notch_width: notch::notch_width(),
    }
}

/// Persist the notch player preference and reconcile the window with it.
///
/// When the current Mac has no camera housing the preference is deliberately
/// **not** written: a closed lid or a different display must not erase the
/// user's choice. The response still reports `available = false` so the
/// settings row stays disabled and in sync.
#[tauri::command]
pub fn set_notch_player_enabled(
    app: tauri::AppHandle,
    local_state: State<'_, Arc<DesktopStateStore>>,
    notch: State<'_, NotchPlayer>,
    enabled: bool,
) -> Result<echo_desktop::ipc::dto::NotchPlayerStateDto, IpcErrorDto> {
    if !notch.available() {
        return Ok(echo_desktop::ipc::dto::NotchPlayerStateDto {
            available: false,
            enabled: notch.enabled(),
            notch_width: None,
        });
    }
    local_state.set_show_notch_player(enabled).map_err(|_| {
        IpcErrorDto::from(&echo_core::error::Error::unavailable(
            "preferences",
            "write failed",
        ))
    })?;
    notch.set_enabled(enabled);
    let available = notch.sync(&app);
    Ok(echo_desktop::ipc::dto::NotchPlayerStateDto {
        available,
        enabled: notch.enabled(),
        notch_width: notch::notch_width(),
    })
}

/// Grow or shrink the companion window. The renderer awaits this before it
/// plays the expand animation, so the panel is never clipped by the old frame.
#[tauri::command]
pub fn notch_player_set_expanded(
    app: tauri::AppHandle,
    notch: State<'_, NotchPlayer>,
    expanded: bool,
) -> Result<(), IpcErrorDto> {
    if !notch.available() || !notch.enabled() {
        return Err(IpcErrorDto::from(&echo_core::error::Error::unavailable(
            "notch-player",
            "the notch player is not open",
        )));
    }
    // Record the state only after the window actually resized: a failed resize
    // must not leave the flag claiming a size the window does not have.
    notch::set_expanded(&app, expanded).map_err(|_| {
        IpcErrorDto::from(&echo_core::error::Error::unavailable(
            "notch-player",
            "the notch window could not be resized",
        ))
    })?;
    notch.set_expanded(expanded);
    Ok(())
}
