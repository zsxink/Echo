//! Desktop observability: structured tracing, panic hook and safe error mapping
//! (task 7.8, design §17).
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
//!   (core only provides the test logger). It writes JSON lines to a rolling
//!   file with a per-file size cap and bounded retention, so diagnostics stay
//!   bounded on disk. The writer makes **no structural change** to the field
//!   values it emits: privacy is enforced upstream, where business code must
//!   already pass redacted fields; this layer only guarantees the *channel* is
//!   an opaque file that never reaches the network.
//! - **Panic hook** — [`Diagnostics::install_panic_hook`] appends a local
//!   crash diagnostic (thread, panic message, panic location) to `crash.log`,
//!   then re-raises so the previous hook and the default abort behaviour run.
//!   Crashes are recorded locally only — never uploaded.
//! - **Safe error mapping** — the IPC boundary (`crate::ipc::error`) maps each
//!   core error to a stable `code` + user-safe `messageKey` + `retryable` flag
//!   through the single [`ErrorPolicy`](crate::ipc::error::ErrorPolicy) table,
//!   so the frontend and the boundary never drift on which errors are safe to
//!   retry. The privacy of the *channel* this module installs is asserted by
//!   the production-log privacy test in `crates/echo-desktop/tests/privacy.rs`.
//!
//! The two installer functions mutate process-global state (the `tracing`
//! subscriber and the panic hook), so they are thin wrappers over the
//! testable rolling writer and the error-policy table.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use tracing_subscriber::prelude::*;

/// Default cap for a single log/panic file before it rotates.
pub const DEFAULT_MAX_FILE_BYTES: u64 = 10 * 1024 * 1024; // 10 MiB
/// Default cap on retained rotated files (excluding the active one).
pub const DEFAULT_MAX_ROTATIONS: usize = 4;
/// Default cumulative cap across all retained files (LOGGING §3: ≤50 MiB).
pub const DEFAULT_MAX_TOTAL_BYTES: u64 = 50 * 1024 * 1024; // 50 MiB

