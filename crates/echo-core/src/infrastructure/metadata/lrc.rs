//! The LRC [`LyricsParser`](crate::application::ports::LyricsParser) (task 4.5).
//!
//! Grammar handled:
//!
//! - Timestamped lines `[mm:ss.xx]text` — one *or more* leading timestamps per
//!   line, hundredths (`ss.xx`) and thousandths (`ss.xxx`), optional hours
//!   (`[hh:mm:ss.xx]`).
//! - Metadata tag lines `[ti:…]`/`[ar:…]`/`[al:…]`/`[by:…]`/`[offset:±ms]`:
//!   recognized, kept out of the lyric lines; `offset` shifts every timestamp.
//! - Lines with no valid timestamps at all → the candidate is *plain text*
//!   (its readable lines stay the display content).
//! - Out-of-order timestamps are sorted into playback order, keeping each
//!   line's `original_index`; timestamps that land negative after the offset
//!   are invalid and dropped with a parse note.
//! - Empty/unreadable input yields a candidate with a parse note — a corrupt
//!   or empty source must never block the song record.

use crate::application::ports::LyricsParser;
use crate::domain::entities::{LyricsCandidate, LyricsLine, LyricsSource};

/// The LRC parser.
#[derive(Clone, Debug, Default)]
pub struct LrcLyricsParser;

impl LyricsParser for LrcLyricsParser {
    fn parse(&self, raw: &str) -> LyricsCandidate {
        parse_lrc(raw, LyricsSource::Embedded)
    }
}

/// Parse `raw` into a candidate for `source` (the scan pipeline rewraps the
/// port-level result for sidecar sources).
///
/// `parse_error` is reserved for *fatal* corruption (nothing readable at all).
/// Dropping a single out-of-range timestamp is a documented, non-fatal repair:
/// the surviving lines are the candidate, and the drop is visible in the
/// returned content itself. Out-of-order input is sorted; `[offset:]` applies
/// to the whole file (two-pass, wherever the tag appears).
#[must_use]
pub fn parse_lrc(raw: &str, source: LyricsSource) -> LyricsCandidate {
    // Pass 1: the global offset (LRC convention, wherever the tag sits).
    let offset_ms: i64 = raw
        .lines()
        .find_map(|line| match tag_line(line) {
            Some(("offset", value)) => value.parse().ok(),
            _ => None,
        })
        .unwrap_or(0);

    // Pass 2: timestamps + content.
    let mut lines: Vec<LyricsLine> = Vec::new();
    let mut plain_lines: Vec<String> = Vec::new();
    let mut out_of_range: usize = 0;
    for (index, raw_line) in raw.lines().enumerate() {
        let (timestamps, text) = split_timestamps(raw_line);
        if timestamps.is_empty() {
            // A metadata tag line (`[ti:...]`) or ordinary text. Tag lines
            // carry no lyric content; real text lines feed the plain path.
            if tag_line(raw_line).is_none() {
                let text = text.trim();
                if !text.is_empty() {
                    plain_lines.push(text.to_owned());
                }
            }
            continue;
        }
        let text = text.trim().to_owned();
        for timestamp_ms in timestamps {
            let shifted = timestamp_ms + offset_ms;
            if shifted < 0 {
                out_of_range += 1;
                continue;
            }
            lines.push(LyricsLine {
                timestamp_ms: shifted,
                text: text.clone(),
                original_index: index,
            });
        }
    }

    if lines.is_empty() {
        // No usable timestamps: either plain text (readable content) or an
        // empty/unreadable source (a fatal parse note, no content).
        if plain_lines.is_empty() {
            let mut error = if raw.trim().is_empty() {
                "lyrics are empty".to_owned()
            } else {
                "no readable lyric content".to_owned()
            };
            if out_of_range > 0 {
                use std::fmt::Write as _;
                let _ = write!(
                    error,
                    "; {out_of_range} timestamp(s) out of range after offset"
                );
            }
            return LyricsCandidate::with_raw_text(
                source,
                raw.to_owned(),
                Vec::new(),
                None,
                Some(error),
            );
        }
        return LyricsCandidate::with_raw_text(
            source,
            raw.to_owned(),
            Vec::new(),
            Some(plain_lines.join("\n")),
            None,
        );
    }
    // Playback order, with `original_index` preserved from the source text.
    lines.sort_by_key(|line| (line.timestamp_ms, line.original_index));
    LyricsCandidate::with_raw_text(source, raw.to_owned(), lines, None, None)
}

/// Take one leading `[timestamp]` off `line`: `Some((ms, rest))` when the
/// group is a numeric timestamp, `None` otherwise.
fn take_timestamp(line: &str) -> Option<(i64, &str)> {
    let after_open = line.strip_prefix('[')?;
    let close = after_open.find(']')?;
    let ms = parse_timestamp(&after_open[..close])?;
    Some((ms, &after_open[close + 1..]))
}

