//! The lofty [`MetadataReader`](crate::application::ports::MetadataReader) with
//! the phase-4 input limits (tasks 4.3/4.4).
//!
//! Responsibilities:
//!
//! - Tags (title/artist/album/album-artist/genre/track) with Unicode NFKC,
//!   control-character cleanup and empty-value fallback (`None`, display
//!   fallbacks live above the adapter).
//! - Embedded lyrics and cover art, passed through to the scan pipeline for
//!   their separate stores (design §7).
//! - Stream-level audio parameters from the container properties — never from
//!   tag text. Duration/format stay unset: the probe owns those.
//! - Input limits (tag field 4 KiB, lyrics 2 MiB, cover 20 MiB): an over-limit
//!   asset is dropped with a [`ParseWarning`](crate::domain::media::ParseWarning)
//!   and never blocks the song record. The original file is never modified.

use std::io::BufReader;
use std::path::Path;

use lofty::prelude::*;
use lofty::probe::Probe;
use lofty::tag::ItemKey;

use crate::application::ports::MetadataReader;
use crate::domain::ids::{LibraryRootId, RelativeMediaPath};
use crate::domain::media::{
    AudioParameters, EmbeddedCover, ParseWarning, ParseWarningKind, ParsedMetadata,
};
use crate::error::Error;

use super::super::filesystem::registry::RootRegistry;

/// Per-input byte limits. Defaults per design: 4 KiB / 2 MiB / 20 MiB.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InputLimits {
    pub tag_field: usize,
    pub lyrics: usize,
    pub cover: usize,
}

/// The lofty-backed metadata reader.
#[derive(Clone, Debug)]
pub struct LoftyMetadataReader {
    registry: RootRegistry,
    limits: InputLimits,
}

impl LoftyMetadataReader {
    #[must_use]
    pub const fn new(registry: RootRegistry) -> Self {
        Self {
            registry,
            limits: InputLimits::defaults(),
        }
    }

    /// Custom limits (tests use tiny ones to exercise the skip paths).
    #[must_use]
    pub const fn with_limits(mut self, limits: InputLimits) -> Self {
        self.limits = limits;
        self
    }

    fn abs(
        &self,
        root: LibraryRootId,
        path: &RelativeMediaPath,
    ) -> Result<std::path::PathBuf, Error> {
        Ok(self.registry.path_of(root)?.join(path.normalized()))
    }
}

impl MetadataReader for LoftyMetadataReader {
    fn read(&self, root: LibraryRootId, path: &RelativeMediaPath) -> Result<ParsedMetadata, Error> {
        let abs = self.abs(root, path)?;
        read_from_file(&abs, &self.limits)
    }

    fn read_bytes(&self, content: &[u8]) -> Result<ParsedMetadata, Error> {
        read_from_reader(std::io::Cursor::new(content), &self.limits)
    }
}

/// Parse one file's tags (adapter-internal, also drives the unit tests).
pub(crate) fn read_from_file(abs: &Path, limits: &InputLimits) -> Result<ParsedMetadata, Error> {
    let file =
        std::fs::File::open(abs).map_err(|source| Error::io("open for tags", source, abs))?;
    read_from_reader(BufReader::new(file), limits)
}

