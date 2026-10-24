//! The rolling writer behind the structured run log: one file per local date
//! (`echo-YYYY-MM-DD.log`), size-triggered rotations **within** a date
//! (`.N` suffixes), and the write-path date switch that moves output to the
//! next day's file across midnight without restarting the process (issue
//! #38). The global `tracing` subscriber can only be installed once per
//! process, so a background timer swapping files is not an option — the
//! switch is judged inside `write`, which already serializes on the writer
//! mutex.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use chrono::NaiveDate;

use super::retention::{self, RetentionLimits};

/// Injectable "what is today" clock so date-switch and retention behaviour is
/// deterministically testable without waiting for real midnights.
pub(super) type DateClock = Arc<dyn Fn() -> NaiveDate + Send + Sync>;

/// A size- and date-bounded log writer. All writes serialize through one
/// mutex; the active file is `dir/<prefix>-<local date>.log`, intra-day size
/// rotations shift to `.N` siblings, and the first write after a local date
/// change switches files and triggers a retention pass.
#[derive(Clone)]
pub struct RollingLog {
    inner: Arc<Mutex<RollingLogInner>>,
}

struct RollingLogInner {
    dir: PathBuf,
    prefix: String,
    max_file_bytes: u64,
    max_rotations: usize,
    max_total_bytes: u64,
    retention_days: u32,
    clock: DateClock,
    /// Local date the current handle belongs to.
    today: NaiveDate,
    file: Option<File>,
    written: u64,
}

impl RollingLog {
    /// Create a rolling writer on the machine's real local clock.
    #[must_use]
    pub fn new(
        dir: PathBuf,
        prefix: &str,
        max_file_bytes: u64,
        max_rotations: usize,
        max_total_bytes: u64,
        retention_days: u32,
    ) -> Self {
        Self::with_clock(
            dir,
            prefix,
            max_file_bytes,
            max_rotations,
            max_total_bytes,
            retention_days,
            Arc::new(retention::local_today),
        )
    }

    /// Create a rolling writer driven by an explicit clock (tests and
    /// diagnostics tooling).
    #[must_use]
    pub fn with_clock(
        dir: PathBuf,
        prefix: &str,
        max_file_bytes: u64,
        max_rotations: usize,
        max_total_bytes: u64,
        retention_days: u32,
        clock: DateClock,
    ) -> Self {
        let today = clock();
        Self {
            inner: Arc::new(Mutex::new(RollingLogInner {
                dir,
                prefix: prefix.to_owned(),
                max_file_bytes,
                max_rotations,
                max_total_bytes,
                retention_days,
                clock,
                today,
                file: None,
                written: 0,
            })),
        }
    }

    /// The active writer's file path (for the current local date).
    #[must_use]
    pub fn path(&self) -> PathBuf {
        let inner = self.lock();
        retention::dated_path(&inner.dir, &inner.prefix, inner.today)
    }
}