/// The desktop diagnostics handle. Resolves `echo/logs`, installs the global
/// `tracing` subscriber and the panic hook, and holds the size caps.
#[derive(Clone, Debug)]
pub struct Diagnostics {
    logs_dir: PathBuf,
    max_file_bytes: u64,
    max_rotations: usize,
    max_total_bytes: u64,
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
        }
    }

    /// The absolute diagnostics directory this handle writes to.
    #[must_use]
    pub fn logs_dir(&self) -> &Path {
        &self.logs_dir
    }

    /// Install the **global** `tracing` subscriber, writing structured JSON
    /// lines to `echo.log` in the diagnostics directory, rotating by size.
    ///
    /// Only the first call in a process is effective — `tracing` allows a
    /// single global default, so later calls report `false` without touching
    /// the installed subscriber. The directory is created on this call.
    ///
    /// # Returns
    ///
    /// `true` when this call installed the subscriber; `false` when one was
    /// already set (idempotent no-op).
    #[must_use]
    pub fn install_logger(&self) -> bool {
        let writer = RollingLog::new(
            self.log_path(),
            self.max_file_bytes,
            self.max_rotations,
            self.max_total_bytes,
        );
        tracing_subscriber::registry()
            .with(
                tracing_subscriber::fmt::layer()
                    .event_format(tracing_subscriber::fmt::format().json().flatten_event(true))
                    .with_writer(move || writer.clone()),
            )
            .with(env_filter())
            .try_init()
            .is_ok()
    }

    /// Install the global panic hook. After appending a crash diagnostic to
    /// `crash.log`, the previously-installed hook runs and the process
    /// continues its normal panic handling (usually abort) — the hook never
    /// swallows a panic and never uploads anything. Calling this again chains
    /// on top of the existing hook (each fires on a panic), which is safe but
    /// normally unnecessary; the runtime installs it once at startup.
    ///
    /// # Panics
    ///
    /// `std::panic::set_hook` can panic if called while another thread is
    /// panicking — the one documented failure mode of the API.
    pub fn install_panic_hook(&self) {
        let previous = std::panic::take_hook();
        let logs_dir = self.logs_dir.clone();
        let cap = self.max_file_bytes;
        std::panic::set_hook(Box::new(move |info| {
            let _ = write_crash(&logs_dir, cap, info);
            previous(info);
        }));
    }

    /// The active structured-log file (`echo.log`).
    #[must_use]
    pub fn log_path(&self) -> PathBuf {
        self.logs_dir.join("echo.log")
    }

    /// The active crash-diagnostics file (`crash.log`).
    #[must_use]
    pub fn crash_path(&self) -> PathBuf {
        self.logs_dir.join("crash.log")
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
// Rolling file writer
// ---------------------------------------------------------------------------

/// A size-bounded, self-rotating log file. The active file is `echo.log`; on
/// rotation it becomes `echo.log.1`, existing rotations shift up, and the
/// oldest is dropped — respecting both the per-file cap and a cumulative cap.
///
/// Writer state is shared behind an internal mutex so every `tracing` event
/// serializes through one lock; writes never interleave across threads.
#[derive(Clone, Debug)]
pub struct RollingLog {
    inner: std::sync::Arc<Mutex<RollingLogInner>>,
}

#[derive(Debug)]
struct RollingLogInner {
    path: PathBuf,
    max_file_bytes: u64,
    max_rotations: usize,
    max_total_bytes: u64,
    file: Option<File>,
    written: u64,
}

impl RollingLog {
    /// Create a rolling writer on `path` with the given caps.
    #[must_use]
    pub fn new(
        path: PathBuf,
        max_file_bytes: u64,
        max_rotations: usize,
        max_total_bytes: u64,
    ) -> Self {
        Self {
            inner: std::sync::Arc::new(Mutex::new(RollingLogInner {
                path,
                max_file_bytes,
                max_rotations,
                max_total_bytes,
                file: None,
                written: 0,
            })),
        }
    }

    /// The active writer's file path (the one it was constructed with).
    #[must_use]
    pub fn path(&self) -> PathBuf {
        self.lock().path.clone()
    }
}

impl RollingLog {
    /// Lock and recover the writer state from poisoning (a panicked writer
    /// thread must not take down logging for every other thread).
    fn lock(&self) -> std::sync::MutexGuard<'_, RollingLogInner> {
        self.inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

// The `inner` guard must be held for the whole write — it owns the open
// `File` handle and the running byte count; dropping it early (which
// `clippy::significant_drop_tightening` proposes) would release the handle
// mid-write, so the lint's suggestion is deliberately not taken here.
#[allow(clippy::significant_drop_tightening)]
impl io::Write for RollingLog {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut inner = self.lock();
        ensure_open(&mut inner)?;
        if inner.written + buf.len() as u64 > inner.max_file_bytes {
            rotate(&mut inner)?;
        }
        let file = inner.file.as_mut().expect("opened above");
        let n = file.write(buf)?;
        inner.written += n as u64;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        let mut inner = self.lock();
        if let Some(file) = inner.file.as_mut() {
            file.flush()?;
        }
        Ok(())
    }
}

/// Open the active file for append if it is not already open, and record the
/// number of bytes already in it (so a pre-existing file counts toward the cap
/// and rotates correctly).
fn ensure_open(inner: &mut RollingLogInner) -> io::Result<()> {
    if inner.file.is_none() {
        if let Some(parent) = inner.path.parent() {
            fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&inner.path)?;
        inner.written = file.metadata().map_or(0, |m| m.len());
        inner.file = Some(file);
    }
    Ok(())
}

/// Rotate: shift `path.n` up to the retention limit, rename the active file to
/// `path.1`, trim the oldest to honour the cumulative cap, and reopen fresh.
fn rotate(inner: &mut RollingLogInner) -> io::Result<()> {
    // Close the current handle (drop the `File`) before renaming.
    inner.file = None;

    if inner.max_rotations == 0 {
        // No retention configured: just truncate the file instead of keeping
        // any history.
        let _ = fs::remove_file(&inner.path);
        return reopen_fresh(inner);
    }

    // Shift rotations up, then drop the active into slot 1.
    for index in (1..=inner.max_rotations as u64).rev() {
        let from = rotation_path(&inner.path, index);
        let to = rotation_path(&inner.path, index + 1);
        if from.exists() {
            let _ = fs::rename(&from, &to);
        }
    }
    let _ = fs::remove_file(rotation_path(&inner.path, inner.max_rotations as u64 + 1));
    let _ = fs::rename(&inner.path, rotation_path(&inner.path, 1));
    trim_to_total(inner, inner.max_total_bytes);
    reopen_fresh(inner)
}

/// Remove rotated files (from the oldest) until the cumulative size is within
/// `cap`. The active file is never trimmed by this pass.
fn trim_to_total(inner: &RollingLogInner, cap: u64) {
    let mut total = current_total(inner);
    let mut index = inner.max_rotations as u64 + 1;
    while total > cap && index > 1 {
        let path = rotation_path(&inner.path, index);
        if let Ok(meta) = fs::metadata(&path) {
            let len = meta.len();
            let _ = fs::remove_file(&path);
            total = total.saturating_sub(len);
        }
        index -= 1;
    }
}

/// Current cumulative size of the rotated files (active handled separately).
fn current_total(inner: &RollingLogInner) -> u64 {
    let mut total = 0u64;
    for index in 1..=(inner.max_rotations as u64 + 1) {
        let path = rotation_path(&inner.path, index);
        if let Ok(meta) = fs::metadata(&path) {
            total = total.saturating_add(meta.len());
        }
    }
    total
}

fn rotation_path(path: &Path, index: u64) -> PathBuf {
    PathBuf::from(format!("{}.{index}", path.display()))
}

fn reopen_fresh(inner: &mut RollingLogInner) -> io::Result<()> {
    // A fresh active file starts empty (the prior content rotated away).
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&inner.path)?;
    inner.written = 0;
    inner.file = Some(file);
    Ok(())
}

