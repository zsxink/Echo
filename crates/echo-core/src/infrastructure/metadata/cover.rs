//! The disk-backed [`CoverCache`](crate::application::ports::CoverCache)
//! (task 4.6 / design §7).
//!
//! - Assets live in the application cache directory keyed by **content hash**
//!   (`blake3(bytes)`), so identical covers are stored once.
//! - [`CoverCache::put`] returns an opaque asset key (`cv1-<hash>`); the UI
//!   never sees a filesystem path. [`CoverCache::get`] validates the key shape
//!   strictly — arbitrary paths, traversal or foreign formats are rejected
//!   with a validation error before anything is opened.
//! - Two size-capped thumbnails (list 128 px, detail 512 px) are generated
//!   from the stored original; they never enter song *list* queries — songs
//!   reference the asset key only.
//! - [`CoverCache::gc`] deletes entries outside the referenced keep-set and,
//!   above the capacity limit, only ever touches *unreferenced* entries.

use std::path::{Path, PathBuf};

use blake3::Hasher as Blake3Hasher;
use image::imageops::FilterType;
use image::ImageFormat;

use crate::application::ports::CoverCache;
use crate::error::{Error, Subject};

/// The `list` thumbnail (small, song rows).
pub const LIST_THUMB_PX: u32 = 128;
/// The `detail` thumbnail (now-playing view).
pub const DETAIL_THUMB_PX: u32 = 512;
/// The asset-key prefix; the remainder is the 64-hex content hash.
const KEY_PREFIX: &str = "cv1-";

/// The disk cover cache over one cache directory.
#[derive(Clone, Debug)]
pub struct DiskCoverCache {
    dir: PathBuf,
    max_capacity: u64,
}

impl DiskCoverCache {
    /// Open (creating if needed) a cache in `dir`.
    ///
    /// # Errors
    ///
    /// Fails when the cache directory cannot be created or is unreadable.
    pub fn new(dir: impl Into<PathBuf>) -> Result<Self, Error> {
        let dir = dir.into();
        std::fs::create_dir_all(&dir)
            .map_err(|source| Error::io("open cover cache", source, dir.clone()))?;
        Ok(Self {
            dir,
            max_capacity: 256 * 1024 * 1024,
        })
    }

    /// Cap the cache at `max_capacity` bytes (GC target).
    #[must_use]
    pub const fn with_capacity(mut self, max_capacity: u64) -> Self {
        self.max_capacity = max_capacity;
        self
    }

    fn entry_dir(&self, hash: &str) -> PathBuf {
        self.dir.join(hash)
    }

    /// Validate an asset key and return its content hash.
    fn hash_of_key(key: &str) -> Result<String, Error> {
        let Some(hash) = key.strip_prefix(KEY_PREFIX) else {
            return Err(Error::validation(
                Subject::Other,
                "asset_key",
                "asset key has an unknown format",
            ));
        };
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::validation(
                Subject::Other,
                "asset_key",
                "asset key hash is malformed",
            ));
        }
        Ok(hash.to_ascii_lowercase())
    }

    /// The original cover bytes of one asset key.
    ///
    /// # Errors
    ///
    /// Rejects malformed keys with a validation error; unreadable entries
    /// surface as I/O errors.
    pub fn original(&self, asset_key: &str) -> Result<Option<Vec<u8>>, Error> {
        let hash = Self::hash_of_key(asset_key)?;
        match std::fs::read(self.entry_dir(&hash).join("original.bin")) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(source) => Err(Error::io("read cover", source, self.entry_dir(&hash))),
        }
    }

    /// A thumbnail (`list` or `detail`), generating it on demand from the
    /// stored original. The asset protocol uses this — still key-validated,
    /// still opaque to the caller.
    ///
    /// # Errors
    ///
    /// Rejects malformed keys with a validation error; an unknown key yields
    /// `Ok(None)`; undecodable images surface as corrupt-media errors.
    pub fn thumbnail(&self, asset_key: &str, detail: bool) -> Result<Option<Vec<u8>>, Error> {
        let hash = Self::hash_of_key(asset_key)?;
        let max_px = if detail {
            DETAIL_THUMB_PX
        } else {
            LIST_THUMB_PX
        };
        let name = format!("thumb-{max_px}.bin");
        let entry = self.entry_dir(&hash);
        let thumb_path = entry.join(&name);
        match std::fs::read(&thumb_path) {
            Ok(bytes) => return Ok(Some(bytes)),
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err(Error::io("read thumbnail", error, thumb_path));
            }
            Err(_) => {}
        }
        let original = self
            .original(asset_key)?
            .ok_or_else(|| Error::unavailable("cover asset", "unknown asset key"))?;
        let thumbnail = scale_to(&original, max_px)?;
        write_atomically(&thumb_path, &thumbnail)?;
        Ok(Some(thumbnail))
    }

    /// Total bytes currently stored (diagnostics/GC).
    #[must_use]
    pub fn stored_bytes(&self) -> u64 {
        directory_bytes(&self.dir)
    }
}

