//! Bounded, in-memory cover-pick previews at the Tauri command boundary.
//!
//! The edit-playlist dialog must show the image the user just picked, but a
//! `WebView` that decodes the *original* image pays an RGBA allocation
//! proportional to pixel count (issue #133). This module turns the picked
//! bytes into a `detail`-tier JPEG thumbnail inside the desktop shell and
//! returns only that thumbnail's base64 text, which the `WebView` loads from an
//! already-CSP-allowed `data:` URL.
//!
//! Nothing here touches the cover cache, the library directory, or `cover://`:
//! the preview is a one-shot, in-memory response to an untrusted payload. The
//! write path (`cover_payload` → `AppServices` → Core) is unchanged, and Core's
//! header-only `artwork::validate` (LD-12) is deliberately not reused: a
//! preview is the one place where a full decode is intended, so it owns its
//! own allocation ceiling instead of weakening the write-side guard.

use std::io::Cursor;

use base64::Engine;
use echo_desktop::ipc::IpcErrorDto;
use image::imageops::FilterType;
use image::{DynamicImage, ImageFormat, ImageReader, Limits};

use crate::cover_payload::{decode_required_cover_bytes, validation, MAX_COVER_BYTES};

/// Same display tier as `cover://…?s=detail` (`DETAIL_THUMB_PX` in Core).
/// Kept here rather than imported so the shell's preview and the protocol's
/// tier stay two independent constants with the same value; the drift test
/// below compares their actual outputs.
const DETAIL_THUMB_PX: u32 = 512;

/// Header-only pixel budget. 4000×4000 (16 MP) still previews; a
/// 20000×20000 IHDR (400 MP) is refused before any pixel buffer exists.
const MAX_PREVIEW_PIXELS: u64 = 20_000_000;

/// Decoder allocation ceiling. `ImageReader::decode` calls
/// `Limits::reserve(decoder.total_bytes())` before allocating pixels, so this
/// is a hard pre-decode rejection for decoders that honour the limit.
///
/// Budget (design D4): `max_alloc` covers the decoder's own RGBA output.
/// `DynamicImage::resize`'s Lanczos3 intermediates and `to_rgb8`'s RGB buffer
/// are *additional* allocations, so the header precheck above caps the input
/// at 20 MP: 20 MP × (4 B RGBA + ~8 B intermediates + 3 B RGB) is the shell's
/// worst case, never the `WebView` renderer's. The write path never decodes.
const MAX_DECODE_ALLOC: u64 = 96 * 1024 * 1024;

/// Produce a bounded JPEG preview for the picked cover.
///
/// The command is async solely so the CPU/memory-bound decode runs on a
/// controlled blocking task rather than on the async runtime thread
/// (`CODE_STANDARDS §4.3`). Both arguments are required `String`s: a missing
/// `bytes` key must fail argument deserialization, never become "clear the
/// cover" as it would with `Option<String>` (design D3).
///
/// # Errors
/// Returns the same validation `IpcErrorDto` shape as the cover write path.
#[tauri::command]
pub async fn preview_cover(bytes: String, mime: String) -> Result<String, IpcErrorDto> {
    tokio::task::spawn_blocking(move || generate_preview(&bytes, &mime))
        .await
        .map_err(|error| validation(&format!("封面预览任务失败：{error}")))?
}

fn generate_preview(payload: &str, mime: &str) -> Result<String, IpcErrorDto> {
    let bytes = decode_required_cover_bytes(payload)?;
    if bytes.is_empty() || bytes.len() > MAX_COVER_BYTES {
        return Err(validation("封面图片体积不符合要求"));
    }
    let declared = declared_format(mime)?;
    let detected = detect_format(&bytes)?;
    if detected != declared || !detected.reading_enabled() {
        return Err(validation("封面图片格式与声明类型不一致"));
    }
    let (width, height) = probe_dimensions(&bytes, detected)?;
    let jpeg = thumbnail_jpeg(&bytes, detected, width, height)?;
    Ok(base64::engine::general_purpose::STANDARD.encode(jpeg))
}

/// Reuse the write path's whitelist; `ImageFormat::from_mime_type` alone would
/// also accept GIF/BMP/TIFF, which are valid embedded-cover formats but not
/// valid *picked* cover formats (`DAS-R22`/`LD-12`).
fn declared_format(mime: &str) -> Result<ImageFormat, IpcErrorDto> {
    if !matches!(mime, "image/jpeg" | "image/png" | "image/webp") {
        return Err(validation("封面图片类型不受支持"));
    }
    ImageFormat::from_mime_type(mime).ok_or_else(|| validation("封面图片类型不受支持"))
}

