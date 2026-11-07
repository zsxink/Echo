use std::io::Cursor;

use image::{ImageFormat, RgbImage};

use super::{resolve, resolve_legacy, save, validate};
use crate::application::ports::{ControlPlanePort, CoverCache};
use crate::application::testkit::small_fakes::{MemoryControlPlane, MemoryCoverCache};
use crate::domain::ids::LibraryRootId;
use crate::error::Error;

/// A tiny, real image in each whitelisted container. Generated in-process so
/// the tests need no fixture file or external encoder.
fn encode(format: ImageFormat) -> Vec<u8> {
    let image = RgbImage::from_pixel(4, 4, image::Rgb([200, 50, 50]));
    let mut out = Cursor::new(Vec::new());
    image.write_to(&mut out, format).expect("fixture encodes");
    out.into_inner()
}

/// CRC-32 of a PNG chunk; used to rewrite a real PNG's IHDR dimensions.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFF_u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xEDB8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// A legal IHDR declaring a huge image, with the original 4x4 IDAT left in
/// place: header-only validation must not need (or allocate) the pixels.
fn png_with_huge_dimensions() -> Vec<u8> {
    // 8-byte signature + 4-byte length + 4-byte "IHDR" = offset 16.
    const IHDR_DATA: usize = 16;
    const CRC_AT: usize = IHDR_DATA + 13;
    let mut png = encode(ImageFormat::Png);
    png[IHDR_DATA..IHDR_DATA + 4].copy_from_slice(&20_000_u32.to_be_bytes());
    png[IHDR_DATA + 4..IHDR_DATA + 8].copy_from_slice(&20_000_u32.to_be_bytes());
    let crc = crc32(&png[12..CRC_AT]);
    png[CRC_AT..CRC_AT + 4].copy_from_slice(&crc.to_be_bytes());
    png
}

fn validation_reason(result: Result<impl std::fmt::Debug, Error>) -> String {
    let error = result.expect_err("expected a validation error");
    assert_eq!(error.code(), "validation");
    match error {
        Error::Validation { reason, .. } => reason,
        other => panic!("unexpected error kind: {other:?}"),
    }
}

const TEXT: &[u8] = b"this is not an image at all, just plain bytes";
const ZIP: &[u8] = b"PK\x03\x04 a zip archive is not a cover";

#[test]
fn validate_rejects_non_image_bytes_declared_as_png() {
    for bytes in [TEXT, ZIP] {
        let reason = validation_reason(validate(bytes, "image/png"));
        assert!(
            reason.contains("not a recognized image"),
            "unexpected reason: {reason}"
        );
    }
}

#[test]
fn validate_rejects_declared_mime_that_differs_from_detected_format() {
    for (format, declared) in [
        (ImageFormat::Png, "image/jpeg"),
        (ImageFormat::Jpeg, "image/webp"),
        (ImageFormat::WebP, "image/png"),
    ] {
        let reason = validation_reason(validate(&encode(format), declared));
        assert!(
            reason.contains("does not match the declared"),
            "unexpected reason: {reason}"
        );
    }
    // A non-whitelisted container hidden behind a whitelisted declaration.
    let reason = validation_reason(validate(b"GIF89a fake gif payload", "image/png"));
    assert!(reason.contains("does not match the declared"));
}

#[test]
fn validate_rejects_truncated_or_malformed_container_header() {
    for bytes in [
        &b"\x89PNG\r\n\x1a\n"[..],
        &b"\xFF\xD8\xFF"[..],
        &b"RIFF\x00\x00\x00\x00WEBP"[..],
        &b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0dIHDRgarbage"[..],
    ] {
        let reason = validation_reason(validate(bytes, matching_mime(bytes)));
        assert!(reason.contains("header"), "unexpected reason: {reason}");
    }
}

fn matching_mime(bytes: &[u8]) -> &'static str {
    if bytes.starts_with(b"\x89PNG") {
        "image/png"
    } else if bytes.starts_with(b"\xFF\xD8\xFF") {
        "image/jpeg"
    } else {
        "image/webp"
    }
}

#[test]
fn validate_reads_only_the_container_header_without_pixel_decoding() {
    let huge = png_with_huge_dimensions();
    assert!(
        huge.len() < 200,
        "the fixture must stay a header-sized byte slice"
    );
    validate(&huge, "image/png").expect("a parseable huge IHDR is accepted");
}