impl RollingLog {
    /// Lock and recover the writer state from poisoning (a panicked writer
    /// thread must not take down logging for every other thread).
    fn lock(&self) -> MutexGuard<'_, RollingLogInner> {
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
        let today = (inner.clock)();
        if today != inner.today {
            // Crossed local midnight (or the first write happened after the
            // constructor's date). Release the old date's handle, adopt the
            // new date and run the retention pass — the daily switch is the
            // mid-run cleanup trigger (issue #38), so no timer thread exists.
            inner.file = None;
            inner.today = today;
            retention::cleanup_logs_dir(
                &inner.dir,
                &RetentionLimits {
                    retention_days: inner.retention_days,
                    max_total_bytes: inner.max_total_bytes,
                },
                today,
            );
        }
        ensure_open(&mut inner)?;
        if inner.written + buf.len() as u64 > inner.max_file_bytes {
            rotate(&mut inner)?;
        }
        let file = inner.file.as_mut().expect("opened above");
        let n = file.write(buf)?;
        // A tracing formatter may not call `flush` after every event. Logs are
        // diagnostic evidence, so make each completed event visible promptly
        // rather than leaving it buffered until process shutdown.
        file.flush()?;
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

/// The active file path for the inner state's current date.
fn active_path(inner: &RollingLogInner) -> PathBuf {
    retention::dated_path(&inner.dir, &inner.prefix, inner.today)
}

/// Open the active file for append if it is not already open, and record the
/// number of bytes already in it (so a pre-existing file counts toward the cap
/// and rotates correctly). A same-day restart therefore appends to the
/// existing file instead of forking a second one.
fn ensure_open(inner: &mut RollingLogInner) -> io::Result<()> {
    if inner.file.is_none() {
        fs::create_dir_all(&inner.dir)?;
        let path = active_path(inner);
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        inner.written = file.metadata().map_or(0, |m| m.len());
        inner.file = Some(file);
    }
    Ok(())
}

/// Size-triggered rotation within the current date: shift the date's `.n`
/// rotations up to the retention limit, rename the active file to `.1`, trim
/// the cumulative cap, and reopen fresh.
fn rotate(inner: &mut RollingLogInner) -> io::Result<()> {
    // Close the current handle (drop the `File`) before renaming.
    inner.file = None;

    if inner.max_rotations == 0 {
        // No retention configured: just truncate the file instead of keeping
        // any history.
        let _ = fs::remove_file(active_path(inner));
        return reopen_fresh(inner);
    }

    let active = active_path(inner);
    // Shift rotations up, then drop the active into slot 1.
    for index in (1..=inner.max_rotations as u64).rev() {
        let from = retention::rotation_path(&active, index);
        let to = retention::rotation_path(&active, index + 1);
        if from.exists() {
            let _ = fs::rename(&from, &to);
        }
    }
    let _ = fs::remove_file(retention::rotation_path(
        &active,
        inner.max_rotations as u64 + 1,
    ));
    let _ = fs::rename(&active, retention::rotation_path(&active, 1));
    // The cumulative cap is the second gate behind the date window: trim
    // oldest-first across all retained dates. The active file does not exist
    // right now (it was just renamed to `.1`), so nothing live can be hit.
    retention::trim_prefix_to_total(
        &inner.dir,
        &inner.prefix,
        inner.max_total_bytes,
        inner.today,
    );
    reopen_fresh(inner)
}

fn reopen_fresh(inner: &mut RollingLogInner) -> io::Result<()> {
    // A fresh active file starts empty (the prior content rotated away).
    let file = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(active_path(inner))?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::Path;

    use super::retention::RUN_LOG_PREFIX;

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("echo-diag-rolling-{name}-{}", std::process::id()))
    }

    fn cleanup(p: &Path) {
        let _ = fs::remove_dir_all(p);
    }

    fn fixed_clock(date: NaiveDate) -> DateClock {
        Arc::new(move || date)
    }

    fn day(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).expect("valid test date")
    }

    #[test]
    fn writes_append_and_flush() {
        let dir = temp_dir("append");
        cleanup(&dir);
        let log = RollingLog::with_clock(
            dir.clone(),
            RUN_LOG_PREFIX,
            1024,
            3,
            10 * 1024,
            7,
            fixed_clock(day(2026, 10, 24)),
        );

        let mut a = log.clone();
        let mut b = log.clone();
        a.write_all(b"line-one\n").unwrap();
        b.write_all(b"line-two\n").unwrap();
        a.flush().unwrap();

        let path = dir.join("echo-2026-10-24.log");
        assert_eq!(log.path(), path);
        let text = fs::read_to_string(path).unwrap();
        assert!(text.contains("line-one"), "{text}");
        assert!(text.contains("line-two"), "{text}");
        cleanup(&dir);
    }

    #[test]
    fn rotates_within_the_date_when_the_file_cap_is_exceeded() {
        let dir = temp_dir("rotate");
        cleanup(&dir);
        // Tiny per-file cap so a few writes force a rotation.
        let mut w = RollingLog::with_clock(
            dir.clone(),
            RUN_LOG_PREFIX,
            20,
            2,
            10 * 1024,
            7,
            fixed_clock(day(2026, 10, 24)),
        );

        for _ in 0..4 {
            w.write_all(b"0123456789\n").unwrap(); // 11 bytes each
        }
        w.flush().unwrap();

        // The active file is fresh (truncated by reopen); the previous content
        // moved to `.1` and older to `.2` — same date stem.
        assert!(dir.join("echo-2026-10-24.log.1").exists(), "first rotation");
        let active_len = fs::metadata(dir.join("echo-2026-10-24.log")).map_or(0, |m| m.len());
        assert!(active_len <= 20 + 11, "active file stays near the cap");
        assert!(
            !dir.join("echo-2026-10-24.log.3").exists(),
            "retention kept"
        );
        cleanup(&dir);
    }

    #[test]
    fn truncates_when_retention_is_zero() {
        let dir = temp_dir("truncate");
        cleanup(&dir);
        let mut w = RollingLog::with_clock(
            dir.clone(),
            RUN_LOG_PREFIX,
            20,
            0,
            1024,
            7,
            fixed_clock(day(2026, 10, 24)),
        );
        w.write_all(b"0123456789\n").unwrap();
        w.write_all(b"0123456789\n").unwrap();
        w.flush().unwrap();
        // With zero retention the file is truncated, leaving only the last
        // write's remnant (`0123456789\n` fits under cap and stays).
        let text = fs::read_to_string(dir.join("echo-2026-10-24.log")).unwrap();
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
        let mut w = RollingLog::with_clock(
            dir.clone(),
            RUN_LOG_PREFIX,
            20,
            3,
            45,
            7,
            fixed_clock(day(2026, 10, 24)),
        );
        for _ in 0..6 {
            w.write_all(b"0123456789\n").unwrap(); // 11 bytes
        }
        w.flush().unwrap();

        let mut total = 0u64;
        for index in 1..=3u64 {
            if let Ok(m) = fs::metadata(dir.join(format!("echo-2026-10-24.log.{index}"))) {
                total += m.len();
            }
        }
        assert!(total <= 45, "cumulative rotated total within cap: {total}");
        cleanup(&dir);
    }

    #[test]
    fn crossing_midnight_moves_writes_to_the_new_date_file() {
        let dir = temp_dir("midnight");
        cleanup(&dir);
        // A mutable clock: the writer must observe the new date on its next
        // write without being reconstructed (no restart, no timer).
        let date = Arc::new(Mutex::new(day(2026, 10, 24)));
        let clock: DateClock = {
            let date = Arc::clone(&date);
            Arc::new(move || {
                *date
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
            })
        };
        let mut w =
            RollingLog::with_clock(dir.clone(), RUN_LOG_PREFIX, 1024, 2, 10 * 1024, 7, clock);

        w.write_all(b"before-midnight\n").unwrap();
        *date
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = day(2026, 10, 25);
        w.write_all(b"after-midnight\n").unwrap();
        w.flush().unwrap();

        let day1 = fs::read_to_string(dir.join("echo-2026-10-24.log")).unwrap();
        let day2 = fs::read_to_string(dir.join("echo-2026-10-25.log")).unwrap();
        assert!(day1.contains("before-midnight"), "{day1}");
        assert!(!day1.contains("after-midnight"), "no bleed into old date");
        assert!(day2.contains("after-midnight"), "{day2}");
        assert!(!day2.contains("before-midnight"), "no bleed into new date");
        cleanup(&dir);
    }

    #[test]
    fn the_daily_switch_also_runs_the_retention_pass() {
        let dir = temp_dir("switch-cleanup");
        cleanup(&dir);
        // Stale files from an "older" run: one beyond the 7-day window and a
        // legacy name from a previous version. Neither is touched while the
        // date holds; the first write after the switch must clean both.
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("echo-2026-10-16.log"), b"stale").unwrap();
        fs::write(dir.join("echo.log.2"), b"legacy").unwrap();

        let date = Arc::new(Mutex::new(day(2026, 10, 24)));
        let clock: DateClock = {
            let date = Arc::clone(&date);
            Arc::new(move || {
                *date
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
            })
        };
        let mut w =
            RollingLog::with_clock(dir.clone(), RUN_LOG_PREFIX, 1024, 2, 10 * 1024, 7, clock);
        w.write_all(b"day-one\n").unwrap();
        assert!(dir.join("echo-2026-10-16.log").exists(), "no switch yet");
        *date
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = day(2026, 10, 25);
        w.write_all(b"day-two\n").unwrap();

        assert!(
            !dir.join("echo-2026-10-16.log").exists(),
            "beyond the window cleaned at the switch"
        );
        assert!(
            !dir.join("echo.log.2").exists(),
            "legacy names cleaned at the switch"
        );
        assert!(dir.join("echo-2026-10-24.log").exists(), "recent kept");
        cleanup(&dir);
    }
}
