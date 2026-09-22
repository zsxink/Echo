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

        // MP4/AAC tags parse through the same lofty path (the fixture carries
        // the standard title/artist/album generated by scripts/gen-fixtures.mjs).
        let m4a = read("tone-short.m4a");
        assert_eq!(m4a.title.as_deref(), Some("M4A Tone"));
        assert_eq!(m4a.artist.as_deref(), Some("Echo Fixtures"));
        assert_eq!(m4a.album.as_deref(), Some("Fixture Album"));
        assert!(
            m4a.parameters.sample_rate_hz == Some(44_100),
            "stream parameters come from the container: {:?}",
            m4a.parameters
        );
        // Duration/format are probe-owned, never tag-owned.
        assert!(m4a.duration.is_none());
        assert!(m4a.warnings.is_empty());
    }

    #[test]
    fn ape_unknown_items_supply_cover_and_unsynced_lyrics() {
        use lofty::tag::{ItemValue, TagItem};

        let mut tag = Tag::new(TagType::Ape);
        tag.insert_unchecked(TagItem::new(
            ItemKey::Unknown("COVER ART (FRONT)".to_owned()),
            ItemValue::Binary(
                [
                    b"Cover Art (Front).jpg\0".as_slice(),
                    &[0xff, 0xd8, 0xff, 0xe0, 0, 0x10, b'J', b'F', b'I', b'F'],
                ]
                .concat(),
            ),
        ));
        tag.insert_unchecked(TagItem::new(
            ItemKey::Unknown("UNSYNCEDLYRICS".to_owned()),
            ItemValue::Text("[00:01.00]first line".to_owned()),
        ));

        assert_eq!(
            lyrics_from_tag(&tag),
            Some("[00:01.00]first line"),
            "common APE lyrics key is not in lofty's normalized map"
        );

        let mut parsed = ParsedMetadata::default();
        read_cover(&tag, InputLimits::defaults().cover, &mut parsed);
        let cover = parsed.cover.expect("APE binary picture becomes a cover");
        assert_eq!(cover.mime, "image/jpeg");
        assert_eq!(&cover.bytes[..4], &[0xff, 0xd8, 0xff, 0xe0]);
        assert!(parsed.warnings.is_empty());
    }

    fn quicktime_atom_bytes(atom_type: [u8; 4], payload: &[u8]) -> Vec<u8> {
        let size = u32::try_from(payload.len() + 8).unwrap();
        [size.to_be_bytes().as_slice(), &atom_type, payload].concat()
    }

    fn quicktime_text_payload(value: &str) -> Vec<u8> {
        let mut payload = Vec::with_capacity(value.len() + 4);
        payload.extend_from_slice(&u16::try_from(value.len()).unwrap().to_be_bytes());
        payload.extend_from_slice(&[0x55, 0xc4]);
        payload.extend_from_slice(value.as_bytes());
        payload
    }

    fn quicktime_text_payload_with_trailing_language(value: &str) -> Vec<u8> {
        let mut payload = Vec::with_capacity(value.len() + 4);
        payload.extend_from_slice(&u16::try_from(value.len()).unwrap().to_be_bytes());
        payload.extend_from_slice(value.as_bytes());
        payload.extend_from_slice(&[0, 0]);
        payload
    }

    #[test]
    fn quicktime_legacy_udta_atoms_supply_text_tags() {
        let udta_payload = [
            quicktime_atom_bytes(*b"\xa9nam", &quicktime_text_payload("旧标题")),
            quicktime_atom_bytes(*b"\xa9ART", &quicktime_text_payload("旧歌手")),
            quicktime_atom_bytes(*b"\xa9alb", &quicktime_text_payload("旧专辑")),
        ]
        .concat();
        let content =
            quicktime_atom_bytes(*b"moov", &quicktime_atom_bytes(*b"udta", &udta_payload));
        let mut parsed = ParsedMetadata::default();

        read_quicktime_udta_tags(&content, &InputLimits::defaults(), &mut parsed);

        assert_eq!(parsed.title.as_deref(), Some("旧标题"));
        assert_eq!(parsed.artist.as_deref(), Some("旧歌手"));
        assert_eq!(parsed.album.as_deref(), Some("旧专辑"));
        assert!(parsed.warnings.is_empty());

        let trailing_language = quicktime_text_payload_with_trailing_language("兼容文本");
        assert_eq!(
            quicktime_text(&trailing_language, 0, trailing_language.len()),
            Some("兼容文本")
        );
    }

    #[test]
    fn quicktime_subtitle_payload_is_decoded() {
        let text = "[00:01.00]歌词";
        let mut payload = Vec::with_capacity(text.len() + 2);
        payload.extend_from_slice(&u16::try_from(text.len()).unwrap().to_be_bytes());
        payload.extend_from_slice(text.as_bytes());

        assert_eq!(quicktime_subtitle_text(&payload), Some(text));
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

    /// The MP4/AAC fixture agrees between the content-based and path-based
    /// reads, so an `.m4a` import source is named from its bytes exactly like
    /// the committed record it will become.
    #[test]
    fn m4a_tags_parse_from_in_memory_import_source_bytes() {
        let bytes = std::fs::read(fixtures_dir().join("tone-short.m4a")).unwrap();
        let reader = || LoftyMetadataReader::new(RootRegistry::new());
        let from_bytes = reader().read_bytes(&bytes).unwrap();
        assert_eq!(from_bytes.title.as_deref(), Some("M4A Tone"));
        assert_eq!(from_bytes.artist.as_deref(), Some("Echo Fixtures"));
        // The content-based read agrees with the path-based read of the same
        // audio (the import naming and the committed record must not drift).
        let from_file = read("tone-short.m4a");
        assert_eq!(from_bytes.title, from_file.title);
        assert_eq!(from_bytes.artist, from_file.artist);
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

    /// A real tag-less WAV must read as `Ok` with empty fields, never fail
    /// the pipeline: the missing-tag fallback only works when a no-tag file is
    /// a successful empty parse, not an error.
    #[test]
    fn tagless_wav_reads_as_ok_with_empty_fields() {
        let wav = read("tone-short.wav");
        assert_eq!(wav.title, None, "a real wav has no readable tags");
        assert_eq!(wav.artist, None);
        assert_eq!(wav.album, None);
        assert!(wav.duration.is_none(), "duration is probe-owned");
        assert!(wav.warnings.is_empty());
        assert_eq!(
            wav.parameters.sample_rate_hz,
            Some(44_100),
            "container stream parameters still parse: {:?}",
            wav.parameters
        );
    }

    fn riff_info_wav() -> Vec<u8> {
        fn chunk(kind: [u8; 4], value: &[u8]) -> Vec<u8> {
            let mut bytes = [
                kind.as_slice(),
                &u32::try_from(value.len() + 1).unwrap().to_le_bytes(),
                value,
                &[0],
            ]
            .concat();
            if value.len() % 2 == 0 {
                bytes.push(0);
            }
            bytes
        }

        let mut list_payload = b"INFO".to_vec();
        list_payload.extend(chunk(*b"INAM", "WAV 标题".as_bytes()));
        list_payload.extend(chunk(*b"IART", "WAV 歌手".as_bytes()));
        list_payload.extend(chunk(*b"IPRD", "WAV 专辑".as_bytes()));
        let list = [
            b"LIST".as_slice(),
            &u32::try_from(list_payload.len()).unwrap().to_le_bytes(),
            &list_payload,
        ]
        .concat();

        let fmt = [
            b"fmt ".as_slice(),
            &16u32.to_le_bytes(),
            &1u16.to_le_bytes(),
            &1u16.to_le_bytes(),
            &8_000u32.to_le_bytes(),
            &16_000u32.to_le_bytes(),
            &2u16.to_le_bytes(),
            &16u16.to_le_bytes(),
        ]
        .concat();
        let data = [b"data".as_slice(), &2u32.to_le_bytes(), &[0, 0]].concat();
        let body = [b"WAVE".as_slice(), &fmt, &list, &data].concat();
        [
            b"RIFF".as_slice(),
            &u32::try_from(body.len()).unwrap().to_le_bytes(),
            &body,
        ]
        .concat()
    }

    #[test]
    fn riff_info_wav_tags_are_read() {
        let parsed =
            read_from_reader(Cursor::new(riff_info_wav()), &InputLimits::defaults()).unwrap();

        assert_eq!(parsed.title.as_deref(), Some("WAV 标题"));
        assert_eq!(parsed.artist.as_deref(), Some("WAV 歌手"));
        assert_eq!(parsed.album.as_deref(), Some("WAV 专辑"));
        assert_eq!(parsed.parameters.sample_rate_hz, Some(8_000));
        assert!(parsed.warnings.is_empty());
    }
}