impl<'a> tracing_subscriber::fmt::writer::MakeWriter<'a> for RollingLog {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

// ---------------------------------------------------------------------------
// Panic hook crash diagnostics
// ---------------------------------------------------------------------------

/// Append a human-readable crash diagnostic for `info` to `crash.log`, rotating
/// the file when it reaches the per-file cap.
///
/// The line records the thread name, the panic payload and the panic location
/// — all safe local diagnostics. The payload is written verbatim because a
/// panic message is a programmer message and `crash.log` sits in the private
/// diagnostics directory (LOGGING §3), never shipped anywhere.
///
/// # Returns
///
/// `io::Result<()>` so a failed write can be swallowed without aborting the
/// panic-reporting path.
fn write_crash(
    logs_dir: &Path,
    max_file_bytes: u64,
    info: &std::panic::PanicHookInfo<'_>,
) -> io::Result<()> {
    let _ = fs::create_dir_all(logs_dir);
    let path = logs_dir.join("crash.log");
    let mut file = OpenOptions::new().create(true).append(true).open(&path)?;
    if file.metadata().map_or(0, |m| m.len()) + 4096 > max_file_bytes {
        // Rotate the crash log too so it stays bounded (keep 3 rotations).
        drop(file);
        for index in (1..=3).rev() {
            let from = logs_dir.join(format!("crash.log.{index}"));
            let to = logs_dir.join(format!("crash.log.{}", index + 1));
            if from.exists() {
                let _ = fs::rename(&from, &to);
            }
        }
        let _ = fs::rename(&path, logs_dir.join("crash.log.1"));
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
    fn diagnostics_dir_is_appdata_plus_logs() {
        let diag = Diagnostics::new("/tmp/echo-app".into());
        assert_eq!(diag.logs_dir(), Path::new("/tmp/echo-app/logs"));
        assert_eq!(diag.log_path().file_name().unwrap(), "echo.log");
        assert_eq!(diag.crash_path().file_name().unwrap(), "crash.log");
    }

    #[test]
    fn rolling_log_writes_appends_and_flushes() {
        let dir = temp_dir("append");
        cleanup(&dir);
        let log = RollingLog::new(dir.join("echo.log"), 1024, 3, 10 * 1024);

        let mut a = log.clone();
        let mut b = log.clone();
        a.write_all(b"line-one\n").unwrap();
        b.write_all(b"line-two\n").unwrap();
        a.flush().unwrap();

        let text = fs::read_to_string(log.path()).unwrap();
        assert!(text.contains("line-one"), "{text}");
        assert!(text.contains("line-two"), "{text}");
        cleanup(&dir);
    }

    #[test]
    fn rolling_log_rotates_when_the_file_cap_is_exceeded() {
        let dir = temp_dir("rotate");
        cleanup(&dir);
        // Tiny per-file cap so a few writes force a rotation.
        let mut w = RollingLog::new(dir.join("echo.log"), 20, 2, 10 * 1024);

        for _ in 0..4 {
            w.write_all(b"0123456789\n").unwrap(); // 11 bytes each
        }
        w.flush().unwrap();

        // The active file is fresh (truncated by reopen); the previous content
        // moved to `echo.log.1` and older to `echo.log.2`.
        assert!(dir.join("echo.log.1").exists(), "first rotation exists");
        let active_len = fs::metadata(dir.join("echo.log")).map_or(0, |m| m.len());
        assert!(active_len <= 20 + 11, "active file stays near the cap");
        // Retention is bounded: no `.3`.
        assert!(!dir.join("echo.log.3").exists(), "retention respected");
        cleanup(&dir);
    }

    #[test]
    fn rolling_log_truncates_when_retention_is_zero() {
        let dir = temp_dir("truncate");
        cleanup(&dir);
        let mut w = RollingLog::new(dir.join("echo.log"), 20, 0, 1024);
        w.write_all(b"0123456789\n").unwrap();
        w.write_all(b"0123456789\n").unwrap();
        w.flush().unwrap();
        // With zero retention the file is truncated, leaving only the last
        // write's remnant (`0123456789\n` fits under cap and stays).
        let text = fs::read_to_string(dir.join("echo.log")).unwrap();
        assert!(
            !text.contains("0123456789\n0123456789\n"),
            "no history kept"
        );
        assert!(text.ends_with('\n'));
        cleanup(&dir);
    }

    #[test]
    fn cumulative_total_is_trimmed_to_the_cap() {
        let dir = temp_dir("total");
        cleanup(&dir);
        // Per-file cap 20, retention 3, cumulative cap 45 → only ~2 full files
        // survive trimming.
        let mut w = RollingLog::new(dir.join("echo.log"), 20, 3, 45);
        for _ in 0..6 {
            w.write_all(b"0123456789\n").unwrap(); // 11 bytes
        }
        w.flush().unwrap();

        let mut total = 0u64;
        for index in 1..=3u64 {
            let p = dir.join(format!("echo.log.{index}"));
            if let Ok(m) = fs::metadata(&p) {
                total += m.len();
            }
        }
        assert!(total <= 45, "cumulative rotated total within cap: {total}");
        cleanup(&dir);
    }

    // The capture mutex guards must be held to read/write the payload; the
    // `String` inside makes the `MutexGuard` drop "significant" to clippy, but
    // dropping it early would lose the capture, so the lint is opted out.
    #[allow(clippy::significant_drop_tightening)]
    #[test]
    fn panic_payload_extracts_str_payloads() {
        // `PanicHookInfo` is not constructible outside a real panic, so the
        // pure helper is exercised through the real hook machinery via
        // `catch_unwind`. Interior mutability is required because the hook
        // closure is `Fn` (not `FnMut`) and must be `'static`.
        let captured = std::sync::Arc::new(Mutex::new(None::<String>));
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
    fn write_crash_appends_a_local_diagnostic() {
        let dir = temp_dir("crash");
        cleanup(&dir);
        let path = dir.join("crash.log");

        let old = std::panic::take_hook();
        // The hook drives `write_crash` into `dir`, then we capture the payload.
        let dir_for_hook = dir.clone();
        std::panic::set_hook(Box::new(move |info| {
            let _ = write_crash(&dir_for_hook, 1024 * 1024, info);
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

        let text = fs::read_to_string(&path).expect("crash file written");
        assert!(text.contains("some crash"), "{text}");
        assert!(text.contains("crash-test"), "thread name recorded: {text}");
        // The diagnostics stay local to the supplied directory only.
        assert_eq!(path.file_name().unwrap(), "crash.log");
        cleanup(&dir);
    }

    /// The panic hook and the tracing subscriber are process-global, so their
    /// installers are exercised once and guarded: the exact return value
    /// depends on whether another test thread claimed the global first, which
    /// is fine — the contract is idempotency + side effects, not a fixed bool.
    static GLOBALS_CLAIMED: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);
    static CLAIM_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn install_hook_and_logger_are_side_effect_free() {
        let _guard = CLAIM_LOCK
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if GLOBALS_CLAIMED.swap(true, Ordering::SeqCst) {
            return; // another test thread already claimed the globals
        }
        let dir = temp_dir("globals");
        cleanup(&dir);
        let diag = Diagnostics::new(dir.clone());

        // Logger: `try_init` is atomic — the first call installs, a second
        // call reports `false` and does not disturb the installed subscriber.
        let first = diag.install_logger();
        let second = diag.install_logger();
        assert!(first, "first call installs the subscriber");
        assert!(!second, "second call is an idempotent no-op (try_init)");

        // Hook: installing must never panic and must not leave the process
        // without a functional hook (it chains the previous one).
        diag.install_panic_hook();
        diag.install_panic_hook();

        // The subscriber writes structured lines to `echo.log`: emit a
        // redaction-safe event on the app target and confirm it lands.
        tracing::info!(target: "echo_desktop", operation = "diag-test", "structured tracing reaches the diagnostics file");
        let log_path = diag.log_path();
        let text = fs::read_to_string(&log_path).unwrap_or_default();
        assert!(
            text.contains("diag-test") && text.contains("structured tracing"),
            "structured line landed in the diagnostics file: {text}"
        );
        cleanup(&dir);
    }
}