/// Split leading `[…]` groups off a line: the timestamps they contain (if
/// numeric) plus the remaining text.
fn split_timestamps(line: &str) -> (Vec<i64>, &str) {
    let mut timestamps = Vec::new();
    let mut rest = line;
    while let Some((ms, remainder)) = take_timestamp(rest) {
        timestamps.push(ms);
        rest = remainder;
    }
    (timestamps, rest)
}

/// `mm:ss.xx`, `mm:ss.xxx`, `hh:mm:ss.xx(x)` → milliseconds.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::suboptimal_flops
)]
fn parse_timestamp(body: &str) -> Option<i64> {
    let parts: Vec<&str> = body.split(':').collect();
    let (hours, minutes, seconds) = match parts.as_slice() {
        [minutes, seconds] => (0.0_f64, parse_number(minutes)?, parse_number(seconds)?),
        [hours, minutes, seconds] => (
            parse_number(hours)?,
            parse_number(minutes)?,
            parse_number(seconds)?,
        ),
        _ => return None,
    };
    if seconds > 59.999_999 {
        // Out-of-range second field (e.g. [00:75.00]) is not a timestamp.
        return None;
    }
    Some((hours * 3_600_000.0 + minutes * 60_000.0 + seconds * 1000.0) as i64)
}

/// Parse a number that may use `,` as the fractional separator (some LRC
/// writers do), returning the value in its field's natural unit.
fn parse_number(text: &str) -> Option<f64> {
    text.trim().replace(',', ".").parse().ok()
}

/// Recognize `[tag:value]` metadata lines.
fn tag_line(line: &str) -> Option<(&'static str, &str)> {
    const TAGS: [&str; 5] = ["ti", "ar", "al", "by", "offset"];
    let inner = line.strip_prefix('[')?.strip_suffix(']')?;
    for tag in TAGS {
        if let Some(value) = inner.strip_prefix(tag).and_then(|r| r.strip_prefix(':')) {
            return Some((tag, value.trim()));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lrc_parser_sorts_out_of_order_and_multi_timestamps() {
        let raw = "[ar:x][ti:y]\n[00:20.00]second\n[00:10.00][00:05.50]first dup\n[00:30.500]third";
        let candidate = LrcLyricsParser.parse(raw);
        assert!(candidate.parse_error().is_none());
        let lines = candidate.lines();
        let times: Vec<i64> = lines.iter().map(|l| l.timestamp_ms).collect();
        assert_eq!(times, vec![5_500, 10_000, 20_000, 30_500]);
        assert_eq!(lines[0].text, "first dup");
        assert_eq!(lines[2].text, "second");
        assert_eq!(lines[2].original_index, 1, "original index preserved");
        assert!(!candidate.is_plain_text());
        assert_eq!(candidate.raw_text(), raw);
    }

    #[test]
    fn lrc_offset_is_global_and_out_of_range_lines_are_dropped() {
        let raw = "[00:10.00]first\n[offset:-30000]\n[00:20.00]gone";
        let candidate = LrcLyricsParser.parse(raw);
        // The offset applies to the whole file; lines shifted below zero are
        // dropped, and with nothing readable left the source is fatally
        // flagged (which never blocks the song record itself).
        assert!(candidate.lines().is_empty(), "{:?}", candidate.lines());
        let note = candidate.parse_error().expect("fatal note after drops");
        assert!(note.contains("out of range"), "{note}");
        assert_eq!(candidate.raw_text(), raw);
    }

    #[test]
    fn lrc_offset_shifts_all_lines() {
        let raw = "[offset:1500]\n[00:10.00]a\n[00:20.00]b";
        let candidate = LrcLyricsParser.parse(raw);
        let times: Vec<i64> = candidate.lines().iter().map(|l| l.timestamp_ms).collect();
        assert_eq!(times, vec![11_500, 21_500]);
        assert!(candidate.parse_error().is_none());
    }

    #[test]
    fn lrc_without_timestamps_is_plain_text() {
        let raw = "第一行\n第二行";
        let candidate = LrcLyricsParser.parse(raw);
        assert!(candidate.is_plain_text());
        assert_eq!(candidate.plain_text(), Some("第一行\n第二行"));
        assert!(candidate.lines().is_empty());
        assert!(candidate.parse_error().is_none());
    }

    #[test]
    fn lrc_empty_or_unreadable_is_a_note_not_a_blocker() {
        let empty = LrcLyricsParser.parse("");
        assert_eq!(empty.parse_error(), Some("lyrics are empty"));

        // Tag lines only: no lyric content anywhere.
        let tags_only = LrcLyricsParser.parse("[ti:title]\n[ar:artist]");
        assert_eq!(tags_only.parse_error(), Some("no readable lyric content"));
    }

    #[test]
    fn lrc_accepts_millisecond_and_hour_forms() {
        let raw = "[01:02.300]a\n[00:00:03.45]b\n[00:07,5]c";
        let candidate = LrcLyricsParser.parse(raw);
        let times: Vec<i64> = candidate.lines().iter().map(|l| l.timestamp_ms).collect();
        assert_eq!(times, vec![3_450, 7_500, 62_300]);
    }
}
