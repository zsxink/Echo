//! Commands on the OS file-open / window file-drop channel (issue #117).
//!
//! Kept out of `commands.rs` so that file keeps a single, reviewable concern:
//! these two commands are the only IPC surface of the shell's file-open gate.
//! The shell itself owns path normalization, filtering and batching; this
//! module only resolves accepted paths into playback identities.

// Same command-layer lint contract as `commands.rs`: Tauri requires owned
// arguments, a `T | IpcErrorDto` result shape, and the coordinator guard is
// held across the whole command body on purpose.
#![allow(
    clippy::needless_pass_by_value,
    clippy::significant_drop_tightening,
    clippy::unnecessary_wraps
)]

use std::sync::Arc;

use echo_desktop::ipc::IpcErrorDto;
use echo_desktop::player::coordinator::{OpenedPlayItem, TemporaryPlay};
use echo_desktop::runtime::services::AppServices;
use echo_desktop::runtime::StartupSupervisor;
use tauri::{AppHandle, State};

use crate::commands::PlayerHandle;

#[tauri::command]
pub fn play_opened_files(
    services: State<'_, AppServices>,
    state: State<'_, PlayerHandle>,
    paths: Vec<String>,
) -> Result<(), IpcErrorDto> {
    // One OS open / window drop = one batch = one queue. Every element is a
    // DECODED absolute filesystem path: the shell normalized `file://` URLs
    // exactly once (normalize-os-file-open-paths), and drag-drop paths arrive
    // as `PathBuf` from wry. DO NOT URL-decode here — a filename may legitimately
    // contain a literal `%20`. The display name is the final path segment,
    // taken verbatim; it is resolved here so the WebView never has to parse a
    // path for presentation.
    //
    // This is a playback path, not an import: it deliberately does not go
    // through `guard_writes`, so a read-only or unavailable library root never
    // blocks temporary playback (docs/PRODUCT.md:34).
    let coord = state.coordinator.clone();
    let mut items = Vec::with_capacity(paths.len());
    for path in paths {
        let file_path = std::path::PathBuf::from(&path);
        if let Some(song_id) = services
            .active_song_for_path(&file_path)
            .map_err(IpcErrorDto::from)?
        {
            items.push(OpenedPlayItem::Library(song_id));
            continue;
        }
        let display_name = file_path.file_name().map_or_else(
            || "音频文件".to_owned(),
            |name| name.to_string_lossy().into_owned(),
        );
        let metadata = services.read_temporary_metadata(&file_path);
        items.push(OpenedPlayItem::Temporary(Box::new(TemporaryPlay {
            display_name,
            path: file_path,
            duration: metadata.duration,
            metadata,
            on_active_root: false,
        })));
    }
    if items.is_empty() {
        return Ok(());
    }
    let mut coord = coord.lock().expect("player coordinator lock");
    coord.play_opened_files(items);
    Ok(())
}

/// The frontend has registered its `app://file-open-request` listener
/// (task 9.1 / deliver-file-opens-after-frontend-ready).
///
/// On a cold start the OS hands the double-clicked file to Echo while the
/// `WebView` is still booting, so the shell queues the path: Tauri drops an
/// event emitted before any `JS` listener exists. The frontend calls this the
/// moment its listener is in place, and the shell delivers what it queued —
/// which is the difference between "Echo started" and "Echo played the file".
///
/// Idempotent, so React's `StrictMode` double-invoked effect and a listener
/// re-registered after a reload are both harmless: the second call finds an
/// empty queue. The paths themselves travel on `FILE_OPEN_REQUEST`, exactly
/// like a file open on an already-running instance, so this reports nothing
/// back — the frontend consumes them through the listener it just registered.
///
/// Returns `Result` because every command's transport is `T | IpcErrorDto`
/// (see the module note on `unnecessary_wraps`), not because this can fail.
#[tauri::command]
pub fn file_open_frontend_ready(
    app: AppHandle,
    startup: State<'_, Arc<StartupSupervisor>>,
) -> Result<(), IpcErrorDto> {
    crate::drain_pending_opens(&app, &startup);
    Ok(())
}
