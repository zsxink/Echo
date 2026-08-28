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
}

/// Parse one file's tags (adapter-internal, also drives the unit tests).
pub(crate) fn read_from_file(abs: &Path, limits: &InputLimits) -> Result<ParsedMetadata, Error> {
    let file =
        std::fs::File::open(abs).map_err(|source| Error::io("open for tags", source, abs))?;
    let tagged = Probe::new(BufReader::new(file))
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
}