fn detect_format(bytes: &[u8]) -> Result<ImageFormat, IpcErrorDto> {
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|_| validation("封面图片无法识别"))?;
    reader
        .format()
        .ok_or_else(|| validation("封面图片无法识别"))
}

/// Read container dimensions *before* decoding pixels, and enforce the pixel
/// budget from those declared dimensions. `into_dimensions()` builds a decoder
/// and reads the header only — it does not allocate an RGBA pixel buffer.
fn probe_dimensions(bytes: &[u8], format: ImageFormat) -> Result<(u32, u32), IpcErrorDto> {
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    reader.limits(preview_limits());
    let (width, height) = reader
        .into_dimensions()
        .map_err(|_| validation("封面图片头无效或已截断"))?;
    if width == 0 || height == 0 {
        return Err(validation("封面图片头无效或已截断"));
    }
    if u64::from(width) * u64::from(height) > MAX_PREVIEW_PIXELS {
        return Err(validation("封面图片尺寸过大，无法生成预览"));
    }
    Ok((width, height))
}

fn preview_limits() -> Limits {
    // `Limits` is #[non_exhaustive]; start from the defaults (512 MiB) and
    // lower the allocation ceiling instead of disabling limits entirely.
    let mut limits = Limits::default();
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    limits
}

fn thumbnail_jpeg(
    bytes: &[u8],
    format: ImageFormat,
    width: u32,
    height: u32,
) -> Result<Vec<u8>, IpcErrorDto> {
    let mut reader = ImageReader::with_format(Cursor::new(bytes), format);
    reader.limits(preview_limits());
    let image = reader.decode().map_err(|error| {
        if matches!(error, image::ImageError::Limits(_)) {
            validation("封面图片尺寸过大，无法生成预览")
        } else {
            validation("封面图片无法解码")
        }
    })?;
    // Never upscale: a 64×32 cover stays 64×32.
    let scaled = if width > DETAIL_THUMB_PX || height > DETAIL_THUMB_PX {
        image.resize(DETAIL_THUMB_PX, DETAIL_THUMB_PX, FilterType::Lanczos3)
    } else {
        image
    };
    // JPEG has no alpha; the existing `cover://` thumbnails drop it too.
    let rgb = DynamicImage::ImageRgb8(scaled.to_rgb8());
    let mut out = Cursor::new(Vec::new());
    rgb.write_to(&mut out, ImageFormat::Jpeg)
        .map_err(|_| validation("封面预览编码失败"))?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgb, RgbImage};

    fn encode(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    fn png(width: u32, height: u32) -> Vec<u8> {
        let image =
            DynamicImage::ImageRgb8(RgbImage::from_pixel(width, height, Rgb([24, 68, 120])));
        let mut out = Cursor::new(Vec::new());
        image
            .write_to(&mut out, ImageFormat::Png)
            .expect("fixture encodes");
        out.into_inner()
    }

    fn decode_preview(result: &str) -> Vec<u8> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(result)
            .expect("base64 preview payload");
        assert_eq!(&bytes[..3], &[0xFF, 0xD8, 0xFF], "preview must be JPEG");
        bytes
    }

    fn dimensions(jpeg: &[u8]) -> (u32, u32) {
        let image = image::load_from_memory(jpeg).expect("preview decodes");
        (image.width(), image.height())
    }

    /// CRC-32 of a PNG chunk, used to rewrite IHDR dimensions while leaving
    /// the tiny IDAT untouched (same technique as Core's header-only tests).
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

    fn png_with_huge_header() -> Vec<u8> {
        const IHDR_DATA: usize = 16;
        const CRC_AT: usize = IHDR_DATA + 13;
        let mut bytes = png(2, 2);
        bytes[IHDR_DATA..IHDR_DATA + 4].copy_from_slice(&20_000_u32.to_be_bytes());
        bytes[IHDR_DATA + 4..IHDR_DATA + 8].copy_from_slice(&20_000_u32.to_be_bytes());
        let crc = crc32(&bytes[12..CRC_AT]);
        bytes[CRC_AT..CRC_AT + 4].copy_from_slice(&crc.to_be_bytes());
        bytes
    }

    fn assert_validation(error: &IpcErrorDto) {
        assert_eq!(error.code, "validation");
        assert_eq!(error.message_key, "error.validation");
        assert_eq!(error.field.as_deref(), Some("cover"));
    }

    #[test]
    fn preview_thumbnail_caps_the_longest_edge_at_detail_tier() {
        let large = png(1200, 800);
        let result = generate_preview(&encode(&large), "image/png").expect("large preview");
        let (width, height) = dimensions(&decode_preview(&result));
        assert_eq!((width, height), (DETAIL_THUMB_PX, 341));

        let small = png(64, 32);
        let result = generate_preview(&encode(&small), "image/png").expect("small preview");
        assert_eq!(dimensions(&decode_preview(&result)), (64, 32));
    }

    #[test]
    fn preview_thumbnail_rejects_oversized_pixel_allocation_before_decoding() {
        let huge = png_with_huge_header();
        assert_eq!(
            probe_dimensions(&huge, ImageFormat::Png)
                .expect_err("huge IHDR")
                .code,
            "validation"
        );
        assert_validation(&generate_preview(&encode(&huge), "image/png").expect_err("huge IHDR"));
    }

    #[test]
    fn preview_thumbnail_rejects_mime_and_format_mismatch() {
        let bytes = png(32, 32);
        assert_validation(
            &generate_preview(&encode(&bytes), "image/jpeg").expect_err("declared JPEG"),
        );
        assert_validation(
            &generate_preview(&encode(&bytes), "image/gif").expect_err("unsupported MIME"),
        );
    }

    #[test]
    fn preview_thumbnail_rejects_malformed_or_empty_payload() {
        assert_validation(&generate_preview("", "image/png").expect_err("empty payload"));
        assert_validation(&generate_preview("not*base64!", "image/png").expect_err("bad base64"));
        assert_validation(
            &generate_preview(&encode(b"definitely not an image"), "image/png")
                .expect_err("non-image bytes"),
        );
    }

    #[test]
    fn preview_thumbnail_returns_jpeg_bytes_without_touching_disk() {
        // The generator has no path parameter and no `std::fs` reference (also
        // asserted by the change's rg gate). Watch a fresh temp directory so a
        // future accidental write is caught here as well.
        let dir = tempfile::tempdir().expect("temp dir");
        let before: Vec<_> = std::fs::read_dir(dir.path())
            .expect("read dir")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        let bytes = png(700, 500);
        let result = generate_preview(&encode(&bytes), "image/png").expect("preview");
        let jpeg = decode_preview(&result);
        assert!(
            jpeg.len() < bytes.len(),
            "thumbnail must be smaller than original"
        );
        let after: Vec<_> = std::fs::read_dir(dir.path())
            .expect("read dir")
            .map(|entry| entry.expect("entry").file_name())
            .collect();
        assert_eq!(before, after, "preview generation must not write files");
    }

    /// Manual evidence for design D4/D8: a 4000×4000 input (16 MP, inside
    /// the 20 MP header budget) is accepted and yields a 512 px preview.
    /// Running this single ignored test under `/usr/bin/time -l` gives the
    /// shell's real peak RSS for the quantization record.
    #[test]
    #[ignore = "manual peak-memory measurement (design D8)"]
    fn measures_a_16mp_preview_within_the_budget() {
        let bytes = png(4000, 4000);
        let result = generate_preview(&encode(&bytes), "image/png").expect("16 MP preview");
        let jpeg = decode_preview(&result);
        assert_eq!(dimensions(&jpeg), (DETAIL_THUMB_PX, DETAIL_THUMB_PX));
        assert!(jpeg.len() < 1024 * 1024, "thumbnail stays small");
    }

    #[test]
    fn preview_thumbnail_matches_cover_detail_tier_dimensions() {
        // Drift guard for design D2: the shell's preview and Core's `scale_to`
        // are separate implementations, so compare against the same four-step
        // pipeline applied to the same fixture.
        let bytes = png(1000, 640);
        let expected = image::load_from_memory(&bytes)
            .expect("fixture decodes")
            .resize(DETAIL_THUMB_PX, DETAIL_THUMB_PX, FilterType::Lanczos3);
        let result = generate_preview(&encode(&bytes), "image/png").expect("preview");
        let jpeg = decode_preview(&result);
        assert_eq!(dimensions(&jpeg), (expected.width(), expected.height()));
        assert!(dimensions(&jpeg).0 <= DETAIL_THUMB_PX);
    }
}
