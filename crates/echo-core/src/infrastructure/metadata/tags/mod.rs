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

use std::fmt::Write as _;
use std::io::{BufReader, Cursor};
use std::path::Path;

use lofty::picture::Picture;
use lofty::prelude::*;
use lofty::probe::Probe;
use lofty::tag::{ItemKey, Tag, TagType};

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
        let mut parsed = read_from_reader(Cursor::new(content), &self.limits)?;
        if is_mp4_content(content) {
            enrich_mp4_metadata(content, &self.limits, &mut parsed);
        }
        Ok(parsed)
    }
}

/// Parse one file's tags (adapter-internal, also drives the unit tests).
pub(crate) fn read_from_file(abs: &Path, limits: &InputLimits) -> Result<ParsedMetadata, Error> {
    let file =
        std::fs::File::open(abs).map_err(|source| Error::io("open for tags", source, abs))?;
    let mut parsed = read_from_reader(BufReader::new(file), limits)?;
    let is_mp4_path = abs
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("m4a") || ext.eq_ignore_ascii_case("mp4"));
    if is_mp4_path {
        if let Ok(bytes) = std::fs::read(abs) {
            enrich_mp4_metadata(&bytes, limits, &mut parsed);
        }
    }
    Ok(parsed)
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
    // A WAV may carry both RIFF INFO and ID3v2 chunks. Read every tag instead
    // of stopping at the primary tag: text commonly lives in INFO while the
    // cover/lyrics live in ID3v2.
    for tag in tagged.tags() {
        if parsed.title.is_none() {
            parsed.title = limited_field(
                tag.title().as_deref(),
                "tag:title",
                limits.tag_field,
                &mut parsed,
            );
        }
        if parsed.artist.is_none() {
            parsed.artist = limited_field(
                tag.artist().as_deref(),
                "tag:artist",
                limits.tag_field,
                &mut parsed,
            );
        }
        if parsed.album.is_none() {
            parsed.album = limited_field(
                tag.album().as_deref(),
                "tag:album",
                limits.tag_field,
                &mut parsed,
            );
        }
        if parsed.album_artist.is_none() {
            parsed.album_artist = limited_field(
                tag.get_string(&ItemKey::AlbumArtist),
                "tag:album_artist",
                limits.tag_field,
                &mut parsed,
            );
        }
        if parsed.genre.is_none() {
            parsed.genre = limited_field(
                tag.genre().as_deref(),
                "tag:genre",
                limits.tag_field,
                &mut parsed,
            );
        }
        if parsed.track.is_none() {
            parsed.track = tag.track();
        }
        if parsed.embedded_lyrics.is_none() {
            parsed.embedded_lyrics =
                limited_lyrics(lyrics_from_tag(tag), limits.lyrics, &mut parsed);
        }
        if parsed.cover.is_none() {
            read_cover(tag, limits.cover, &mut parsed);
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

fn is_mp4_content(content: &[u8]) -> bool {
    content.get(4..8) == Some(b"ftyp")
}

/// Fill the gaps left by `QuickTime` files that use legacy `moov/udta` atoms
/// and media tracks instead of the iTunes `ilst` tag model. This is common for
/// files produced by older converters: text tags are direct `©nam`/`©ART`/
/// `©alb` atoms, while lyrics and artwork are `mov_text` and MJPEG tracks.
fn enrich_mp4_metadata(content: &[u8], limits: &InputLimits, parsed: &mut ParsedMetadata) {
    read_quicktime_udta_tags(content, limits, parsed);

    let stream = symphonia::core::io::MediaSourceStream::new(
        Box::new(Cursor::new(content.to_vec())),
        symphonia::core::io::MediaSourceStreamOptions::default(),
    );
    let probed = symphonia::default::get_probe().format(
        &symphonia::core::probe::Hint::new(),
        stream,
        &symphonia::core::formats::FormatOptions::default(),
        &symphonia::core::meta::MetadataOptions::default(),
    );
    let Ok(probed) = probed else {
        return;
    };
    let mut format = probed.format;
    let mut cover_found = parsed.cover.is_some();
    let mut lyrics = Vec::new();

    while let Ok(packet) = format.next_packet() {
        if !cover_found {
            let mime = image_mime(packet.data.as_ref());
            if let Some(mime) = mime {
                store_cover(packet.data.as_ref(), Some(&mime), limits.cover, parsed);
                cover_found = parsed.cover.is_some();
            }
        }

        let Some(text) = quicktime_subtitle_text(packet.data.as_ref()) else {
            continue;
        };
        let Some(time_base) = format
            .tracks()
            .iter()
            .find(|track| track.id == packet.track_id())
            .and_then(|track| track.codec_params.time_base)
        else {
            continue;
        };
        let time = time_base.calc_time(packet.ts());
        let timestamp_centis = time.seconds.saturating_mul(100).saturating_add(
            #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
            {
                (time.frac * 100.0).round() as u64
            },
        );
        lyrics.push((timestamp_centis, text.to_owned()));
    }

    if parsed.embedded_lyrics.is_none() && !lyrics.is_empty() {
        let mut raw = String::new();
        for (timestamp, text) in lyrics {
            let minutes = timestamp / 6000;
            let seconds = (timestamp / 100) % 60;
            let centis = timestamp % 100;
            let _ = writeln!(raw, "[{minutes:02}:{seconds:02}.{centis:02}]{text}");
        }
        parsed.embedded_lyrics = limited_lyrics(Some(&raw), limits.lyrics, parsed);
    }
}

fn read_quicktime_udta_tags(content: &[u8], limits: &InputLimits, parsed: &mut ParsedMetadata) {
    let mut offset = 0;
    while let Some((atom_type, payload_start, payload_end, next)) =
        quicktime_atom(content, offset, content.len())
    {
        if atom_type == *b"moov" {
            walk_quicktime_udta(content, payload_start, payload_end, limits, parsed);
        }
        offset = next;
    }
}

fn walk_quicktime_udta(
    content: &[u8],
    start: usize,
    end: usize,
    limits: &InputLimits,
    parsed: &mut ParsedMetadata,
) {
    let mut offset = start;
    while let Some((atom_type, payload_start, payload_end, next)) =
        quicktime_atom(content, offset, end)
    {
        if atom_type == *b"udta" {
            walk_quicktime_udta(content, payload_start, payload_end, limits, parsed);
        } else if let Some(value) = quicktime_text(content, payload_start, payload_end) {
            match atom_type {
                [0xA9, b'n', b'a', b'm'] if parsed.title.is_none() => {
                    parsed.title =
                        limited_field(Some(value), "tag:title", limits.tag_field, parsed);
                }
                [0xA9, b'A', b'R', b'T'] if parsed.artist.is_none() => {
                    parsed.artist =
                        limited_field(Some(value), "tag:artist", limits.tag_field, parsed);
                }
                [0xA9, b'a', b'l', b'b'] if parsed.album.is_none() => {
                    parsed.album =
                        limited_field(Some(value), "tag:album", limits.tag_field, parsed);
                }
                [b'a', b'A', b'R', b'T'] if parsed.album_artist.is_none() => {
                    parsed.album_artist =
                        limited_field(Some(value), "tag:album_artist", limits.tag_field, parsed);
                }
                [0xA9, b'g', b'e', b'n'] if parsed.genre.is_none() => {
                    parsed.genre =
                        limited_field(Some(value), "tag:genre", limits.tag_field, parsed);
                }
                _ => {}
            }
        }
        offset = next;
    }
}

fn quicktime_atom(
    content: &[u8],
    offset: usize,
    end: usize,
) -> Option<([u8; 4], usize, usize, usize)> {
    let header_end = offset.checked_add(8)?;
    if header_end > end {
        return None;
    }
    let size = u32::from_be_bytes(content[offset..offset + 4].try_into().ok()?);
    let atom_type = content[offset + 4..header_end].try_into().ok()?;
    let (header_len, atom_len) = if size == 1 {
        let extended_end = offset.checked_add(16)?;
        if extended_end > end {
            return None;
        }
        (
            16,
            u64::from_be_bytes(content[header_end..extended_end].try_into().ok()?),
        )
    } else if size == 0 {
        (8, (end - offset) as u64)
    } else {
        (8, u64::from(size))
    };
    let atom_len = usize::try_from(atom_len).ok()?;
    let atom_end = offset.checked_add(atom_len)?;
    if atom_len < header_len || atom_end > end {
        return None;
    }
    Some((atom_type, offset + header_len, atom_end, atom_end))
}

fn quicktime_text(content: &[u8], start: usize, end: usize) -> Option<&str> {
    let payload = content.get(start..end)?;
    let payload = if payload.len() >= 4
        && u16::from_be_bytes(payload[..2].try_into().ok()?) as usize == payload.len() - 4
    {
        let text_with_trailing_language = &payload[2..payload.len() - 2];
        if std::str::from_utf8(text_with_trailing_language).is_ok() {
            text_with_trailing_language
        } else {
            &payload[4..]
        }
    } else if payload.len() >= 2
        && u16::from_be_bytes(payload[..2].try_into().ok()?) as usize == payload.len() - 2
    {
        &payload[2..]
    } else {
        payload
    };
    std::str::from_utf8(payload)
        .ok()
        .filter(|value| !value.is_empty())
}

fn quicktime_subtitle_text(data: &[u8]) -> Option<&str> {
    if data.len() < 2 {
        return None;
    }
    let length = usize::from(u16::from_be_bytes([data[0], data[1]]));
    (length == data.len() - 2)
        .then(|| std::str::from_utf8(&data[2..]).ok())
        .flatten()
        .filter(|value| !value.trim().is_empty())
}

fn image_mime(data: &[u8]) -> Option<lofty::picture::MimeType> {
    if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(lofty::picture::MimeType::Jpeg)
    } else if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(lofty::picture::MimeType::Png)
    } else {
        None
    }
}

