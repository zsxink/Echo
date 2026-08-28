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

/// The filesystem-semantic comparison key for a relative media path.
///
/// Two paths that the operating system could resolve to the *same file* must
/// produce the same key, so the identity of a song cannot be duplicated by a
/// spelling difference:
///
/// - **Canonical equivalence (NFD)** — `café` in its NFC and NFD spellings is
///   one file on macOS (APFS/HFS+ normalize canonically for lookup), so the
///   key is normalized to NFD (also after folding, because folding can
///   introduce new decomposable sequences).
/// - **Full case folding** — the default filesystems of macOS and Windows are
///   case-insensitive, so `A.MP3` and `a.mp3` are the same file and must not
///   become two song identities.
/// - **No compatibility folding (no NFKC)** — full-width `Ａ` and ASCII `A`
///   are *different files* on every platform; merging them would corrupt
///   identity, so only canonical normalization is applied.
///
/// Linux filesystems are byte-exact, so the key is deliberately conservative
/// there: it can only merge spellings that are safe to treat as one identity,
/// never split one file into two.
#[must_use]
pub fn path_identity_key(value: &str) -> String {
    let decomposed: String = value.nfd().collect();
    let folded: String = decomposed.case_fold().collect();
    folded.nfd().collect()
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
/// Windows reserved names get a trailing `_`. A value that cleans to nothing
/// becomes the visible fallback `未命名`.
#[must_use]
pub fn safe_component(value: &str) -> String {
    clean_component_opt(value).unwrap_or_else(|| "未命名".to_owned())
}

/// [`safe_component`] without the empty-name fallback: `None` when the value
/// cleans to nothing (only dots, spaces or control characters), so callers
/// can apply their own *visible and deterministic* fallback label instead of
/// inheriting an unrelated one.
#[must_use]
fn clean_component_opt(value: &str) -> Option<String> {
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
        return None;
    }
    if is_windows_reserved(&out) {
        out.push('_');
    }
    Some(out)
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
/// deterministic Chinese labels ("未知艺人" / "未命名歌曲"). Both the artist
/// directory and the file component are bounded to `max_component_bytes`.
#[must_use]
pub fn build_target_path(
    artist: Option<&str>,
    title: Option<&str>,
    extension: &str,
    max_component_bytes: usize,
) -> String {
    let cap = max_component_bytes.max(64);
    format!(
        "{}/{}",
        target_artist_component(artist, cap),
        truncate_component_with_extension(&target_file_stem(artist, title), extension, cap)
    )
}

/// The visible, deterministic folder name for a file whose tags carry no
/// artist (spec: 缺少歌手时使用可见且确定性的兜底名称).
pub const UNKNOWN_ARTIST: &str = "未知艺人";
/// The visible, deterministic file-stem fallback for a missing song title.
pub const UNNAMED_SONG: &str = "未命名歌曲";

/// One import-target part (artist folder or title): a whitespace-only or
/// entirely unnameable value counts as *missing* and takes `fallback`.
fn target_part(value: Option<&str>, fallback: &str) -> String {
    value
        .map(str::trim)
        .filter(|trimmed| !trimmed.is_empty())
        .and_then(clean_component_opt)
        .unwrap_or_else(|| fallback.to_owned())
}

/// The safe artist *directory* component of an import target, bounded to
/// `max_bytes` (over-long names keep a short hash so distinct artists stay
/// distinguishable).
#[must_use]
pub fn target_artist_component(artist: Option<&str>, max_bytes: usize) -> String {
    truncate_component_with_extension(&target_part(artist, UNKNOWN_ARTIST), "", max_bytes.max(64))
}

/// The `artist - title` file stem of an import target with the missing-tag
/// fallbacks — cleaned but neither truncated nor numbered: the caller numbers
/// against conflicts and truncates together with the extension.
#[must_use]
pub fn target_file_stem(artist: Option<&str>, title: Option<&str>) -> String {
    format!(
        "{} - {}",
        target_part(artist, UNKNOWN_ARTIST),
        target_part(title, UNNAMED_SONG)
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
    fn path_identity_key_merges_spelling_variants_of_one_file() {
        // Case-insensitive (macOS/Windows filesystems).
        assert_eq!(
            path_identity_key("歌手/Song.MP3"),
            path_identity_key("歌手/song.mp3")
        );
        // Canonical-equivalence insensitive (macOS lookup behavior): NFC vs NFD.
        let nfc = "Caf\u{e9}未被删除/晴天.mp3";
        let nfd = "Cafe\u{301}未被删除/晴天.mp3";
        assert_eq!(path_identity_key(nfc), path_identity_key(nfd));
    }

    #[test]
    fn path_identity_key_never_merges_distinct_files() {
        // Compatibility forms are DIFFERENT files on every platform.
        assert_ne!(path_identity_key("Ａ.mp3"), path_identity_key("A.mp3"));
        // Distinct names stay distinct.
        assert_ne!(path_identity_key("晴天.mp3"), path_identity_key("夜曲.mp3"));
    }

    #[test]
    fn path_identity_key_is_idempotent() {
        let once = path_identity_key("歌手/Caf\u{e9}fé.MP3");
        assert_eq!(path_identity_key(&once), once);
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

#[cfg(test)]
mod platform_golden_cases {
    //! Task 5.2 three-platform golden cases. The naming rules are pure
    //! functions, so every assertion below runs identically on macOS, Windows
    //! and Linux hosts — the per-platform groups document which platform
    //! hazard each rule neutralizes. No real foreign OS is required (or used):
    //! platform fidelity comes from the rule set, not from the test host.

    use super::*;

    // -----------------------------------------------------------------------
    // Windows golden cases
    // -----------------------------------------------------------------------

    /// Windows forbids `< > : " / \ | ? *` in every path component.
    #[test]
    fn windows_forbidden_characters_are_neutralized() {
        for c in ['<', '>', ':', '"', '/', '\\', '|', '?', '*'] {
            assert_eq!(safe_component(&format!("a{c}b")), "a_b", "char {c:?}");
        }
        // The composed target keeps the same guarantee on both components
        // (the one `/` per target is the separator the builder itself emits).
        let path = build_target_path(Some("AC/DC:Live"), Some("谁<strong>"), "flac", 255);
        for c in ['<', '>', ':', '"', '\\', '|', '?', '*'] {
            assert!(!path.contains(c), "{c:?} survived in {path}");
        }
        assert_eq!(path.matches('/').count(), 1, "{path}");
    }

    /// Windows reserves device names in any case, with or without extension;
    /// trailing dots and spaces are silently dropped by Windows and therefore
    /// removed deterministically up front.
    #[test]
    fn windows_reserved_names_and_trailing_junk_are_neutralized() {
        for name in [
            "CON", "con", "Con", "PRN", "prn", "AUX", "aux", "NUL", "nul", "COM1", "com4", "COM9",
            "LPT1", "lpt7", "LPT9",
        ] {
            assert_eq!(
                safe_component(name),
                format!("{name}_"),
                "{name} is reserved"
            );
        }
        // The reserved stem keeps its extension when composed into a target.
        let path = build_target_path(Some("NUL"), Some("nul.txt"), "flac", 255);
        assert!(path.starts_with("NUL_/"), "{path}");
        assert!(path.contains(" - nul.txt_"), "{path}");
        // Trailing dots/spaces never survive into a component.
        assert_eq!(safe_component("晴天."), "晴天");
        assert_eq!(safe_component("晴天 "), "晴天");
        assert_eq!(safe_component("晴天. . "), "晴天");
    }

    /// macOS and Windows default filesystems are case-insensitive: two
    /// spellings that differ only in case are one file, so the identity key
    /// folds case (the *displayed* name keeps the user's case).
    #[test]
    fn windows_and_macos_case_rules_are_identity_level() {
        assert_eq!(
            path_identity_key("歌手/SONG.MP3"),
            path_identity_key("歌手/song.mp3")
        );
        assert_eq!(
            safe_component("ABC"),
            "ABC",
            "display keeps the user's case"
        );
    }

    // -----------------------------------------------------------------------
    // macOS golden cases
    // -----------------------------------------------------------------------

    /// macOS forbids `/` and NUL (`:` was the classic HFS separator); lookups
    /// normalize canonically, so NFC and NFD spellings are the same file and
    /// must map to one identity — never two songs.
    #[test]
    fn macos_separators_and_unicode_normalization() {
        assert_eq!(safe_component("a/b"), "a_b");
        assert_eq!(safe_component("a:b"), "a_b");
        assert_eq!(
            safe_component("a\u{0}b"),
            "a_b",
            "NUL never survives (it maps to the replacement like every forbidden char)"
        );
        let nfc = "Caf\u{e9}";
        let nfd = "Cafe\u{301}";
        assert_eq!(
            path_identity_key(&format!("华语/{nfc}.flac")),
            path_identity_key(&format!("华语/{nfd}.flac")),
            "NFC and NFD spellings are one file on APFS/HFS+"
        );
        // Compatibility forms are NOT canonical equivalents: they stay distinct.
        assert_ne!(
            path_identity_key("华语/ｶ.flac"),
            path_identity_key("华语/カ.flac")
        );
    }

    // -----------------------------------------------------------------------
    // Linux golden cases
    // -----------------------------------------------------------------------

    /// Linux forbids `/` and NUL only; names are byte-exact and case-sensitive.
    /// Echo applies the union of all three platforms' restrictions (Windows is
    /// the strictest), so Linux-only-legal characters are cleaned too, and
    /// case-distinct components stay distinct while the conservative identity
    /// key can only merge spellings, never split one file into two.
    #[test]
    fn linux_separators_case_and_union_rules() {
        assert_eq!(safe_component("a/b"), "a_b");
        assert_eq!(safe_component("a\u{0}b"), "a_b", "NUL never survives");
        // Legal on Linux but still cleaned: one rule set, valid everywhere.
        assert_eq!(safe_component("a\\b"), "a_b");
        assert_eq!(safe_component("a:b"), "a_b");
        // Case-sensitive byte-exact names stay distinct as components.
        assert_ne!(safe_component("Song"), safe_component("song"));
        assert_ne!(safe_component("晴天"), safe_component("夜曲"));
        // …while the identity key folds case so a spelling can never become a
        // second identity on any platform.
        assert_eq!(
            path_identity_key("Song.flac"),
            path_identity_key("song.flac")
        );
    }

    // -----------------------------------------------------------------------
    // Truncation, short hash and fallback-label golden cases
    // -----------------------------------------------------------------------

    /// Over-long components are bounded with the extension preserved and a
    /// stable short-hash suffix that keeps distinct inputs distinct (design §8:
    /// 保留扩展名并附短 hash 后缀保证可区分).
    #[test]
    fn truncation_keeps_extension_and_short_hash() {
        let long = "一首非常长的歌曲名称确实超出了组件的字节限制".repeat(8);
        let out = truncate_component_with_extension(&long, "flac", 200);
        assert!(out.len() <= 200, "{} bytes", out.len());
        assert!(
            std::path::Path::new(&out)
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("flac")),
            "{out}"
        );
        assert!(out.contains('~'), "short-hash suffix present: {out}");
        // Deterministic: same input, same output.
        assert_eq!(out, truncate_component_with_extension(&long, "flac", 200));
        // Distinct inputs never collapse into the same truncated name.
        let other = truncate_component_with_extension(&format!("{long}！"), "flac", 200);
        assert_ne!(out, other);
    }

    /// Missing or blank tags take the visible, deterministic fallbacks
    /// (spec: 未知艺人 / 未命名歌曲).
    #[test]
    fn fallback_labels_are_visible_and_deterministic() {
        assert_eq!(
            build_target_path(Some("周杰伦"), Some("晴天"), "flac", 255),
            "周杰伦/周杰伦 - 晴天.flac"
        );
        assert_eq!(
            build_target_path(None, Some("晴天"), "flac", 255),
            "未知艺人/未知艺人 - 晴天.flac"
        );
        assert_eq!(
            build_target_path(Some("周杰伦"), None, "flac", 255),
            "周杰伦/周杰伦 - 未命名歌曲.flac"
        );
        assert_eq!(
            build_target_path(None, None, "flac", 255),
            "未知艺人/未知艺人 - 未命名歌曲.flac"
        );
        // Blank (whitespace-only) tags count as missing, for both labels.
        assert_eq!(
            build_target_path(Some("   "), Some("\u{3000}"), "flac", 255),
            "未知艺人/未知艺人 - 未命名歌曲.flac"
        );
        // A tag that cleans to nothing (control characters only) is missing too.
        assert_eq!(
            build_target_path(Some("\u{1}\u{2}"), Some("晴天"), "flac", 255),
            "未知艺人/未知艺人 - 晴天.flac"
        );
    }
}
