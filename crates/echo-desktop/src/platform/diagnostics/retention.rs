//! Retention policy for the local diagnostics directory (issue #38):
//! date-stamped file naming, strict name parsing, the 7-day window, removal of
//! legacy size-rotation names, and the cumulative byte cap that acts as the
//! second retention gate behind the date window.
//!
//! Everything here is a pure function over the directory so all three callers
//! — startup install, the run log's daily switch, and the crash write path —
//! share one policy. Deletion decisions are made from the date **in the file
//! name**, never from mtime: a copied or synced file must not drift into (or
//! out of) the retention window.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{Local, NaiveDate};

use tracing_subscriber::fmt::time::FormatTime;

/// File-name prefix of the structured run log (`echo-YYYY-MM-DD.log`).
pub(super) const RUN_LOG_PREFIX: &str = "echo";
/// File-name prefix of the crash diagnostics log (`crash-YYYY-MM-DD.log`).
pub(super) const CRASH_LOG_PREFIX: &str = "crash";

/// Retention knobs shared by every cleanup caller.
#[derive(Clone, Copy, Debug)]
pub(super) struct RetentionLimits {
    /// Files whose name date is older than `today - retention_days` are
    /// deleted; exactly `retention_days` days old is still kept.
    pub retention_days: u32,
    /// Cumulative byte cap per log prefix (the second gate, issue #38).
    pub max_total_bytes: u64,
}

/// The machine's current local date. File dates and the date window are local
/// dates, not UTC dates (issue #38).
pub(super) fn local_today() -> NaiveDate {
    Local::now().date_naive()
}

/// Active file name for `prefix` on `date`, e.g. `echo-2026-10-24.log`.
pub(super) fn dated_file_name(prefix: &str, date: NaiveDate) -> String {
    format!("{prefix}-{date}.log")
}

/// Full path of the active `prefix` file for `date` inside `dir`.
pub(super) fn dated_path(dir: &Path, prefix: &str, date: NaiveDate) -> PathBuf {
    dir.join(dated_file_name(prefix, date))
}

/// Rotation suffix path (`index` ≥ 1) for an active log `path`.
pub(super) fn rotation_path(path: &Path, index: u64) -> PathBuf {
    PathBuf::from(format!("{}.{index}", path.display()))
}

/// The parsed identity of a diagnostics log file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct DatedLogFile {
    pub prefix: &'static str,
    /// Local date encoded in the file name.
    pub date: NaiveDate,
    /// Intra-day rotation slot; 0 is the active (unsuffixed) name and a larger
    /// index holds older content shifted by a size-triggered rotation.
    pub index: u64,
}

/// Recognize `echo-YYYY-MM-DD.log` / `crash-YYYY-MM-DD.log` with an optional
/// numeric rotation suffix. Dates must be fixed-width, zero-padded ISO dates;
/// anything else (including unrelated files such as the redaction path map)
/// returns `None` and is therefore never touched by cleanup.
pub(super) fn parse_dated_log_file(file_name: &str) -> Option<DatedLogFile> {
    for prefix in [RUN_LOG_PREFIX, CRASH_LOG_PREFIX] {
        let Some(rest) = file_name.strip_prefix(prefix) else {
            continue;
        };
        let Some(rest) = rest.strip_prefix('-') else {
            continue;
        };
        // `rest` is `YYYY-MM-DD.log` or `YYYY-MM-DD.log.N`.
        let Some((date_part, tail)) = rest.split_once(".log") else {
            continue;
        };
        let date = parse_iso_date(date_part)?;
        let index = match tail {
            "" => 0,
            suffix if suffix.len() > 1 && suffix.starts_with('.') => {
                let digits = &suffix[1..];
                if digits.bytes().all(|b| b.is_ascii_digit()) {
                    digits.parse().ok()?
                } else {
                    return None;
                }
            }
            _ => return None,
        };
        return Some(DatedLogFile {
            prefix,
            date,
            index,
        });
    }
    None
}

/// Parse a strict zero-padded `YYYY-MM-DD` local date.
fn parse_iso_date(text: &str) -> Option<NaiveDate> {
    let bytes = text.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    if !bytes
        .iter()
        .enumerate()
        .all(|(i, b)| i == 4 || i == 7 || b.is_ascii_digit())
    {
        return None;
    }
    NaiveDate::parse_from_str(text, "%Y-%m-%d").ok()
}

/// Recognize the legacy size-rotation names kept by earlier versions:
/// `echo.log`, `crash.log` and their `.N` rotations. They carry no date, so
/// the startup pass deletes them instead of migrating them under a guessed
/// date (design D4: mtime is not a trusted signal).
fn is_legacy_log_name(file_name: &str) -> bool {
    for stem in ["echo.log", "crash.log"] {
        if file_name == stem {
            return true;
        }
        if let Some(digits) = file_name.strip_prefix(&format!("{stem}.")) {
            if !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()) {
                return true;
            }
        }
    }
    false
}

