//! macOS gate-exit scheduling and pending-file-open queueing/delivery.

#[allow(clippy::wildcard_imports, unused_imports)]
use super::*;

#[allow(clippy::redundant_pub_crate)]
pub(super) fn schedule_gate_exit(app: tauri::AppHandle) {
    let Ok(milliseconds) = env::var("ECHO_GATE_QUIT_AFTER_MS") else {
        return;
    };
    let Ok(milliseconds) = milliseconds.parse::<u64>() else {
        return;
    };

    thread::spawn(move || {
        thread::sleep(Duration::from_millis(milliseconds));
        app.exit(0);
    });
}

/// The Gate uses this opt-in log to assert that a hot second launch delivered
/// its file path to the already-running instance. Production never sets it.
#[allow(clippy::redundant_pub_crate)]
pub(super) fn record_gate_open(paths: &[PathBuf]) {
    let Ok(log_path) = env::var("ECHO_GATE_OPEN_LOG") else {
        return;
    };
    let Ok(mut log) = OpenOptions::new().create(true).append(true).open(log_path) else {
        return;
    };
    for path in paths {
        let _ = writeln!(log, "{}", path.to_string_lossy());
    }
}

/// The shell→frontend event carrying one accepted file-open path (task 9.1).
/// The frontend bridge subscribes to this; the payload is a single absolute
/// path that was either delivered immediately or drained from the startup FIFO.
/// The event name also appears in `echo-desktop::ipc::events`.
const FILE_OPEN_REQUEST: &str = "app://file-open-request";

/// Deliver a batch of accepted file-open paths to the frontend.
///
/// The single place that emits [`FILE_OPEN_REQUEST`], so a path — delivered
/// immediately or drained later — is recorded for the Gate exactly once.
#[allow(clippy::redundant_pub_crate)]
pub(super) fn emit_file_opens(app: &tauri::AppHandle, paths: Vec<PathBuf>) {
    if paths.is_empty() {
        return;
    }
    record_gate_open(&paths);
    let _ = app.emit(FILE_OPEN_REQUEST, paths);
}

/// Route one OS file-open path through the startup supervisor (task 9.1).
///
/// While the runtime is still initializing — or the frontend has not yet
/// registered its file-open listener — the path is stashed in the bounded FIFO
/// and later drained by [`drain_pending_opens`], never silently dropped.
/// `receive_file_open` returns `Some` only once **both** gates are open, and
/// only then is an emit safe: Tauri drops an event with no registered JS
/// listener, which is what happened to every cold-start double-click before
/// `deliver-file-opens-after-frontend-ready`.
///
/// `startup` is passed in rather than looked up through `try_state` on purpose.
/// macOS delivers a cold-start `odoc` **before `.setup()` runs**, so a managed
/// state lookup is `None` at exactly the moment the first — and only — open
/// request arrives; that lookup is what dropped the path and made a
/// double-click merely launch the app. The caller owns the process-wide
/// instance created at the top of [`main`], so there is no window in which a
/// path can arrive with nowhere to go.
#[allow(clippy::redundant_pub_crate)]
pub(super) fn deliver_file_open(
    app: &tauri::AppHandle,
    startup: &StartupSupervisor,
    path: PathBuf,
) {
    if let Some(ready_path) = startup.receive_file_open(path) {
        emit_file_opens(app, vec![ready_path]);
    }
}

/// The frontend registered its `app://file-open-request` listener: open the
/// delivery gate and hand back every path queued before that
/// (deliver-file-opens-after-frontend-ready).
///
/// This is the **only** delivery point for queued paths. The shell cannot drain
/// them when `.setup()` finishes — the `WebView` has not loaded yet, so
/// `js_event_listeners` is still empty and Tauri would drop the emit. Paths
/// therefore wait here instead of being lost.
///
/// Idempotent (see [`StartupSupervisor::mark_frontend_ready`]): a repeat call
/// drains an empty queue, so the frontend effect may run twice.
#[allow(clippy::redundant_pub_crate)]
pub(super) fn drain_pending_opens(app: &tauri::AppHandle, startup: &StartupSupervisor) {
    let drained = startup.mark_frontend_ready();
    emit_file_opens(app, drained);
}