/// Parse tags from any seekable reader (library file or in-memory import
/// source). Lofty errors never embed the path, so the mapped error stays
/// path-free by construction.
fn read_from_reader<R: std::io::Read + std::io::Seek>(
    reader: R,
    limits: &InputLimits,
) -> Result<ParsedMetadata, Error> {
    let tagged = Probe::new(reader)
        .guess_file_type()
        .map_err(|source| corrupt("tag probe", &source.to_string()))?
        .read()
        .map_err(|source| corrupt("tag read", &source.to_string()))?;

    let mut parsed = ParsedMetadata::default();
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
    if let Some(tag) = tag {
        parsed.title = limited_field(
            tag.title().as_deref(),
            "tag:title",
            limits.tag_field,
            &mut parsed,
        );
        parsed.artist = limited_field(
            tag.artist().as_deref(),
            "tag:artist",
            limits.tag_field,
            &mut parsed,
        );
        parsed.album = limited_field(
            tag.album().as_deref(),
            "tag:album",
            limits.tag_field,
            &mut parsed,
        );
        parsed.album_artist = limited_field(
            tag.get_string(&ItemKey::AlbumArtist),
            "tag:album_artist",
            limits.tag_field,
            &mut parsed,
        );
        parsed.genre = limited_field(
            tag.genre().as_deref(),
            "tag:genre",
            limits.tag_field,
            &mut parsed,
        );
        parsed.track = tag.track();
        parsed.embedded_lyrics =
            limited_lyrics(tag.get_string(&ItemKey::Lyrics), limits.lyrics, &mut parsed);
        if let Some(picture) = tag.pictures().first() {
            let data = picture.data();
            if data.len() > limits.cover {
                parsed.warnings.push(ParseWarning {
                    kind: ParseWarningKind::CoverLimit,
                    field: "cover".to_owned(),
                });
            } else if !data.is_empty() {
                parsed.cover = Some(EmbeddedCover {
                    bytes: data.to_vec(),
                    mime: mime_of(picture.mime_type()),
                });
            }
        }
    }
    // Stream parameters come from the container properties (lofty reads the
    // audio stream, not the tag text); duration/format stay probe-owned.
    let properties = tagged.properties();
    parsed.parameters = AudioParameters {
        bitrate_bps: properties.audio_bitrate().map(u64::from),
        sample_rate_hz: properties.sample_rate().filter(|rate| *rate > 0),
        channels: properties
            .channels()
            .filter(|channels| *channels > 0)
            .map(u16::from),
        bits_per_sample: properties.bit_depth().map(u16::from),
    };
    Ok(parsed)
}

