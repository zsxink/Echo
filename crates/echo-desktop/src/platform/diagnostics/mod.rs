//! Desktop observability: structured tracing, panic hook and safe error mapping
//! (task 7.8, design §17; daily retention per issue #38).
//!
//! `docs/LOGGING.md` is the authoritative convention: default log output must
//! never contain full absolute paths, lyrics text, tag strings or file
//! contents — only opaque identifiers and hashes. The *redaction* itself lives
//! in `echo-core` (`Error::to_log`, `logging::redact_path` / `redact_sensitive`
//! / `DiagnosticMode`); this module owns what the runtime is responsible for:
//!
//! - **Resolving the local diagnostics directory** (`echo/logs` under a
//!   caller-provided app-data root). The absolute app-data location is a
//!   platform/shell concern; this module takes the base path the way
//!   [`DesktopStateStore`](crate::platform::local_state::DesktopStateStore)
//!   takes its file path, keeping Core platform-independent.
//! - **Structured tracing** — [`Diagnostics::install_logger`] installs the
//!   *global* `tracing` subscriber that `echo-core` deliberately never owns
//!   (core only provides the test logger). It writes JSON lines to one file
//!   per **local date** (`echo-YYYY-MM-DD.log`, size-rotated within the day
//!   with `.N` suffixes) and stamps every event with the machine's **local
//!   time plus UTC offset** — not bare UTC. Retention is date-driven: files
//!   older than 7 days (by name date, never mtime) and legacy `echo.log*`
//!   residue are removed at startup and at each daily switch, with the
//!   per-file and cumulative byte caps acting as the second gate (issue #38;
//!   see [`retention`]). The writer makes **no structural change** to the
//!   field values it emits: privacy is enforced upstream, where business code
//!   must already pass redacted fields; this layer only guarantees the
//!   *channel* is an opaque file that never reaches the network.
//! - **Panic hook** — [`Diagnostics::install_panic_hook`] appends a local
//!   crash diagnostic (thread, panic message, panic location) to
//!   `crash-YYYY-MM-DD.log`, then re-raises so the previous hook and the
//!   default abort behaviour run. Crashes are recorded locally only — never
//!   uploaded.
//! - **Safe error mapping** — the IPC boundary (`crate::ipc::error`) maps each
//!   core error to a stable `code` + user-safe `messageKey` + `retryable` flag
//!   through the single [`ErrorPolicy`](crate::ipc::error::ErrorPolicy) table,
//!   so the frontend and the boundary never drift on which errors are safe to
//!   retry. The privacy of the *channel* this module installs is asserted by
//!   the production-log privacy test in `crates/echo-desktop/tests/privacy.rs`.
//!
//! The two installer functions mutate process-global state (the `tracing`
//! subscriber and the panic hook), so they are thin wrappers over the testable
//! [`RollingLog`] writer, the [`retention`] policy and the error-policy table.

mod retention;
mod rolling;

pub use rolling::RollingLog;

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use tracing_subscriber::prelude::*;

use retention::{
    cleanup_logs_dir, dated_path, local_today, rotation_path, LocalTimer, RetentionLimits,
    CRASH_LOG_PREFIX, RUN_LOG_PREFIX,
};

/// Default cap for a single log/panic file before it rotates within its date.
pub const DEFAULT_MAX_FILE_BYTES: u64 = 10 * 1024 * 1024; // 10 MiB
/// Default cap on retained intra-day rotations (excluding the active one).
pub const DEFAULT_MAX_ROTATIONS: usize = 4;
/// Default cumulative cap across all retained files of one log stream
/// (LOGGING §3: ≤50 MiB; the second gate behind the date window).
pub const DEFAULT_MAX_TOTAL_BYTES: u64 = 50 * 1024 * 1024; // 50 MiB
/// Default retention window: keep log files whose name date is within the
/// last 7 days, today included (issue #38).
pub const DEFAULT_RETENTION_DAYS: u32 = 7;

/// The desktop diagnostics handle. Resolves `echo/logs`, installs the global
/// `tracing` subscriber and the panic hook, and holds the retention caps.
#[derive(Clone, Debug)]
pub struct Diagnostics {
    logs_dir: PathBuf,
    max_file_bytes: u64,
    max_rotations: usize,
    max_total_bytes: u64,
    retention_days: u32,
}