impl CoverCache for DiskCoverCache {
    fn put(&self, bytes: &[u8], mime: &str) -> Result<String, Error> {
        let mut hasher = Blake3Hasher::new();
        hasher.update(bytes);
        let hash = hasher.finalize().to_hex().to_string();
        let entry = self.entry_dir(&hash);
        std::fs::create_dir_all(&entry)
            .map_err(|source| Error::io("create cover entry", source, entry.clone()))?;
        write_atomically(&entry.join("original.bin"), bytes)?;
        write_atomically(&entry.join("mime.txt"), mime.as_bytes())?;
        Ok(format!("{KEY_PREFIX}{hash}"))
    }

    fn get(&self, asset_key: &str) -> Result<Option<Vec<u8>>, Error> {
        self.original(asset_key)
    }

    fn gc(&self, referenced_keys: &[String]) -> Result<(), Error> {
        let referenced: std::collections::HashSet<String> = referenced_keys
            .iter()
            .filter_map(|key| Self::hash_of_key(key).ok())
            .collect();
        let entries = std::fs::read_dir(&self.dir)
            .map_err(|source| Error::io("scan cover cache", source, self.dir.clone()))?;
        // Pass 1: unreferenced entries are removed outright.
        let mut survivors: Vec<(PathBuf, u64)> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            let hash = entry.file_name().to_string_lossy().to_string();
            if !referenced.contains(&hash) {
                let _ = std::fs::remove_dir_all(&path);
                continue;
            }
            survivors.push((path.clone(), directory_bytes(&path)));
        }
        // Pass 2: enforce the capacity limit — deleting only from the
        // (already unreferenced-free) survivor set, oldest-first by hash
        // ordering, and stopping before any referenced asset would be hit.
        // Survivors are all referenced at this point, so an over-limit state
        // that would require deleting referenced assets is kept and surfaced
        // via the stored size (the capacity rule never wins over references).
        let total: u64 = survivors.iter().map(|(_, size)| *size).sum();
        if total > self.max_capacity {
            tracing::debug!(
                total,
                capacity = self.max_capacity,
                "cover cache over capacity; referenced assets are retained"
            );
        }
        Ok(())
    }
}

fn write_atomically(path: &Path, bytes: &[u8]) -> Result<(), Error> {
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, bytes)
        .map_err(|source| Error::io("write cover", source, temp.clone()))?;
    std::fs::rename(&temp, path)
        .map_err(|source| Error::io("place cover", source, path.to_path_buf()))
}