/// One retention pass over the diagnostics directory:
///
/// 1. delete dated log files whose name date falls before the 7-day window;
/// 2. delete legacy `echo.log*` / `crash.log*` residue from older versions;
/// 3. trim each prefix back under its cumulative byte cap, oldest first.
///
/// Unrecognized files are never touched. Missing directory is not an error —
/// there is simply nothing to clean yet.
pub(super) fn cleanup_logs_dir(dir: &Path, limits: &RetentionLimits, today: NaiveDate) {
    let cutoff = today
        .checked_sub_days(chrono::Days::new(u64::from(limits.retention_days)))
        .unwrap_or(NaiveDate::MIN);
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        if !entry.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let expired = parse_dated_log_file(name).is_some_and(|f| f.date < cutoff);
        if expired || is_legacy_log_name(name) {
            let _ = fs::remove_file(entry.path());
        }
    }
    for prefix in [RUN_LOG_PREFIX, CRASH_LOG_PREFIX] {
        trim_prefix_to_total(dir, prefix, limits.max_total_bytes, today);
    }
}

/// Enforce the cumulative byte cap for one prefix: when the retained files of
/// that prefix together exceed `cap`, delete the oldest first — by name date
/// ascending, and within a date by descending rotation index (a higher index
/// holds older content). The current day's active file (index 0) is never
/// deleted; the cap is a retention gate, not a truncate-the-live-file rule.
pub(super) fn trim_prefix_to_total(dir: &Path, prefix: &str, cap: u64, today: NaiveDate) {
    let mut retained: Vec<(NaiveDate, u64, u64, PathBuf)> = Vec::new();
    let mut total = 0u64;
    for entry in fs::read_dir(dir).into_iter().flatten().flatten() {
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        let Some(parsed) = parse_dated_log_file(&name) else {
            continue;
        };
        if parsed.prefix != prefix {
            continue;
        }
        let Ok(meta) = fs::metadata(entry.path()) else {
            continue;
        };
        total = total.saturating_add(meta.len());
        retained.push((parsed.date, parsed.index, meta.len(), entry.path()));
    }
    if total <= cap {
        return;
    }
    retained.sort_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
    for (date, index, len, path) in retained {
        if total <= cap {
            break;
        }
        if date == today && index == 0 {
            continue; // never delete the live active file
        }
        if fs::remove_file(&path).is_ok() {
            total = total.saturating_sub(len);
        }
    }
}

/// `tracing` timer rendering the machine's local wall clock with an explicit
/// UTC offset, e.g. `2026-10-24T21:05:03.123+08:00`. The JSON `timestamp`
/// field must read as local time — not as bare UTC that masquerades as the
/// user's wall clock (issue #38). UTC machines render `+00:00`.
pub(super) struct LocalTimer;

