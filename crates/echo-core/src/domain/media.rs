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
    Ape,
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
            Self::Ape => "ape",
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

/// Which embedded loudness tagging scheme produced a [`LoudnessTags`] value.
///
/// `ReplayGain` 2.0 tags are expressed in dB relative to -18 LUFS; R128 tags
/// are Q7.8 fixed-point values relative to -23 LUFS. The desktop rule layer
/// owns the conversion and the alignment between the two references.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LoudnessSource {
    ReplayGain2,
    R128,
}

impl LoudnessSource {
    /// Stable database encoding (never a display string).
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::ReplayGain2 => "replaygain2",
            Self::R128 => "r128",
        }
    }

    /// Decode a database value; unknown codes are treated as absent.
    #[must_use]
    pub fn from_code(value: &str) -> Option<Self> {
        match value {
            "replaygain2" => Some(Self::ReplayGain2),
            "r128" => Some(Self::R128),
            _ => None,
        }
    }
}

/// Per-track loudness tags read from the media file, never written back.
///
/// Values are stored as fixed-point integers (milli-dB and parts-per-million
/// of full scale) so the metadata bag and the [`Song`](crate::domain::entities::Song)
/// entity keep their `Eq` identity: float fields would make every comparison
/// and every persisted row ambiguous. The desktop rule layer converts them to
/// `f64` dB / amplitude at the point of use.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoudnessTags {
    pub source: LoudnessSource,
    /// Track gain in milli-dB (`dB * 1000`).
    pub track_gain_mdb: Option<i32>,
    /// Album gain in milli-dB (`dB * 1000`).
    pub album_gain_mdb: Option<i32>,
    /// Track sample peak as amplitude * `1_000_000`.
    pub track_peak_ppm: Option<u32>,
    /// Album sample peak as amplitude * `1_000_000`.
    pub album_peak_ppm: Option<u32>,
}

impl LoudnessTags {
    #[must_use]
    pub const fn new(
        source: LoudnessSource,
        track_gain_mdb: Option<i32>,
        album_gain_mdb: Option<i32>,
        track_peak_ppm: Option<u32>,
        album_peak_ppm: Option<u32>,
    ) -> Self {
        Self {
            source,
            track_gain_mdb,
            album_gain_mdb,
            track_peak_ppm,
            album_peak_ppm,
        }
    }

    /// Whether any usable gain is present.
    #[must_use]
    pub const fn has_gain(self) -> bool {
        self.track_gain_mdb.is_some() || self.album_gain_mdb.is_some()
    }

    #[must_use]
    pub fn gain_db(value: Option<i32>) -> Option<f64> {
        Some(f64::from(value?) / 1_000.0)
    }

    #[must_use]
    pub fn peak(value: Option<u32>) -> Option<f64> {
        Some(f64::from(value?) / 1_000_000.0)
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
    /// Embedded loudness tags (`ReplayGain` 2.0 / R128), when present and valid.
    /// Scan-derived facts: the file itself is never modified.
    pub loudness: Option<LoudnessTags>,
    /// Raw embedded lyrics text (before LRC parsing), when present and within
    /// the input limit.
    pub embedded_lyrics: Option<String>,
    /// Embedded cover art bytes, when present and within the input limit.
    pub cover: Option<EmbeddedCover>,
    /// Non-fatal parse diagnostics: over-limit assets, undecodable fields.
    /// A warning never blocks the song record.
    pub warnings: Vec<ParseWarning>,
}

/// Derive a disposable cover-cache key from a validated original content hash.
#[must_use]
pub fn cover_asset_key(content_hash: &str) -> String {
    format!("cv1-{content_hash}")
}

/// Embedded cover art as read from the audio file (task 4.3).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EmbeddedCover {
    pub bytes: Vec<u8>,
    /// Cover MIME declared by the tag and decodable by the thumbnail
    /// pipeline (e.g. `image/jpeg`); unsupported declarations are stored as
    /// `application/octet-stream`.
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
    /// An embedded loudness tag was malformed or uses an unsupported encoding.
    LoudnessTag,
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
            Self::LoudnessTag => "loudness_tag",
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
    fn parsed_metadata_defaults_are_absent() {
        let md = ParsedMetadata::default();
        assert!(md.title.is_none());
        assert_eq!(md.format, AudioFormat::default());
        assert!(md.duration.is_none());
        assert!(md.embedded_lyrics.is_none());
        assert!(md.cover.is_none());
        assert!(md.loudness.is_none());
        assert!(md.warnings.is_empty());
    }

    #[test]
    fn parse_warning_kinds_have_stable_codes() {
        assert_eq!(ParseWarningKind::TagLimit.code(), "tag_limit");
        assert_eq!(ParseWarningKind::LyricsLimit.code(), "lyrics_limit");
        assert_eq!(ParseWarningKind::CoverLimit.code(), "cover_limit");
        assert_eq!(ParseWarningKind::TagDecode.code(), "tag_decode");
        assert_eq!(ParseWarningKind::LoudnessTag.code(), "loudness_tag");
    }

    #[test]
    fn loudness_source_codes_round_trip() {
        for source in [LoudnessSource::ReplayGain2, LoudnessSource::R128] {
            assert_eq!(LoudnessSource::from_code(source.code()), Some(source));
        }
        assert_eq!(LoudnessSource::from_code("itunnorm"), None);
    }

    #[test]
    fn loudness_tags_fixed_point_conversion() {
        let tags = LoudnessTags::new(
            LoudnessSource::ReplayGain2,
            Some(-6_540),
            None,
            Some(987_650),
            None,
        );
        assert!(tags.has_gain());
        assert_eq!(LoudnessTags::gain_db(tags.track_gain_mdb), Some(-6.54));
        assert_eq!(LoudnessTags::peak(tags.track_peak_ppm), Some(0.98765));
        assert_eq!(LoudnessTags::gain_db(None), None);
        assert_eq!(LoudnessTags::peak(None), None);
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
