//! Media value objects and diagnostics (task 2.2).
//!
//! `media.rs` holds the *value objects* that describe a parsed media file:
//! the metadata bag, the audio format parameters, and a stable per-file
//! diagnostic. The [`Song`](crate::domain::entities::Song) entity consumes
//! these; the diagnostics themselves belong to the scan/import pipeline.
//!
//! None of these types carry absolute paths, raw file contents, or acceptable
//! forms of lyric/tag text that would break the logging privacy rule.

use std::time::Duration;

/// Format family, kept small and stable (the `MediaProbe` port produces these).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AudioFormat {
    #[default]
    Mpeg,
    Flac,
    /// MP4 container (`.m4a` or `.mp4` with an audio track).
    Mp4,
    Ogg,
    Opus,
    Wav,
    /// A supported container in an unsupported/damaged state.
    UnknownDamaged,
}

impl AudioFormat {
    /// The canonical lower-case extension this format maps to on disk.
    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self {
            Self::Mpeg => "mp3",
            Self::Flac => "flac",
            Self::Mp4 => "m4a",
            Self::Ogg => "ogg",
            Self::Opus => "opus",
            Self::Wav => "wav",
            Self::UnknownDamaged => "unknown",
        }
    }
}

impl std::fmt::Display for AudioFormat {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.extension())
    }
}

/// Audio stream parameters from the media probe.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AudioParameters {
    /// Bits per second, approximate (from container/stream).
    pub bitrate_bps: Option<u64>,
    /// Sample rate in Hz.
    pub sample_rate_hz: Option<u32>,
    /// Channel count (1 = mono, 2 = stereo…).
    pub channels: Option<u16>,
    /// Bits per sample (16/24/32 for PCM formats).
    pub bits_per_sample: Option<u16>,
}

/// The parsed, non-authoritative metadata bag from a tag read.
///
/// Values are cleaned display strings (NFKC, control characters stripped —
/// see [`crate::domain::text`]). Duration and format are *stream* facts: a tag
/// reader must leave them absent and let the [`MediaProbe`](crate::application::ports::MediaProbe)
/// outcome fill them, because tags are never authoritative for streams.
///
/// Embedded lyrics and covers ride along so a scan's parse worker can persist
/// them into their separate stores (design §7) without a second file read.
/// Input limits (tag 4 KiB / lyrics 2 MiB / cover 20 MiB) are enforced by the
/// reader: an over-limit asset is dropped and reported as a [`ParseWarning`],
/// never as a song failure.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ParsedMetadata {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub genre: Option<String>,
    pub track: Option<u32>,
    pub duration: Option<Duration>,
    /// The format family (from the probe, not the extension).
    pub format: AudioFormat,
    pub parameters: AudioParameters,
    /// Raw embedded lyrics text (before LRC parsing), when present and within
    /// the input limit.
    pub embedded_lyrics: Option<String>,
    /// Embedded cover art bytes, when present and within the input limit.
    pub cover: Option<EmbeddedCover>,
    /// Non-fatal parse diagnostics: over-limit assets, undecodable fields.
    /// A warning never blocks the song record.
    pub warnings: Vec<ParseWarning>,
}

impl ParsedMetadata {
    /// The music key used for conservative weak re-linking: only applied when
    /// both sides are unique and duration is within the tolerance.
    #[must_use]
    pub fn music_key(&self) -> Option<MusicKey> {
        Some(MusicKey {
            artist: self.artist.clone(),
            album: self.album.clone(),
            title: self.title.clone(),
            duration_seconds: self.duration.map(|d| d.as_secs()),
        })
    }
}

/// Embedded cover art as read from the audio file (task 4.3).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmbeddedCover {
    pub bytes: Vec<u8>,
    /// MIME type as declared by the tag (e.g. `image/jpeg`).
    pub mime: String,
}

/// Why a tag reader skipped or degraded one asset (task 4.4).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParseWarningKind {
    /// A single tag field exceeded the 4 KiB input limit.
    TagLimit,
    /// Embedded lyrics exceeded the 2 MiB input limit.
    LyricsLimit,
    /// Embedded cover exceeded the 20 MiB input limit.
    CoverLimit,
    /// A field could not be decoded and was dropped.
    TagDecode,
}

impl ParseWarningKind {
    /// Stable machine code recorded in `scan_issues`.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::TagLimit => "tag_limit",
            Self::LyricsLimit => "lyrics_limit",
            Self::CoverLimit => "cover_limit",
            Self::TagDecode => "tag_decode",
        }
    }
}

/// One non-fatal, path-free parse diagnostic attached to a [`ParsedMetadata`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ParseWarning {
    pub kind: ParseWarningKind,
    /// Which input hit the limit (e.g. `tag:title`, `cover`) — never content.
    pub field: String,
}

/// The `(artist, album, title, duration)` music key for weak re-linking.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MusicKey {
    pub artist: Option<String>,
    pub album: Option<String>,
    pub title: Option<String>,
    pub duration_seconds: Option<u64>,
}

/// A media probe result that failed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProbeFailure {
    pub reason: String,
}

/// A normalized, sanitized display string with its stable sort key.
///
/// Echo applies NFKC + case folding to the *sort* form and keeps the raw
/// (fallbacked) display string for UI.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DisplayValue {
    pub display: String,
    pub sort: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_extension_is_stable() {
        assert_eq!(AudioFormat::Mpeg.extension(), "mp3");
        assert_eq!(AudioFormat::Opus.extension(), "opus");
        assert_eq!(AudioFormat::Wav.extension(), "wav");
    }

    #[test]
    fn music_key_is_optional_on_duration() {
        let md = ParsedMetadata {
            title: Some("晴天".into()),
            artist: Some("周杰伦".into()),
            duration: Some(Duration::from_secs(269)),
            ..Default::default()
        };
        let key = md.music_key().unwrap();
        assert_eq!(key.artist.as_deref(), Some("周杰伦"));
        assert_eq!(key.duration_seconds, Some(269));

        let empty = ParsedMetadata::default();
        assert!(empty.music_key().is_some(), "even empty tags form a key");
    }

    #[test]
    fn parsed_metadata_defaults_are_absent() {
        let md = ParsedMetadata::default();
        assert!(md.title.is_none());
        assert_eq!(md.format, AudioFormat::default());
        assert!(md.duration.is_none());
        assert!(md.embedded_lyrics.is_none());
        assert!(md.cover.is_none());
        assert!(md.warnings.is_empty());
    }

    #[test]
    fn parse_warning_kinds_have_stable_codes() {
        assert_eq!(ParseWarningKind::TagLimit.code(), "tag_limit");
        assert_eq!(ParseWarningKind::LyricsLimit.code(), "lyrics_limit");
        assert_eq!(ParseWarningKind::CoverLimit.code(), "cover_limit");
        assert_eq!(ParseWarningKind::TagDecode.code(), "tag_decode");
    }

    #[test]
    fn embedded_cover_carries_bytes_and_mime() {
        let cover = EmbeddedCover {
            bytes: vec![1, 2, 3],
            mime: "image/jpeg".into(),
        };
        let md = ParsedMetadata {
            cover: Some(cover.clone()),
            ..Default::default()
        };
        assert_eq!(md.cover, Some(cover));
    }
}
