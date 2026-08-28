//! Text normalization and platform-safe name rules (task 2.3).
//!
//! Echo stores user-provided text (song names, artists, playlist names) in its
//! original Unicode form for display, but derives a *normalized* form for the
//! operations that must be deterministic and comparable:
//!
//! - **NFKC** — compatibility + canonical decomposition (full-width → half,
//!   ligatures split), which is the normalization Echo uses for indexes,
//!   deduplication and import target naming.
//! - **Case folding** — a Unicode-aware, *not* locale-aware case fold used for
//!   comparisons (search, playlist-name uniqueness). We fold then normalize.
//! - **Grapheme cluster length** — the user-perceived character count
//!   (a family emoji with ZWJ sequences counts as ONE cluster; a base + marks
//!   counts as one). Used for the 40-character playlist-name limit.
//! - **Safe filename components** — for import targets `歌手/歌曲.ext`: NFKC,
//!   control-char removal, Windows reserved names / trailing dots and spaces
//!   handled, platform-length truncation with a short-hash suffix keeping the
//!   extension.
//!
//! Implementation notes:
//!
//! - Pure Rust (the [`unicode-normalization`](https://crates.io/crates/unicode-normalization)
//!   and [`unicode-segmentation`](https://crates.io/crates/unicode-segmentation)
//!   crates), no `unsafe`, no platform `cfg`. The *rules* are platform-safe by
//!   construction even though the OS still enforces its own limits.
//! - Case folding is implemented as a conservative map: lower-case then fold
//!   common full/half forms via NFKC. (A full caseless fold is a large table;
//!   Echo's need is *stable comparisons*, not linguistic exactness.)

use unicode_casefold::UnicodeCaseFold;
use unicode_normalization::UnicodeNormalization;
use unicode_segmentation::UnicodeSegmentation;

/// The normalized, comparable form of a user text value.
///
/// Produced by [`normalize_text`]: NFKC + trim + collapse ASCII runs of
/// whitespace. Mirrors `normalized_value` in the logging module but for names,
/// and is what the repository indexes and compares.
#[must_use]
pub fn normalize_text(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut pending_space = false;
    let mut started = false;
    for c in value.nfkc() {
        if c.is_whitespace() {
            pending_space = true;
        } else {
            if started && pending_space {
                out.push(' ');
            }
            out.extend(c.nfkc());
            pending_space = false;
            started = true;
        }
    }
    out
}

/// Fold case *and* run [`normalize_text`] so comparisons are
/// case-insensitive AND compatibility-insensitive.
///
/// Uses Unicode's full, non-Turkic default case fold after NFKC. This makes
/// canonical equivalents such as `Straße`/`STRASSE` and final/normal Greek
/// sigma compare identically without depending on the user's locale.
#[must_use]
pub fn normalized_key(value: &str) -> String {
    let normalized: String = value.nfkc().collect();
    normalize_text(&normalized.case_fold().collect::<String>())
}

/// Count user-perceived characters (grapheme clusters) — the number a human
/// sees when reading the string, not its byte or scalar length.
#[must_use]
pub fn grapheme_count(value: &str) -> usize {
    value.graphemes(true).count()
}

/// Validate a playlist / song name (1..=40 user-perceived characters after
/// trimming both ends and normalization).
///
/// # Errors
///
/// Returns `Some(reason)` when the name is empty/whitespace-only, or exceeds
/// 40 grapheme clusters.
pub fn validate_playlist_name(name: &str) -> Result<(), String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err("名称不能为空".to_owned());
    }
    let count = grapheme_count(trimmed);
    if count > 40 {
        return Err(format!("名称不能超过 40 个字符（当前 {count}）"));
    }
    Ok(())
}

/// The unique-key form used to reject duplicate playlist names:
/// NFKC + case fold + normalized whitespace.
#[must_use]
pub fn playlist_name_key(name: &str) -> String {
    normalized_key(name)
}

// ---------------------------------------------------------------------------
// Safe filename components (import target naming)
// ---------------------------------------------------------------------------

/// Characters that must never appear in a single path component (they have
/// meaning for the OS / separator / current-dir).
const fn forbidden_char(c: char) -> bool {
    matches!(
        c,
        '/' | '\\'
            | '\0'
            | ':' // drive / alternate data stream on Windows
            | '*' | '?' | '"' | '<' | '>' | '|'
    )
}