impl FormatTime for LocalTimer {
    fn format_time(&self, w: &mut tracing_subscriber::fmt::format::Writer<'_>) -> std::fmt::Result {
        let now = Local::now();
        w.write_str(&now.to_rfc3339_opts(chrono::SecondsFormat::Millis, false))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{DateTime, FixedOffset};

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("echo-diag-retention-{name}-{}", std::process::id()))
    }

    fn cleanup(p: &Path) {
        let _ = fs::remove_dir_all(p);
    }

    fn day(year: i32, month: u32, day: u32) -> NaiveDate {
        NaiveDate::from_ymd_opt(year, month, day).expect("valid test date")
    }

    #[test]
    fn parses_dated_names_with_and_without_rotation_suffix() {
        let parsed = parse_dated_log_file("echo-2026-10-24.log").expect("active run log");
        assert_eq!(parsed.prefix, RUN_LOG_PREFIX);
        assert_eq!(parsed.date, day(2026, 10, 24));
        assert_eq!(parsed.index, 0);

        let parsed = parse_dated_log_file("echo-2026-10-24.log.3").expect("rotation");
        assert_eq!(parsed.index, 3);

        let parsed = parse_dated_log_file("crash-2026-01-02.log.1").expect("crash rotation");
        assert_eq!(parsed.prefix, CRASH_LOG_PREFIX);
        assert_eq!(parsed.date, day(2026, 1, 2));
    }

    #[test]
    fn rejects_non_log_or_malformed_names() {
        // Not diagnostics files — cleanup must never touch these.
        assert!(parse_dated_log_file("paths.json").is_none());
        assert!(parse_dated_log_file("echo-2026-10-24.json").is_none());
        // Non-zero-padded or impossible dates.
        assert!(parse_dated_log_file("echo-2026-1-4.log").is_none());
        assert!(parse_dated_log_file("echo-2026-13-01.log").is_none());
        // Non-numeric suffix or double extension.
        assert!(parse_dated_log_file("echo-2026-10-24.log.x").is_none());
        assert!(parse_dated_log_file("echo-2026-10-24.log.log").is_none());
        // Different stream prefix.
        assert!(parse_dated_log_file("library-2026-10-24.log").is_none());
    }

    #[test]
    fn recognizes_legacy_rotation_names_only() {
        assert!(is_legacy_log_name("echo.log"));
        assert!(is_legacy_log_name("echo.log.4"));
        assert!(is_legacy_log_name("crash.log"));
        assert!(is_legacy_log_name("crash.log.2"));
        assert!(!is_legacy_log_name("echo.logx"));
        assert!(!is_legacy_log_name("echo.logging"));
        assert!(!is_legacy_log_name("paths.json"));
        assert!(!is_legacy_log_name("echo.log.two"));
    }

    #[test]
    fn cleanup_honours_the_seven_day_window_and_legacy_names() {
        let dir = temp_dir("window");
        cleanup(&dir);
        fs::create_dir_all(&dir).unwrap();
        let today = day(2026, 10, 24);
        let keep_7 = dir.join("echo-2026-10-17.log"); // exactly 7 days old: kept
        let keep_recent = dir.join("echo-2026-10-23.log");
        let drop_8 = dir.join("echo-2026-10-16.log"); // 8 days old: dropped
        let drop_crash = dir.join("crash-2026-10-16.log");
        let drop_legacy = dir.join("echo.log.2");
        let unrelated = dir.join("paths.json");
        for path in [
            &keep_7,
            &keep_recent,
            &drop_8,
            &drop_crash,
            &drop_legacy,
            &unrelated,
        ] {
            fs::write(path, b"x").unwrap();
        }

        cleanup_logs_dir(
            &dir,
            &RetentionLimits {
                retention_days: 7,
                max_total_bytes: 1024,
            },
            today,
        );

        assert!(keep_7.exists(), "exactly 7 days old is within the window");
        assert!(keep_recent.exists());
        assert!(unrelated.exists(), "non-log files untouched");
        assert!(!drop_8.exists(), "older than 7 days removed");
        assert!(!drop_crash.exists(), "crash stream uses the same window");
        assert!(!drop_legacy.exists(), "legacy names removed at startup");
        cleanup(&dir);
    }

    #[test]
    fn total_cap_trims_oldest_dates_first_and_protects_the_active_file() {
        let dir = temp_dir("trim");
        cleanup(&dir);
        fs::create_dir_all(&dir).unwrap();
        let today = day(2026, 10, 24);
        // 5 bytes each; cap 12 → only the newest two files may survive.
        fs::write(dir.join("echo-2026-10-20.log"), b"aaaaa").unwrap();
        fs::write(dir.join("echo-2026-10-21.log"), b"aaaaa").unwrap();
        fs::write(dir.join("echo-2026-10-21.log.2"), b"aaaaa").unwrap();
        fs::write(dir.join("echo-2026-10-24.log"), b"aaaaa").unwrap();
        fs::write(dir.join("crash-2026-10-20.log"), b"aaaaa").unwrap(); // other prefix

        trim_prefix_to_total(&dir, RUN_LOG_PREFIX, 12, today);

        assert!(
            !dir.join("echo-2026-10-20.log").exists(),
            "oldest date first"
        );
        // Same date, higher index is older content: dropped before index 0.
        assert!(!dir.join("echo-2026-10-21.log.2").exists());
        assert!(dir.join("echo-2026-10-21.log").exists());
        assert!(
            dir.join("echo-2026-10-24.log").exists(),
            "today's active file is never trimmed"
        );
        assert!(
            dir.join("crash-2026-10-20.log").exists(),
            "prefixes trimmed independently"
        );
        cleanup(&dir);
    }

    #[test]
    fn local_timer_emits_parseable_local_time_with_machine_offset() {
        let mut buffer = String::new();
        let mut writer = tracing_subscriber::fmt::format::Writer::new(&mut buffer);
        LocalTimer.format_time(&mut writer).expect("format");

        let parsed = DateTime::<FixedOffset>::parse_from_rfc3339(&buffer)
            .expect("RFC 3339 with explicit offset");
        let machine_offset = Local::now().offset().local_minus_utc();
        assert_eq!(
            parsed.offset().local_minus_utc(),
            machine_offset,
            "timestamp offset must match the machine timezone, not default to UTC"
        );
    }
}