impl Diagnostics {
    /// Bind a diagnostics handle to `app_data_dir/logs`. The caller supplies
    /// the already-resolved platform app-data directory; the diagnostics
    /// sub-directory is appended here so the convention lives in one place.
    #[must_use]
    pub fn new(mut app_data_dir: PathBuf) -> Self {
        // Consume the caller's app-data path by pushing the diagnostics
        // sub-directory onto it, so the `echo/logs` convention lives here.
        app_data_dir.push("logs");
        Self {
            logs_dir: app_data_dir,
            max_file_bytes: DEFAULT_MAX_FILE_BYTES,
            max_rotations: DEFAULT_MAX_ROTATIONS,
            max_total_bytes: DEFAULT_MAX_TOTAL_BYTES,
            retention_days: DEFAULT_RETENTION_DAYS,
        }
    }

    /// The absolute diagnostics directory this handle writes to.
    #[must_use]
    pub fn logs_dir(&self) -> &Path {
        &self.logs_dir
    }

    const fn retention_limits(&self) -> RetentionLimits {
        RetentionLimits {
            retention_days: self.retention_days,
            max_total_bytes: self.max_total_bytes,
        }
    }

    /// Install the **global** `tracing` subscriber, writing structured JSON
    /// lines to the current local date's `echo-YYYY-MM-DD.log`, rotating by
    /// size within the day. Event timestamps use the machine's local time with
    /// an explicit UTC offset. Before installing, one retention pass runs:
    /// files past the 7-day window and legacy `echo.log*` names are removed,
    /// and both streams are trimmed under the cumulative byte cap — the
    /// startup pass is what recovers stale files on machines that produced no
    /// logs at all (issue #38).
    ///
    /// Only the first call in a process is effective — `tracing` allows a
    /// single global default, so later calls report `false` without touching
    /// the installed subscriber. The directory itself is created lazily on
    /// the first write.
    ///
    /// # Returns
    ///
    /// `true` when this call installed the subscriber; `false` when one was
    /// already set (idempotent no-op).
    #[must_use]
    pub fn install_logger(&self) -> bool {
        cleanup_logs_dir(&self.logs_dir, &self.retention_limits(), local_today());
        let writer = RollingLog::new(
            self.logs_dir.clone(),
            RUN_LOG_PREFIX,
            self.max_file_bytes,
            self.max_rotations,
            self.max_total_bytes,
            self.retention_days,
        );
        tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .event_format(
                        tracing_subscriber::fmt::format()
                            .json()
                            .flatten_event(true)
                            .with_timer(LocalTimer),
                    )
                    .with_writer(move || writer.clone()),
            )
            .with(env_filter())
            .try_init()
            .is_ok()
    }

    /// Install the global panic hook. After appending a crash diagnostic to
    /// the current local date's `crash-YYYY-MM-DD.log`, the
    /// previously-installed hook runs and the process continues its normal
    /// panic handling (usually abort) — the hook never swallows a panic and
    /// never uploads anything. Calling this again chains on top of the
    /// existing hook (each fires on a panic), which is safe but normally
    /// unnecessary; the runtime installs it once at startup. Installing also
    /// runs one retention pass over the diagnostics directory (issue #38).
    ///
    /// # Panics
    ///
    /// `std::panic::set_hook` can panic if called while another thread is
    /// panicking — the one documented failure mode of the API.
    pub fn install_panic_hook(&self) {
        cleanup_logs_dir(&self.logs_dir, &self.retention_limits(), local_today());
        let previous = std::panic::take_hook();
        let logs_dir = self.logs_dir.clone();
        let cap = self.max_file_bytes;
        let total = self.max_total_bytes;
        let retention_days = self.retention_days;
        std::panic::set_hook(Box::new(move |info| {
            let _ = write_crash(&logs_dir, cap, total, retention_days, info);
            previous(info);
        }));
    }

    /// The active structured-log file for the current local date
    /// (`echo-YYYY-MM-DD.log`).
    #[must_use]
    pub fn log_path(&self) -> PathBuf {
        dated_path(&self.logs_dir, RUN_LOG_PREFIX, local_today())
    }

    /// The active crash-diagnostics file for the current local date
    /// (`crash-YYYY-MM-DD.log`).
    #[must_use]
    pub fn crash_path(&self) -> PathBuf {
        dated_path(&self.logs_dir, CRASH_LOG_PREFIX, local_today())
    }
}

/// Build the release `EnvFilter`: honour `RUST_LOG` if set, else default to
/// `info` for the app crates and `warn` elsewhere (so debug/other crates do
/// not flood the diagnostics file).
fn env_filter() -> tracing_subscriber::EnvFilter {
    tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("echo=info,echo_desktop=info,warn"))
}

// ---------------------------------------------------------------------------
// Panic hook crash diagnostics
// ---------------------------------------------------------------------------