#[test]
fn existing_size_and_mime_whitelist_rejections_are_unchanged() {
    let png = encode(ImageFormat::Png);
    let size_reason = validation_reason(validate(&[], "image/png"));
    assert!(size_reason.contains("between 1 byte and 5 MiB"));
    let oversized = vec![0_u8; 5 * 1024 * 1024 + 1];
    let oversized_reason = validation_reason(validate(&oversized, "image/png"));
    assert!(oversized_reason.contains("between 1 byte and 5 MiB"));
    let whitelist_reason = validation_reason(validate(&png, "image/gif"));
    assert!(whitelist_reason.contains("unsupported image type"));
}

#[test]
fn validate_accepts_jpeg_png_and_webp_with_matching_mime() {
    for (format, mime) in [
        (ImageFormat::Jpeg, "image/jpeg"),
        (ImageFormat::Png, "image/png"),
        (ImageFormat::WebP, "image/webp"),
    ] {
        validate(&encode(format), mime).expect("matching container is accepted");
    }
}

#[test]
fn save_and_resolve_round_trip_all_three_formats() {
    let control = MemoryControlPlane::new();
    let cache = MemoryCoverCache::new();
    let root = LibraryRootId::new();
    for (format, mime) in [
        (ImageFormat::Jpeg, "image/jpeg"),
        (ImageFormat::Png, "image/png"),
        (ImageFormat::WebP, "image/webp"),
    ] {
        let bytes = encode(format);
        let asset = save(&control, root, &bytes, mime).expect("original is saved");
        assert_eq!(asset.byte_len, bytes.len() as u64);
        assert_eq!(asset.mime, mime);
        assert_eq!(
            control
                .durability()
                .expect("durability port")
                .read_asset(root, &asset.content_hash)
                .expect("read asset")
                .as_deref(),
            Some(bytes.as_slice())
        );
        let key = resolve(&control, &cache, root, &asset).expect("original resolves");
        assert_eq!(
            cache.get(&key).expect("cache read").as_deref(),
            Some(bytes.as_slice())
        );
    }
    let _ = resolve_legacy(&cache, None).expect("no legacy key");
}

#[test]
fn every_whitelisted_mime_has_an_enabled_decoder() {
    for mime in ["image/jpeg", "image/png", "image/webp"] {
        assert!(
            ImageFormat::from_mime_type(mime).is_some_and(|format| format.reading_enabled()),
            "{mime} has no enabled decoder"
        );
    }
}

/// A focused test double for the two ordered cover-edit ports: counts the
/// commits that must NOT happen when validation rejects the payload.
#[derive(Default)]
struct StubCoverEdits {
    playlist_writes: std::sync::atomic::AtomicUsize,
    artist_writes: std::sync::atomic::AtomicUsize,
}

impl crate::application::ports::UnitOfWork for StubCoverEdits {
    fn with_tx(&self, _f: crate::application::ports::TxWork) -> Result<(), Error> {
        Ok(())
    }
    fn playlist_cover_edits(
        &self,
    ) -> Option<&dyn crate::application::ports::PlaylistCoverEditPort> {
        Some(self)
    }
    fn artist_cover_edits(&self) -> Option<&dyn crate::application::ports::ArtistCoverEditPort> {
        Some(self)
    }
}