/// Clean a raw tag value: control-character strip + NFKC + whitespace collapse.
/// `None` for values that are empty after cleanup.
fn limited_field(
    raw: Option<&str>,
    field: &str,
    limit: usize,
    parsed: &mut ParsedMetadata,
) -> Option<String> {
    let raw = raw?;
    if raw.len() > limit {
        parsed.warnings.push(ParseWarning {
            kind: warning_kind_for(field),
            field: field.to_owned(),
        });
        return None;
    }
    let cleaned = clean_display_text(raw);
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

const fn warning_kind_for(_field: &str) -> ParseWarningKind {
    // Only tag fields flow through `limited_field`; lyrics have their own
    // `limited_lyrics` path with the lyrics warning kind.
    ParseWarningKind::TagLimit
}

/// Lyrics cleaning keeps line structure: control characters are stripped per
/// line, each line is NFKC-normalized, blank lines collapse away.
#[must_use]
pub fn clean_lyrics_text(value: &str) -> String {
    value
        .lines()
        .map(|line| {
            let cleaned: String = line
                .chars()
                .filter(|c| !c.is_control() && !('\u{7f}'..='\u{9f}').contains(c))
                .collect();
            crate::domain::text::normalize_text(&cleaned)
        })
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

/// The lyrics variant of [`limited_field`]: line-preserving cleanup and its
/// own warning kind.
fn limited_lyrics(raw: Option<&str>, limit: usize, parsed: &mut ParsedMetadata) -> Option<String> {
    let raw = raw?;
    if raw.len() > limit {
        parsed.warnings.push(ParseWarning {
            kind: ParseWarningKind::LyricsLimit,
            field: "lyrics".to_owned(),
        });
        return None;
    }
    let cleaned = clean_lyrics_text(raw);
    if cleaned.is_empty() {
        None
    } else {
        Some(cleaned)
    }
}

/// NFKC + control-character cleanup + whitespace collapse (task 2.3 rules
/// applied to tag text; the source file is never touched).
#[must_use]
pub fn clean_display_text(value: &str) -> String {
    let without_controls: String = value
        .chars()
        .filter(|c| {
            // Keep newline/tab (multi-line lyrics), drop C0 controls, DEL and
            // the C1 range that wrecks display without being whitespace.
            matches!(*c, '\t' | '\n') || (!c.is_control() && !('\u{7f}'..='\u{9f}').contains(c))
        })
        .collect();
    crate::domain::text::normalize_text(&without_controls)
}

fn mime_of(mime: Option<&lofty::picture::MimeType>) -> String {
    use lofty::picture::MimeType;
    match mime {
        Some(MimeType::Png) => "image/png",
        Some(MimeType::Jpeg) => "image/jpeg",
        Some(MimeType::Tiff) => "image/tiff",
        Some(MimeType::Bmp) => "image/bmp",
        Some(MimeType::Gif) => "image/gif",
        Some(MimeType::Unknown(value)) => return value.clone(),
        Some(_) | None => "application/octet-stream",
    }
    .to_owned()
}

fn corrupt(operation: &str, reason: &str) -> Error {
    Error::CorruptMedia {
        operation: operation.to_owned(),
        reason: reason.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixtures_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/audio")
            .canonicalize()
            .expect("fixtures directory present")
    }

    fn read(fixture: &str) -> ParsedMetadata {
        read_from_file(&fixtures_dir().join(fixture), &InputLimits::defaults()).unwrap()
    }

    #[test]
    fn tags_and_parameters_parse_from_fixtures() {
        // Fixture tags are generated by scripts/gen-fixtures.mjs (the FLAC
        // variant renames the tone: "Flac Tone").
        let mp3 = read("tone-short.mp3");
        assert_eq!(mp3.title.as_deref(), Some("Echo Tone"));
        assert_eq!(mp3.artist.as_deref(), Some("Echo Fixtures"));
        assert_eq!(mp3.album.as_deref(), Some("Fixture Album"));
        assert!(
            mp3.parameters.sample_rate_hz == Some(44_100),
            "stream parameters come from the container: {:?}",
            mp3.parameters
        );
        // Duration/format are probe-owned, never tag-owned.
        assert!(mp3.duration.is_none());
        assert!(mp3.warnings.is_empty());

        let flac = read("tone-short.flac");
        assert_eq!(flac.title.as_deref(), Some("Flac Tone"));
        assert!(
            flac.cover.is_some(),
            "the FLAC fixture embeds the generated cover"
        );
    }

    /// Task 5.2: import sources are named from their bytes, before anything
    /// is written to the library — the reader must accept content directly
    /// and agree with the file-based read.
    #[test]
    fn tags_parse_from_in_memory_import_source_bytes() {
        let bytes = std::fs::read(fixtures_dir().join("tone-short.flac")).unwrap();
        let reader = || LoftyMetadataReader::new(RootRegistry::new());
        let from_bytes = reader().read_bytes(&bytes).unwrap();
        assert_eq!(from_bytes.title.as_deref(), Some("Flac Tone"));
        assert_eq!(from_bytes.artist.as_deref(), Some("Echo Fixtures"));
        // The content-based read agrees with the path-based read of the same
        // audio (the import naming and the committed record must not drift).
        let from_file = read("tone-short.flac");
        assert_eq!(from_bytes.title, from_file.title);
        assert_eq!(from_bytes.artist, from_file.artist);
        // Garbage content fails instead of producing a guessable name.
        assert!(reader().read_bytes(b"definitely not audio").is_err());
    }

    /// Task 12.7: hostile/malicious metadata must never panic, never leak an
    /// unbounded resource, and must keep the audio record importable. Control
    /// characters and NFKC junk in tags/lyrics are cleaned; oversized fields
    /// are dropped with a diagnostic (the real boundaries are the 4 KiB tag /
    /// 2 MiB lyrics / 20 MiB cover inputs, asserted separately in
    /// `input_limits_skip_assets_but_keep_song_fields`).
    #[test]
    fn hostile_tags_with_controls_and_oversized_text_are_cleaned_or_diagnosed() {
        // Control characters in the display title.
        let cleaned_title = clean_display_text("晴\u{0}\u{7f}天\t夜里");
        assert_eq!(
            cleaned_title, "晴天 夜里",
            "null/DEL stripped from tags; tab collapses to a display space"
        );

        // Null bytes and DEL hidden in lyrics are stripped per line, and the
        // empty-line collapse keeps the structure.
        let cleaned_lyrics = clean_lyrics_text("第一行\u{0}\n\u{7f}\n第二行");
        assert_eq!(cleaned_lyrics, "第一行\n第二行");
        assert!(!cleaned_lyrics.contains('\u{0}'));

        // An over-limit field (simulated at the same boundary the reader uses)
        // is dropped with the lyrics warning, never returned partially.
        let mut parsed = ParsedMetadata::default();
        let big = "\u{8d85}\u{9650}".repeat(1_048_576 / 2); // ~2 MiB
        let result = limited_lyrics(Some(&big), 2 * 1024 * 1024, &mut parsed);
        assert!(
            result.is_none(),
            "a bytes-over-limit lyrics field is dropped, not returned"
        );
        assert!(
            parsed
                .warnings
                .iter()
                .any(|w| w.kind == ParseWarningKind::LyricsLimit),
            "the over-limit lyrics field records a lyrics_limit diagnostic"
        );
    }

    /// Copy a fixture into a temp dir and write over-limit fields with lofty
    /// (the only way to build a genuinely over-limit tag without shipping a
    /// giant fixture). The temp dir is intentionally kept alive for the test.
    fn write_oversized_fixture(fixture: &str) -> std::path::PathBuf {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(fixture);
        std::fs::copy(fixtures_dir().join(fixture), &path).unwrap();
        let mut tagged = lofty::read_from_path(&path).unwrap();
        let has_primary = tagged.primary_tag().is_some();
        let tag = if has_primary {
            tagged.primary_tag_mut().expect("checked above")
        } else {
            tagged.first_tag_mut().expect("fixture carries a tag")
        };
        tag.set_title("\u{957f}".repeat(64)); // 192 bytes > the 32-byte limit
        tag.insert_text(
            ItemKey::Lyrics,
            "\u{8d85}\u{9650}\u{6b4c}\u{8bcd}".repeat(64), // 256 bytes > the 64-byte limit
        );
        tagged
            .save_to_path(&path, lofty::config::WriteOptions::default())
            .unwrap();
        std::mem::forget(dir);
        path
    }

    #[test]
    fn input_limits_skip_assets_but_keep_song_fields() {
        let oversized = write_oversized_fixture("tone-short.flac");
        let limits = InputLimits {
            tag_field: 32,
            lyrics: 64,
            cover: 128,
        };
        let meta = read_from_file(&oversized, &limits).unwrap();
        assert!(
            meta.title.is_none(),
            "over-limit tag fields are dropped: {meta:?}"
        );
        assert!(
            meta.embedded_lyrics.is_none(),
            "over-limit lyrics are dropped: {meta:?}"
        );
        assert!(
            meta.cover.is_none(),
            "over-limit covers are skipped: {meta:?}"
        );
        let kinds: Vec<_> = meta.warnings.iter().map(|w| w.kind).collect();
        assert!(kinds.contains(&ParseWarningKind::TagLimit), "{kinds:?}");
        assert!(kinds.contains(&ParseWarningKind::LyricsLimit), "{kinds:?}");
        assert!(kinds.contains(&ParseWarningKind::CoverLimit), "{kinds:?}");
        // The audio record is still importable: stream parameters survive.
        assert_eq!(meta.parameters.sample_rate_hz, Some(44_100));
    }

    #[test]
    fn clean_display_text_strips_controls_and_normalizes() {
        assert_eq!(clean_display_text("晴\u{0}天"), "晴天");
        assert_eq!(clean_display_text("  A\u{7f}B  "), "AB");
        assert_eq!(clean_display_text("ＡＢ"), "AB", "NFKC compatibility fold");
        assert_eq!(clean_display_text("line1\nline2"), "line1 line2");
    }

    #[test]
    fn clean_lyrics_text_keeps_line_structure() {
        assert_eq!(clean_lyrics_text("第一行\n\n第二行\u{0}"), "第一行\n第二行");
        assert_eq!(clean_lyrics_text("ＡＢ\nＣＤ"), "AB\nCD");
        assert_eq!(clean_lyrics_text("\n\n"), "");
    }

    #[test]
    fn mime_of_maps_every_variant_with_a_default_fallback() {
        use lofty::picture::MimeType;
        assert_eq!(mime_of(Some(&MimeType::Png)), "image/png");
        assert_eq!(mime_of(Some(&MimeType::Jpeg)), "image/jpeg");
        assert_eq!(mime_of(Some(&MimeType::Tiff)), "image/tiff");
        assert_eq!(mime_of(Some(&MimeType::Bmp)), "image/bmp");
        assert_eq!(mime_of(Some(&MimeType::Gif)), "image/gif");
        assert_eq!(
            mime_of(Some(&MimeType::Unknown("image/webp".to_owned()))),
            "image/webp"
        );
        assert_eq!(mime_of(None), "application/octet-stream");
    }

    /// The reader resolves the path through the root registry and maps a
    /// missing file to an io error — the branch the adapter surfaces to the
    /// scan pipeline before the layout probe.
    #[test]
    fn read_of_an_unregistered_root_or_missing_file_is_an_error() {
        let reader = LoftyMetadataReader::new(RootRegistry::new());
        let ghost = LibraryRootId::new();
        let path = RelativeMediaPath::new("missing.flac").unwrap();
        assert!(reader.read(ghost, &path).is_err());

        let dir = tempfile::tempdir().unwrap();
        let registry = RootRegistry::new();
        let root = LibraryRootId::new();
        registry.register(root, dir.path());
        let reader = LoftyMetadataReader::new(registry);
        assert!(reader.read(root, &path).is_err());
        // The in-memory reader rejects garbage content and returns a
        // corrupt-media classification, not a panic.
        let error = reader.read_bytes(b"definitely not audio").unwrap_err();
        assert_eq!(error.code(), "corrupt_media");
    }

    /// Tag values that collapse to empty after cleanup (`None`), and
    /// over-limit values dropped with the right warning kind, stay consistent
    /// between the field and lyrics variants.
    #[test]
    fn limited_field_and_lyrics_clean_to_empty_or_warn() {
        let mut parsed = ParsedMetadata::default();
        // Whitespace-only and control-only values collapse to None.
        assert_eq!(
            limited_field(Some("   "), "tag:title", 1024, &mut parsed),
            None
        );
        assert_eq!(
            limited_field(Some(""), "tag:title", 1024, &mut parsed),
            None
        );
        assert_eq!(limited_lyrics(Some("\u{0}\n\n"), 1024, &mut parsed), None);
        assert_eq!(limited_lyrics(Some(""), 1024, &mut parsed), None);
        assert!(parsed.warnings.is_empty(), "empty values never warn");

        // An over-limit field records exactly the TagLimit warning.
        let mut over = ParsedMetadata::default();
        let long = "长".repeat(200);
        let value = limited_field(Some(&long), "tag:genre", 32, &mut over);
        assert!(value.is_none());
        assert_eq!(over.warnings.len(), 1);
        assert_eq!(over.warnings[0].kind, ParseWarningKind::TagLimit);
        assert_eq!(over.warnings[0].field, "tag:genre");
    }

    /// The fixture carry no album-artist or track-number; the untouched tag
    /// fields stay `None` rather than producing junk values.
    #[test]
    fn absent_tags_stay_none_on_real_fixtures() {
        let mp3 = read("tone-short.mp3");
        assert_eq!(mp3.album_artist, None);
        assert_eq!(mp3.track, None);
        // The fixture embeds no cover; the parameters still come from the
        // container (probe-owned duration stays unset).
        assert!(mp3.cover.is_none());
        assert!(mp3.duration.is_none());
        assert!(
            mp3.parameters.sample_rate_hz == Some(44_100),
            "stream parameters parsed without a tag-aware branch"
        );
    }
}
