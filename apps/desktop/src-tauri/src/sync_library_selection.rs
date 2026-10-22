//! Library selection with a guarded LAN sync handoff (task 4.1/4.2).
//!
//! The directory picker, candidate scan and activation transaction stay in
//! `AppServices`; this module only adds the session boundary around the
//! activation and reports the resulting root-aware status. Keeping it separate
//! leaves `commands.rs` a thin one-line-per-command layer.

use std::sync::Arc;

use echo_desktop::ipc::dto::LibraryRootStatusDto;
use echo_desktop::ipc::IpcErrorDto;
use echo_desktop::runtime::services::AppServices;
use tauri::{Emitter, Manager, State};

use crate::sync_runtime::SyncRuntime;

#[tauri::command]
pub async fn choose_library_root(
    app: tauri::AppHandle,
    sync: State<'_, Arc<SyncRuntime>>,
) -> Result<Option<LibraryRootStatusDto>, IpcErrorDto> {
    // Tauri executes synchronous commands on the event-loop thread. The real
    // dialog adapter deliberately uses the plugin's blocking picker, which
    // requires a worker thread while the native event loop continues pumping
    // dialog events. Making this command async gives it that worker context.
    let sync = Arc::clone(&sync);
    let app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        // Resolve the services from the cloned handle: a `State` borrow cannot
        // escape into the blocking worker that owns the native picker.
        let services = app.state::<AppServices>();
        // The candidate is fully prepared before the session is fenced, so a
        // cancelled picker or a failed candidate scan leaves the running
        // session, its retries and its caches exactly as they were.
        let mut handoff: Option<crate::sync_runtime::RootHandoff> = None;
        let outcome = services.choose_library_root_with_hooks(&mut |_prepare| {
            // This hook runs after the candidate scan succeeded and immediately
            // before the activation transaction commits. Fencing here is what
            // keeps the old library serving while the picker and the scan run.
            handoff = Some(sync.begin_root_handoff().map_err(|error| {
                echo_core::error::Error::unavailable("sync session", error.message_key())
            })?);
            Ok(())
        });
        match outcome {
            Ok(Some((status, prepare))) => {
                // The activation committed. Hand the session off to the new
                // root: only its own binding and enabled intent may start it.
                match sync.complete_root_handoff(prepare.root_id) {
                    Ok(session) => {
                        publish_status(&app, session);
                        Ok(Some(status))
                    }
                    Err(error) => {
                        publish_status(&app, sync.report_error(error));
                        Err(IpcErrorDto::new(
                            "unavailable",
                            error.message_key(),
                            true,
                            None,
                            None,
                        ))
                    }
                }
            }
            Ok(None) => Ok(None),
            Err(error) => {
                // The activation never committed: the previous root is still
                // active, so put its previous session back instead of leaving
                // it silently stopped by a switch that did not happen.
                if let Some(handoff) = handoff {
                    match sync.abort_root_handoff(&handoff) {
                        Ok(status) => publish_status(&app, status),
                        Err(restore_error) => {
                            publish_status(&app, sync.report_error(restore_error));
                        }
                    }
                }
                Err(IpcErrorDto::from(error))
            }
        }
    })
    .await
    .map_err(|_| IpcErrorDto::new("unavailable", "sync.workerFailed", true, None, None))?
}

/// Publish a root-aware sync status after a library handoff. Refreshing here
/// prevents the UI from keeping the previous root's session while the new root
/// reports stopped, paused or active.
fn publish_status(app: &tauri::AppHandle, status: crate::sync_runtime::SyncStatus) {
    let _ = app.emit("sync://status", status);
}