/// Control chars (C0 + C1) and Unicode non-characters are replaced/removed.
fn is_control_or_noncharacter(c: char) -> bool {
    if c.is_control() {
        return true;
    }
    matches!(c as u32, 0xFDD0..=0xFDEF | 0xFFFE | 0xFFFF)
}

/// Windows reserved device names (case-insensitive), with or without an
/// extension: `CON`, `PRN`, `AUX`, `NUL`, `COM1..9`, `LPT1..9`.
fn is_windows_reserved(component: &str) -> bool {
    let stem = component.split('.').next().unwrap_or(component);
    let upper = stem.to_ascii_uppercase();
    matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (upper.len() == 4
            && (upper.starts_with("COM") || upper.starts_with("LPT"))
            && upper[3..].bytes().all(|b| b.is_ascii_digit() && b != b'0'))
}

/// The safe, deterministic base name for an import target component.
///
/// Steps: NFKC → remove/replace forbidden & control chars → trim trailing
/// dots/spaces (otherwise Windows drops them silently) → magazine-separate
/// leading dots (hidden files are not what a music file should become) →
/// Windows reserved names get a trailing `_`.
#[must_use]
pub fn safe_component(value: &str) -> String {
    let nfkc: String = value.nfkc().collect();
    let mut out = String::with_capacity(nfkc.len());
    for c in nfkc.chars() {
        if forbidden_char(c) {
            out.push('_');
        } else if is_control_or_noncharacter(c) {
            // drop the char entirely
        } else {
            out.push(c);
        }
    }
    // Collapse leading/trailing dots and trailing spaces (Windows normalizes
    // trailing dots/spaces away, so Echo does it deterministically first).
    while out.starts_with('.') {
        out.remove(0);
    }
    while out.ends_with('.') || out.ends_with(' ') {
        out.pop();
    }
    if out.is_empty() {
        return "未命名".to_owned();
    }
    if is_windows_reserved(&out) {
        out.push('_');
    }
    out
}

/// Truncate a base name to at most `max_bytes` UTF-8 bytes, preserving the
/// extension and appending a short, deterministic hash so distinct inputs
/// don't collide after truncation.
///
/// Platform limits are typically 255 bytes per component on Linux/macOS and
/// ~255 UTF-16 units on Windows; Echo chooses a conservative byte cap and
/// guarantees the extension survives.
#[must_use]
pub fn truncate_component_with_extension(base: &str, extension: &str, max_bytes: usize) -> String {
    let ext = if extension.is_empty() {
        String::new()
    } else {
        format!(".{}", extension.trim_start_matches('.'))
    };
    let hash = short_hash(base);
    if base.len() + ext.len() <= max_bytes {
        return format!("{base}{ext}");
    }
    // The final component must fit in `max_bytes`: the stem + "~"+hash suffix
    // + extension together. If a pathological (multi-byte) extension alone
    // exceeds the cap, it is truncated too — the extension is never *dropped*
    // silently, only bounded like the rest of the component.
    let hash_overhead = hash.len() + 1; // `~hash`
    let ext_budget = ext
        .len()
        .min(max_bytes.saturating_sub(hash_overhead).max(1));
    let ext = truncate_utf8(&ext, ext_budget);
    let budget = max_bytes
        .saturating_sub(ext.len())
        .saturating_sub(hash_overhead);
    let stem = truncate_utf8(base, budget);
    format!("{stem}~{hash}{ext}")
}

/// Truncate `value` to at most `max_bytes` UTF-8 bytes, never splitting a
/// scalar in the middle.
fn truncate_utf8(value: &str, max_bytes: usize) -> String {
    let mut out = String::new();
    for c in value.chars() {
        if out.len() + c.len_utf8() > max_bytes {
            break;
        }
        out.push(c);
    }
    out
}

/// A short 6-hex FNV-1a-ish hash of a component (stable, not security).
#[must_use]
fn short_hash(value: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut h);
    format!("{:06x}", h.finish() & 0x00ff_ffff)
}