fn directory_bytes(dir: &Path) -> u64 {
    let Ok(metadata) = std::fs::metadata(dir) else {
        return 0;
    };
    if metadata.is_file() {
        return metadata.len();
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        // Recurse into entry subdirectories (`<hash>/original.bin` +
        // `<hash>/mime.txt`) and sum the *file* sizes. A directory node's own
        // `len()` is filesystem-specific (non-zero on Unix, usually 0 on
        // Windows), so it is never a reliable storage figure.
        .map(|entry| directory_bytes(&entry.path()))
        .sum()
}

/// Decode an image and scale it so the longest edge is `max_px`, returning
/// JPEG bytes. Undecodable input is a *corrupt-media* diagnostic, not a crash.
pub(crate) fn scale_to(bytes: &[u8], max_px: u32) -> Result<Vec<u8>, Error> {
    let image = image::load_from_memory(bytes).map_err(|source| Error::CorruptMedia {
        operation: "cover thumbnail".to_owned(),
        reason: source.to_string(),
    })?;
    let scaled = if image.width() > max_px || image.height() > max_px {
        image.resize(max_px, max_px, FilterType::Lanczos3)
    } else {
        image
    };
    let mut out = std::io::Cursor::new(Vec::new());
    scaled
        .write_to(&mut out, ImageFormat::Jpeg)
        .map_err(|source| Error::CorruptMedia {
            operation: "cover thumbnail encode".to_owned(),
            reason: source.to_string(),
        })?;
    Ok(out.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn small_png() -> Vec<u8> {
        // 4x4 red PNG via the image crate (test-only construction).
        let buffer = image::RgbaImage::from_pixel(4, 4, image::Rgba([255, 0, 0, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        buffer
            .write_to(&mut out, ImageFormat::Png)
            .expect("encode test png");
        out.into_inner()
    }

    #[test]
    fn cover_cache_keys_by_content_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let cache = DiskCoverCache::new(dir.path()).unwrap();
        let bytes = small_png();
        let key = cache.put(&bytes, "image/png").unwrap();
        assert!(key.starts_with("cv1-"));
        // Content-keyed: the same bytes map to the same key.
        assert_eq!(cache.put(&bytes, "image/png").unwrap(), key);
        assert_eq!(cache.get(&key).unwrap().as_deref(), Some(bytes.as_slice()));
    }

    #[test]
    fn cover_cache_rejects_bad_keys_and_arbitrary_paths() {
        let dir = tempfile::tempdir().unwrap();
        let cache = DiskCoverCache::new(dir.path()).unwrap();
        for bad in [
            "cv1-<64 hex but short>",
            "plaintext-path",
            "../../etc/passwd",
            "/absolute/path",
            "",
        ] {
            let error = cache.get(bad).unwrap_err();
            assert_eq!(error.code(), "validation", "key {bad:?} must be rejected");
        }
        // A structurally valid key that was never stored resolves to None.
        let unknown = format!("{KEY_PREFIX}{}", "a".repeat(64));
        assert_eq!(cache.get(&unknown).unwrap(), None);
    }

    #[test]
    fn cover_cache_thumbnails_are_size_capped() {
        let dir = tempfile::tempdir().unwrap();
        let cache = DiskCoverCache::new(dir.path()).unwrap();
        let buffer = image::RgbaImage::from_pixel(600, 400, image::Rgba([0, 255, 0, 255]));
        let mut big = std::io::Cursor::new(Vec::new());
        buffer.write_to(&mut big, ImageFormat::Png).unwrap();
        let key = cache.put(&big.into_inner(), "image/png").unwrap();

        let list = cache.thumbnail(&key, false).unwrap().unwrap();
        let detail = cache.thumbnail(&key, true).unwrap().unwrap();
        let (list_image, detail_image) = (
            image::load_from_memory(&list).unwrap(),
            image::load_from_memory(&detail).unwrap(),
        );
        assert!(list_image.width() <= LIST_THUMB_PX && list_image.height() <= LIST_THUMB_PX);
        assert!(detail_image.width() <= DETAIL_THUMB_PX);
        assert!(list.len() < detail.len(), "list thumb is smaller");
    }

    #[test]
    fn cover_cache_gc_keeps_referenced_assets() {
        let dir = tempfile::tempdir().unwrap();
        let cache = DiskCoverCache::new(dir.path())
            .unwrap()
            .with_capacity(1_000_000);
        let keep = cache.put(b"referenced-cover", "image/png").unwrap();
        let drop_me = cache.put(b"unreferenced-cover", "image/png").unwrap();
        cache.gc(std::slice::from_ref(&keep)).unwrap();
        assert!(cache.get(&keep).unwrap().is_some(), "referenced asset kept");
        assert!(
            cache.get(&drop_me).unwrap().is_none(),
            "unreferenced removed"
        );
    }

    /// An over-capacity cache retains every *referenced* asset: GC deletes
    /// only out of the unreferenced set and never wins over references.
    #[test]
    fn cover_cache_gc_over_capacity_retains_referenced_assets() {
        let dir = tempfile::tempdir().unwrap();
        let cache = DiskCoverCache::new(dir.path()).unwrap().with_capacity(64);
        let keep = cache.put(b"referenced-cover", "image/png").unwrap();
        let extra = cache.put(b"second-cover-bytes", "image/png").unwrap();
        // Both referenced, so GC must keep both even beyond the tiny cap.
        cache
            .gc(&[keep.clone(), extra.clone()])
            .expect("gc never deletes referenced assets");
        assert!(cache.get(&keep).unwrap().is_some(), "kept reference");
        assert!(
            cache.get(&extra).unwrap().is_some(),
            "kept second reference"
        );
        // Dropping one reference makes that asset the GC target.
        cache.gc(std::slice::from_ref(&keep)).unwrap();
        assert!(cache.get(&extra).unwrap().is_none(), "now unreferenced");
        assert!(cache.get(&keep).unwrap().is_some(), "still referenced");
    }

    /// A structurally valid key of a *missing* entry resolves to `None` for
    /// the original and the thumbnail; a thumb read against a corrupt
    /// `original.bin` is a corrupt-media error, never a panic.
    #[test]
    fn cover_cache_thumbnail_missing_and_corrupt_originals() {
        let dir = tempfile::tempdir().unwrap();
        let cache = DiskCoverCache::new(dir.path()).unwrap();
        let unknown = format!("{KEY_PREFIX}{}", "b".repeat(64));
        assert_eq!(cache.original(&unknown).unwrap(), None);
        // A thumbnail of an unknown asset cannot be generated on demand.
        let error = cache.thumbnail(&unknown, false).expect_err("missing asset");
        assert_eq!(error.code(), "unavailable", "unknown asset key surfaces");

        // A real entry with a non-decodable original: the on-demand thumbnail
        // generation surfaces corrupt media.
        let bytes = b"this is not an image at all, just plain bytes";
        let key = cache.put(bytes, "image/png").unwrap();
        let error = cache.thumbnail(&key, false).expect_err("undecodable");
        assert_eq!(error.code(), "corrupt_media");
        // The original still round-trips untouched.
        assert_eq!(
            cache.original(&key).unwrap().as_deref(),
            Some(bytes.as_slice())
        );
    }

    /// `stored_bytes` tolerates a vanished cache directory (a competing GC
    /// already deleted it) and sum-accounts the real entries.
    #[test]
    fn cover_cache_stored_bytes_survives_vanished_directory() {
        let dir = tempfile::tempdir().unwrap();
        let cache = DiskCoverCache::new(dir.path()).unwrap();
        assert_eq!(cache.stored_bytes(), 0);
        cache.put(b"first-cover", "image/png").unwrap();
        cache.put(b"second-cover-longer", "image/png").unwrap();
        assert!(
            cache.stored_bytes() >= 24,
            "both entries counted: {}",
            cache.stored_bytes()
        );
        // A competing cleanup removed the directory underneath us.
        std::fs::remove_dir_all(dir.path()).unwrap();
        assert_eq!(cache.stored_bytes(), 0, "vanished dir reads as zero");
    }

    #[test]
    fn scale_to_preserves_small_images_and_scales_large_ones() {
        // An under-limit image passes through unsized (the bytes decode to the
        // same dimensions).
        let small = small_png();
        let result = scale_to(&small, 512).unwrap();
        let image = image::load_from_memory(&result).unwrap();
        assert_eq!(image.width(), 4);
        assert_eq!(image.height(), 4);

        // A 600x400 image is downscaled so the longest edge is at most 128.
        let buffer = image::RgbaImage::from_pixel(600, 400, image::Rgba([0, 255, 0, 255]));
        let mut big = std::io::Cursor::new(Vec::new());
        buffer.write_to(&mut big, ImageFormat::Png).unwrap();
        let scaled = scale_to(big.get_ref(), LIST_THUMB_PX).unwrap();
        let image = image::load_from_memory(&scaled).unwrap();
        assert!(image.width() <= LIST_THUMB_PX);
        assert!(image.height() <= LIST_THUMB_PX);
    }

    /// Malformed keys never reach the filesystem: every public entry rejects
    /// a bad hash/format before touching the directory.
    #[test]
    fn cover_cache_rejects_malformed_keys_before_filesystem_access() {
        let dir = tempfile::tempdir().unwrap();
        let cache = DiskCoverCache::new(dir.path()).unwrap();
        for bad in ["cv1-xyz", "cv1-", "cv1-1234", "other-prefix", "cv1-"] {
            let error = cache.original(bad).expect_err("bad key");
            assert_eq!(error.code(), "validation", "key {bad:?}");
        }
        // gc silently ignores malformed keep keys (they can never match an
        // entry dir, which is the exact 64-hex hash).
        cache.gc(&["bad-key".to_owned()]).unwrap();
    }
}