/// Read lyrics from the normalized tag, including the common APE
/// `UNSYNCEDLYRICS` field that lofty intentionally leaves as an unknown key.
fn lyrics_from_tag(tag: &Tag) -> Option<&str> {
    tag.get_string(&ItemKey::Lyrics).or_else(|| {
        tag.items().find_map(|item| {
            let ItemKey::Unknown(key) = item.key() else {
                return None;
            };
            let normalized: String = key
                .chars()
                .filter(char::is_ascii_alphanumeric)
                .flat_map(char::to_lowercase)
                .collect();
            matches!(normalized.as_str(), "lyrics" | "unsyncedlyrics")
                .then(|| item.value().text())
                .flatten()
        })
    })
}

/// Read the first cover represented by lofty's regular picture collection or
/// by an APE binary cover-art item. APE stores pictures as binary items whose
/// value includes a description, MIME signature and image bytes; lofty exposes
/// those items as `ItemKey::Unknown` rather than populating `Tag::pictures`.
fn read_cover(tag: &Tag, limit: usize, parsed: &mut ParsedMetadata) {
    if let Some(picture) = tag.pictures().first() {
        store_cover(picture.data(), picture.mime_type(), limit, parsed);
        return;
    }

    if tag.tag_type() != TagType::Ape {
        return;
    }

    for item in tag.items() {
        let ItemKey::Unknown(key) = item.key() else {
            continue;
        };
        if !lofty::ape::APE_PICTURE_TYPES
            .iter()
            .any(|known| known.eq_ignore_ascii_case(key))
        {
            continue;
        }
        let Some(bytes) = item.value().binary() else {
            continue;
        };
        let Ok(picture) = Picture::from_ape_bytes(key, bytes) else {
            parsed.warnings.push(ParseWarning {
                kind: ParseWarningKind::TagDecode,
                field: "cover".to_owned(),
            });
            continue;
        };
        store_cover(picture.data(), picture.mime_type(), limit, parsed);
        return;
    }
}

fn store_cover(
    data: &[u8],
    mime: Option<&lofty::picture::MimeType>,
    limit: usize,
    parsed: &mut ParsedMetadata,
) {
    if data.len() > limit {
        parsed.warnings.push(ParseWarning {
            kind: ParseWarningKind::CoverLimit,
            field: "cover".to_owned(),
        });
    } else if !data.is_empty() {
        parsed.cover = Some(EmbeddedCover {
            bytes: data.to_vec(),
            mime: mime_of(mime),
        });
    }
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
mod tests;