/// Append a human-readable crash diagnostic for `info` to the current local
/// date's `crash-YYYY-MM-DD.log`, rotating within the date when the file
/// reaches the per-file cap. One retention pass runs first so the crash
/// stream stays under the same 7-day window and byte caps as the run log.
///
/// The line records the thread name, the panic payload and the panic location
/// — all safe local diagnostics. The payload is written verbatim because a
/// panic message is a programmer message and the crash file sits in the
/// private diagnostics directory (LOGGING §3), never shipped anywhere.
///
/// # Returns
///
/// `io::Result<()>` so a failed write can be swallowed without aborting the
/// panic-reporting path.
fn write_crash(
    logs_dir: &Path,
    max_file_bytes: u64,
    max_total_bytes: u64,
    retention_days: u32,
    info: &std::panic::PanicHookInfo<'_>,
) -> io::Result<()> {
    let today = local_today();
    cleanup_logs_dir(
        logs_dir,
        &RetentionLimits {
            retention_days,
            max_total_bytes,
        },
        today,
    );
    let _ = fs::create_dir_all(logs_dir);
    let path = dated_path(logs_dir, CRASH_LOG_PREFIX, today);
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    if file.metadata().map_or(0, |m| m.len()) + 4096 > max_file_bytes {
        // Rotate within the day so the crash log stays bounded (keep 3
        // rotations), preserving the date stem.
        drop(file);
        for index in (1..=3).rev() {
            let from = rotation_path(&path, index);
            let to = rotation_path(&path, index + 1);
            if from.exists() {
                let _ = fs::rename(&from, &to);
            }
        }
        let _ = fs::rename(&path, rotation_path(&path, 1));
        file = OpenOptions::new().create(true).append(true).open(&path)?;
    }
    let thread_name = std::thread::current()
        .name()
        .unwrap_or("<unnamed>")
        .to_owned();
    let payload = panic_payload(info);
    let location = info.location().map_or_else(
        || "<unknown>".to_owned(),
        |l| format!("{}:{}", l.file(), l.line()),
    );
    writeln!(
        file,
        "panic thread={thread_name:?} location={location:?} payload={payload:?}"
    )?;
    file.flush()
}