impl crate::application::ports::PlaylistCoverEditPort for StubCoverEdits {
    fn set_library_playlist_cover(
        &self,
        _control: &dyn ControlPlanePort,
        _root: LibraryRootId,
        _id: crate::domain::ids::PlaylistId,
        _asset: Option<crate::domain::library::PortableAssetReference>,
        _device: crate::domain::library::DeviceId,
    ) -> Result<(), Error> {
        self.playlist_writes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

impl crate::application::ports::ArtistCoverEditPort for StubCoverEdits {
    fn set_library_artist_cover(
        &self,
        _control: &dyn ControlPlanePort,
        _root: LibraryRootId,
        _artist_key: &str,
        _asset: Option<crate::domain::library::PortableAssetReference>,
        _device: crate::domain::library::DeviceId,
    ) -> Result<(), Error> {
        self.artist_writes
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}

#[test]
fn playlist_and_artist_cover_writes_share_the_same_validation() {
    use crate::application::artist_cover::set_artist_cover;
    use crate::application::playlist::durable::DurableLibraryEdits;
    use crate::application::ports::PlaylistRepository;
    use crate::application::testkit::repositories::{
        MemoryPlaylistRepository, MemorySongRepository,
    };
    use crate::domain::ids::PlaylistId;
    use crate::domain::library::DeviceId;

    let root = LibraryRootId::new();
    let control = MemoryControlPlane::new();
    let playlists = MemoryPlaylistRepository::new();
    let songs = MemorySongRepository::new();
    let unit = StubCoverEdits::default();
    let device = DeviceId::new();
    let id = PlaylistId::new();
    playlists.create(id, root, "cover").expect("seed playlist");
    let edits = DurableLibraryEdits {
        control: &control,
        songs: &songs,
        playlists: &playlists,
        unit: &unit,
        device,
    };
    let playlist_error = edits
        .set_playlist_cover(root, id, Some(TEXT), Some("image/png"))
        .expect_err("fake playlist cover must be rejected");
    assert_eq!(playlist_error.code(), "validation");
    let artist_error = set_artist_cover(
        &control,
        &unit,
        root,
        "artist",
        Some(TEXT),
        Some("image/png"),
        device,
    )
    .expect_err("fake artist cover must be rejected");
    assert_eq!(artist_error.code(), "validation");
    assert_eq!(
        unit.playlist_writes
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    assert_eq!(
        unit.artist_writes.load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    let hash = blake3::hash(TEXT).to_hex().to_string();
    assert_eq!(
        control
            .durability()
            .expect("durability port")
            .read_asset(root, &hash)
            .expect("read asset"),
        None,
        "no durable asset may be written before validation passes"
    );
}

/// Records whether `resolve` ever hands rejected bytes to the cache.
#[derive(Default)]
struct CountingCoverCache {
    puts: std::sync::atomic::AtomicUsize,
}

impl CoverCache for CountingCoverCache {
    fn put(&self, _bytes: &[u8], _mime: &str) -> Result<String, Error> {
        self.puts.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok("cv1-unused".to_owned())
    }
    fn get(&self, _asset_key: &str) -> Result<Option<Vec<u8>>, Error> {
        Ok(Some(Vec::new()))
    }
    fn thumbnail(&self, _asset_key: &str, _detail: bool) -> Result<Option<Vec<u8>>, Error> {
        Ok(None)
    }
    fn gc(&self, _referenced_keys: &[String]) -> Result<(), Error> {
        Ok(())
    }
}

#[test]
fn resolving_an_existing_invalid_original_reports_a_validation_error() {
    use crate::domain::library::PortableAssetReference;

    let control = MemoryControlPlane::new();
    let cache = CountingCoverCache::default();
    let root = LibraryRootId::new();
    let fake = b"legacy invalid original bytes";
    let hash = control
        .durability()
        .expect("durability port")
        .write_asset(root, fake)
        .expect("seed a historical invalid original");
    let asset = PortableAssetReference {
        content_hash: hash.clone(),
        byte_len: fake.len() as u64,
        mime: "image/png".to_owned(),
    };
    let error = resolve(&control, &cache, root, &asset)
        .expect_err("invalid original must not become a cover");
    assert_eq!(error.code(), "validation");
    assert_eq!(
        cache.puts.load(std::sync::atomic::Ordering::SeqCst),
        0,
        "the disposable cache must not receive undecodable bytes"
    );
    assert_eq!(
        control
            .durability()
            .expect("durability port")
            .read_asset(root, &hash)
            .expect("read asset")
            .as_deref(),
        Some(fake.as_slice()),
        "the historical original and its reference stay untouched"
    );
}

#[test]
fn legacy_cache_only_cover_is_unaffected() {
    let cache = MemoryCoverCache::new();
    let key = cache
        .put(b"legacy bytes", "image/png")
        .expect("seed cache-only cover");
    assert_eq!(
        resolve_legacy(&cache, Some(&key))
            .expect("legacy key resolves")
            .as_deref(),
        Some(key.as_str())
    );
    assert_eq!(resolve_legacy(&cache, None).expect("no legacy key"), None);
}
