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

/// Route a whole OS open / window drop batch through the startup supervisor.
/// Both OS entries (single-instance argv and macOS `RunEvent::Opened`) and the
/// native drop handler converge here. The gate is
/// defined per path (bounded FIFO + frontend-ready flag), so each path still
/// goes through [`StartupSupervisor::receive_file_open`] individually; only the
/// *emit* is batched. One drop therefore produces exactly one
/// [`FILE_OPEN_REQUEST`] event carrying every accepted path in the OS-supplied
/// order, instead of N events that each rebuild the playback queue (#117 D3).
fn accepted_opens(startup: &StartupSupervisor, paths: Vec<PathBuf>) -> Vec<PathBuf> {
    paths
        .into_iter()
        .filter_map(|path| startup.receive_file_open(path))
        .collect()
}

#[allow(clippy::redundant_pub_crate)]
pub(super) fn deliver_file_opens(
    app: &tauri::AppHandle,
    startup: &StartupSupervisor,
    paths: Vec<PathBuf>,
) {
    emit_file_opens(app, accepted_opens(startup, paths));
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

#[cfg(test)]
mod tests {
    use super::*;

    fn path(name: &str) -> PathBuf {
        PathBuf::from("/tmp/echo-open-tests").join(name)
    }

    #[test]
    fn one_batch_keeps_platform_order_when_the_frontend_is_ready() {
        let startup = StartupSupervisor::new();
        startup.on_ready();
        startup.mark_frontend_ready();
        let paths = vec![path("a.flac"), path("b.mp3"), path("c.wav")];
        // A single emit carries all three, in the order the platform supplied.
        assert_eq!(accepted_opens(&startup, paths.clone()), paths);
    }

    #[test]
    fn one_batch_queues_every_path_until_the_frontend_listener_is_ready() {
        let startup = StartupSupervisor::new();
        startup.on_ready();
        let paths = vec![path("a.flac"), path("b.mp3")];
        assert!(accepted_opens(&startup, paths.clone()).is_empty());
        // Nothing is silently dropped: the FIFO drains in arrival order.
        assert_eq!(startup.mark_frontend_ready(), paths);
    }
}