/// Build the full relative target path from artist / title / extension,
/// applying [`safe_component`] and [`truncate_component_with_extension`].
///
/// Handles the unknown-artist / unnamed-song fallbacks with visible,
/// deterministic Chinese labels ("未知艺人" / "未命名歌曲").
#[must_use]
pub fn build_target_path(
    artist: Option<&str>,
    title: Option<&str>,
    extension: &str,
    max_component_bytes: usize,
) -> String {
    let artist = artist.map(safe_component).filter(|a| !a.is_empty());
    let title = title.map(safe_component).filter(|t| !t.is_empty());
    let artist = artist.unwrap_or_else(|| "未知艺人".to_owned());
    let title = title.unwrap_or_else(|| "未命名歌曲".to_owned());
    let file = format!("{artist} - {title}");
    let cap = max_component_bytes.max(64);
    format!(
        "{}/{}",
        artist,
        truncate_component_with_extension(&file, extension, cap)
    )
}

#[cfg(test)]
mod property_tests {
    //! Property tests (task 2.3): the name rules hold for arbitrary Unicode
    //! text — Chinese, Japanese, emoji, combining marks, control chars and
    //! Windows reserved names — not just hand-picked cases.

    use super::*;
    use proptest::prelude::*;

    proptest! {
        // Idempotence: normalizing a normalized string is a no-op (the fold is
        // NFKC once; a second pass is identical).
        #[test]
        fn normalize_text_is_idempotent(s in "\\PC{0,40}") {
            let once = normalize_text(&s);
            prop_assert_eq!(normalize_text(&once), once);
        }

        // Grapheme count is never larger than the scalar count, and is zero
        // exactly for the empty string.
        #[test]
        fn grapheme_count_bounds(s in "\\PC{0,80}") {
            let scalars = s.chars().count();
            let g = grapheme_count(&s);
            prop_assert!(g <= scalars);
            prop_assert_eq!(g == 0, s.is_empty());
        }

        // A normalized key is insensitive to case, width and surrounding space.
        #[test]
        fn normalized_key_is_stable(s in "\\PC{0,20}", pad in " {0,3}") {
            let k1 = normalized_key(&format!("{pad}{s}{pad}"));
            let k2 = normalized_key(&s);
            prop_assert_eq!(k1, k2);
        }

        // safe_component never leaves forbidden characters, control chars or
        // Windows-reserved names in the output.
        #[test]
        fn safe_component_never_emits_forbidden(s in "\\PC{0,40}") {
            let out = safe_component(&s);
            for ch in out.chars() {
                prop_assert!(!forbidden_char(ch));
                prop_assert!(!is_control_or_noncharacter(ch));
            }
            prop_assert!(!matches!(out.as_str(), "CON" | "PRN" | "AUX" | "NUL" | "COM1" | "LPT1"));
        }

        // Truncation keeps a recognizable extension and never exceeds the byte cap.
        #[test]
        fn truncation_respects_byte_cap_and_extension(
            base in "\\PC{1,120}",
            ext in "\\PC{0,5}",
            cap in 16usize..=64,
        ) {
            let out = truncate_component_with_extension(&base, &ext, cap);
            prop_assert!(out.len() <= cap, "component never exceeds the byte cap");
            if !ext.is_empty() {
                // When the extension fits, it is preserved verbatim (after the
                // dot prefix); otherwise it is bounded but never dropped.
                let ext_clean = ext.trim_start_matches('.');
                let expected = format!(".{ext_clean}");
                if expected.len() <= cap - "~000000".len() {
                    prop_assert!(out.ends_with(&expected), "extension preserved");
                } else {
                    prop_assert!(out.contains('.'), "a bounded extension chunk survives");
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_text_nfkc_and_trims() {
        // Full-width ＡＢＣ → ASCII abc through NFKC + lower? NFKC alone maps
        // full-width to ASCII; case stays. The key folds case too.
        assert_eq!(normalize_text("ＡＢＣ"), "ABC");
        assert_eq!(normalize_text("  AB   CD  "), "AB CD");
        // Ligatures: ㍿ is a compatibility character → "株式会社".
        assert_eq!(normalize_text("㍿"), "株式会社");
    }

    #[test]
    fn normalized_key_is_case_and_compat_insensitive() {
        assert_eq!(normalized_key("晴天"), normalized_key("晴天"));
        assert_eq!(normalized_key("晴天"), normalized_key(" 晴天 "));
        assert_eq!(normalized_key("ＡＢＣ"), normalized_key("abc"));
        assert_eq!(normalized_key("Echo"), normalized_key("echo"));
        assert_eq!(normalized_key("Straße"), normalized_key("STRASSE"));
        assert_eq!(normalized_key("ΟΣ"), normalized_key("οσ"));
        assert_eq!(normalized_key("ΟΣ"), normalized_key("ος"));
    }

    #[test]
    fn grapheme_counts_are_user_perceived() {
        // A family emoji is 1 grapheme (multiple scalars).
        assert_eq!(grapheme_count("👨‍👩‍👧‍👦"), 1);
        // e + combining acute = 1 grapheme, 2 scalars.
        assert_eq!(grapheme_count("e\u{301}"), 1);
        // Chinese chars are 1 grapheme each.
        assert_eq!(grapheme_count("晴天"), 2);
        // A long ASCII string.
        assert_eq!(grapheme_count("abcdefghij"), 10);
    }

    #[test]
    fn playlist_name_validation() {
        assert!(validate_playlist_name("通勤路上").is_ok());
        assert!(validate_playlist_name("  ").is_err());
        assert!(validate_playlist_name("").is_err());
        let long = "长".repeat(41);
        assert!(validate_playlist_name(&long).is_err());
        let ok40 = "长".repeat(40);
        assert!(validate_playlist_name(&ok40).is_ok());
    }

    #[test]
    fn playlist_name_keys_dedup_case_and_width() {
        let a = playlist_name_key("夜曲");
        let b = playlist_name_key("　夜曲　"); // full-width spaces
        assert_eq!(a, b);
        let c = playlist_name_key("ABC");
        let d = playlist_name_key("abc");
        assert_eq!(c, d);
    }

    #[test]
    fn safe_component_cleans_path_and_control_chars() {
        assert_eq!(safe_component("A/B"), "A_B");
        assert_eq!(safe_component("A\\B"), "A_B");
        assert_eq!(safe_component("A:B"), "A_B");
        assert_eq!(safe_component("A*B"), "A_B");
        assert_eq!(safe_component("x\u{1}y"), "xy");
        assert_eq!(safe_component(".hidden"), "hidden");
        assert_eq!(safe_component("magic."), "magic");
        assert_eq!(safe_component("magic "), "magic");
    }

    #[test]
    fn safe_component_handles_windows_reserved_names() {
        assert_eq!(safe_component("CON"), "CON_");
        assert_eq!(safe_component("nul"), "nul_");
        assert_eq!(safe_component("COM1"), "COM1_");
        // A normal name is untouched.
        assert_eq!(safe_component("晴天"), "晴天");
    }

    #[test]
    fn truncation_preserves_extension() {
        let out = truncate_component_with_extension("周杰伦 - 晴天", "flac", 32);
        assert!(out.to_ascii_lowercase().ends_with(".flac"), "{out}");
        assert!(out.len() <= 32, "{} <= 32 bytes", out.len());

        // Distinct truncations get distinct suffixes.
        let a = truncate_component_with_extension("一首很长的歌名", "mp3", 20);
        let b = truncate_component_with_extension("另一首很长的歌名", "mp3", 20);
        assert_ne!(a, b);
        assert!(a.to_ascii_lowercase().ends_with(".mp3"));
    }

    #[test]
    fn build_target_path_handles_missing_tags() {
        let with_both = build_target_path(Some("周杰伦"), Some("晴天"), "flac", 255);
        assert_eq!(with_both, "周杰伦/周杰伦 - 晴天.flac");

        let unknown_artist = build_target_path(None, Some("晴天"), "flac", 255);
        assert_eq!(unknown_artist, "未知艺人/未知艺人 - 晴天.flac");

        let unnamed = build_target_path(Some("周杰伦"), None, "mp3", 255);
        assert_eq!(unnamed, "周杰伦/周杰伦 - 未命名歌曲.mp3");
    }

    #[test]
    fn build_target_path_no_overlap_after_truncation() {
        // Ensure the published rule: the target never equals a source name by
        // construction — the file basename always has the artist prefix.
        let out = build_target_path(Some("王一"), Some("晴天"), "flac", 16);
        assert!(!out.is_empty());
        assert!(out.contains('/'), "{out}");
        assert!(out.to_ascii_lowercase().ends_with(".flac"), "{out}");
    }
}