/// Render the panic payload (a `&(dyn Any + Send)`) as an owned string.
fn panic_payload(info: &std::panic::PanicHookInfo<'_>) -> String {
    if let Some(s) = info.payload().downcast_ref::<&str>() {
        return (*s).to_owned();
    }
    info.payload()
        .downcast_ref::<String>()
        .cloned()
        .unwrap_or_else(|| "<non-string panic payload>".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("echo-diag-{name}-{}", std::process::id()))
    }

    fn cleanup(p: &Path) {
        let _ = fs::remove_dir_all(p);
    }

    #[test]
    fn diagnostics_dir_is_appdata_plus_dated_files() {
        let diag = Diagnostics::new("/tmp/echo-app".into());
        assert_eq!(diag.logs_dir(), Path::new("/tmp/echo-app/logs"));
        // The file names carry the machine's *local* date, not a UTC date.
        let today = local_today();
        assert_eq!(
            diag.log_path().file_name().unwrap().to_str().unwrap(),
            format!("echo-{today}.log")
        );
        assert_eq!(
            diag.crash_path().file_name().unwrap().to_str().unwrap(),
            format!("crash-{today}.log")
        );
    }

    // The capture mutex guards must be held to read/write the payload; the
    // `String` inside makes the `MutexGuard` drop "significant" to clippy, but
    // dropping it early would lose the capture, so the lint is opted out.
    #[allow(clippy::significant_drop_tightening)]
    #[test]
    fn panic_payload_extracts_str_payloads() {
        // Panic hooks are process-global; serialize this temporary replacement
        // with the other hook tests so parallel test execution cannot race it.
        let _hook_guard = PANIC_HOOK_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // `PanicHookInfo` is not constructible outside a real panic, so the
        // pure helper is exercised through the real hook machinery via
        // `catch_unwind`. Interior mutability is required because the hook
        // closure is `Fn` (not `FnMut`) and must be `'static`.
        let captured = std::sync::Arc::new(std::sync::Mutex::new(None::<String>));
        let captured_hook = std::sync::Arc::clone(&captured);
        let old = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let mut slot = captured_hook
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            *slot = Some(panic_payload(info));
        }));
        let _ = std::panic::catch_unwind(|| panic!("a &str payload"));
        std::panic::set_hook(old);

        let slot = captured
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        assert_eq!(slot.as_deref(), Some("a &str payload"));
    }

    #[test]
    fn write_crash_appends_a_dated_local_diagnostic() {
        // Panic hooks are process-global. Serialize every test that replaces
        // one so another test cannot observe or overwrite this temporary hook.
        let _hook_guard = PANIC_HOOK_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = temp_dir("crash");
        cleanup(&dir);
        let today = local_today();
        let path = dir.join(format!("crash-{today}.log"));

        let old = std::panic::take_hook();
        // The hook drives `write_crash` into `dir`, then we capture the payload.
        let dir_for_hook = dir.clone();
        std::panic::set_hook(Box::new(move |info| {
            let _ = write_crash(&dir_for_hook, 1024 * 1024, 2 * 1024 * 1024, 7, info);
        }));
        // A real panic on a named thread lets the hook see the thread name and
        // the panic location + payload end to end.
        let handle = std::thread::Builder::new()
            .name("crash-test".into())
            .spawn(|| {
                let _ = std::panic::catch_unwind(|| {
                    panic!("some crash");
                });
            })
            .expect("spawn");
        handle.join().unwrap();
        std::panic::set_hook(old);

        let text = fs::read_to_string(&path).expect("dated crash file written");
        assert!(text.contains("some crash"), "{text}");
        assert!(text.contains("crash-test"), "thread name recorded: {text}");
        // The diagnostics stay local to the supplied directory only, under the
        // dated name.
        assert_eq!(
            path.file_name().unwrap().to_str().unwrap(),
            format!("crash-{today}.log")
        );
        cleanup(&dir);
    }

    #[test]
    fn write_crash_rotates_within_the_date_at_the_cap() {
        let _hook_guard = PANIC_HOOK_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = temp_dir("crash-rotate");
        cleanup(&dir);
        let today = local_today();

        let old = std::panic::take_hook();
        let dir_for_hook = dir.clone();
        // Cap so small that the second panic must rotate within the date.
        std::panic::set_hook(Box::new(move |info| {
            let _ = write_crash(&dir_for_hook, 64, 1024, 7, info);
        }));
        for payload in ["first-crash", "second-crash"] {
            let handle = std::thread::Builder::new()
                .name("crash-rotate".into())
                .spawn(move || {
                    let _ = std::panic::catch_unwind(|| {
                        panic!("{payload}");
                    });
                })
                .expect("spawn");
            handle.join().unwrap();
        }
        std::panic::set_hook(old);

        let rotated = dir.join(format!("crash-{today}.log.1"));
        assert!(rotated.exists(), "intra-date rotation happened");
        assert!(
            fs::read_to_string(rotated).unwrap().contains("first-crash"),
            "older crash preserved under the date stem"
        );
        assert!(
            fs::read_to_string(dir.join(format!("crash-{today}.log")))
                .unwrap()
                .contains("second-crash"),
            "newest crash in the active file"
        );
        cleanup(&dir);
    }

    /// The panic hook and the tracing subscriber are process-global, so their
    /// installers are exercised once and guarded: the exact return value
    /// depends on whether another test thread claimed the global first, which
    /// is fine — the contract is idempotency + side effects, not a fixed bool.
    static GLOBALS_CLAIMED: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);
    static CLAIM_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    static PANIC_HOOK_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn install_hook_and_logger_are_side_effect_free() {
        let _guard = CLAIM_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if GLOBALS_CLAIMED.swap(true, Ordering::SeqCst) {
            return; // another test thread already claimed the globals
        }
        let _hook_guard = PANIC_HOOK_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let dir = temp_dir("globals");
        cleanup(&dir);
        // Stale residue the startup pass must clear: a legacy name and a file
        // beyond the retention window.
        std::fs::create_dir_all(dir.join("logs")).unwrap();
        std::fs::write(dir.join("logs/echo.log.1"), b"legacy").unwrap();
        std::fs::write(dir.join("logs/crash-2000-01-01.log"), b"ancient").unwrap();
        let diag = Diagnostics::new(dir.clone());

        // Logger: `try_init` is atomic — the first call installs, a second
        // call reports `false` and does not disturb the installed subscriber.
        let first = diag.install_logger();
        let second = diag.install_logger();
        assert!(!second, "second call is an idempotent no-op (try_init)");

        // Hook: installing must never panic and must not leave the process
        // without a functional hook (it chains the previous one).
        diag.install_panic_hook();
        diag.install_panic_hook();

        assert!(
            !dir.join("logs/echo.log.1").exists(),
            "startup pass removed legacy names"
        );
        assert!(
            !dir.join("logs/crash-2000-01-01.log").exists(),
            "startup pass removed files beyond the window"
        );
        // Formatting and durable rolling-file output are independently tested
        // with `RollingLog`; this global test only owns installer idempotency.
        let _ = first;
        cleanup(&dir);
    }
}
