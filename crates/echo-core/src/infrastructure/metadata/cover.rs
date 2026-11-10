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
//!   above the capacity limit, evicts rebuildable thumbnails from referenced
//!   entries. The original and its mime record are never reclaimed: capacity
//!   never wins over a live reference.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

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

/// One generation gate per `(content hash, tier)`. The map lock is held only
/// to obtain the gate; the gate itself serializes generation.
type InflightGates = Arc<Mutex<HashMap<(String, bool), Arc<Mutex<()>>>>>;

/// Persisted names of the two rebuildable tiers. GC only ever reclaims these
/// files; a new tier must be registered here and in the candidate collector.
const LIST_THUMB_FILE: &str = "thumb-128.bin";
const DETAIL_THUMB_FILE: &str = "thumb-512.bin";

/// One rebuildable thumbnail that capacity reclamation may delete.
struct ReclaimCandidate {
    path: PathBuf,
    len: u64,
    /// `0` = detail tier (512 px), `1` = list tier (128 px).
    tier: u8,
    hash: String,
}

/// The disk cover cache over one cache directory.
#[derive(Clone, Debug)]
pub struct DiskCoverCache {
    dir: PathBuf,
    max_capacity: u64,
    /// See [`InflightGates`].
    inflight: InflightGates,
    #[cfg(test)]
    decode_count: Arc<std::sync::atomic::AtomicUsize>,
    #[cfg(test)]
    fail_thumbnail_write: Arc<std::sync::atomic::AtomicBool>,
    #[cfg(test)]
    fail_thumbnail_eviction: Arc<std::sync::atomic::AtomicBool>,
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
            inflight: Arc::new(Mutex::new(HashMap::new())),
            #[cfg(test)]
            decode_count: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
            #[cfg(test)]
            fail_thumbnail_write: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            #[cfg(test)]
            fail_thumbnail_eviction: Arc::new(std::sync::atomic::AtomicBool::new(false)),
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
        self.thumbnail_inner(asset_key, detail)
    }

    fn thumbnail_inner(&self, asset_key: &str, detail: bool) -> Result<Option<Vec<u8>>, Error> {
        let hash = Self::hash_of_key(asset_key)?;
        let max_px = if detail {
            DETAIL_THUMB_PX
        } else {
            LIST_THUMB_PX
        };
        let name = if detail {
            DETAIL_THUMB_FILE
        } else {
            LIST_THUMB_FILE
        };
        let thumb_path = self.entry_dir(&hash).join(name);

        // Fast path: an existing thumbnail never takes the generation gate.
        if let Some(bytes) = read_thumbnail(&thumb_path)? {
            return Ok(Some(bytes));
        }

        let gate_key = (hash, detail);
        let gate = {
            let mut inflight = self
                .inflight
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            Arc::clone(
                inflight
                    .entry(gate_key.clone())
                    .or_insert_with(|| Arc::new(Mutex::new(()))),
            )
        };

        let result = (|| -> Result<Option<Vec<u8>>, Error> {
            let _guard = gate
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            // A previous holder may have produced the file while we waited.
            if let Some(bytes) = read_thumbnail(&thumb_path)? {
                return Ok(Some(bytes));
            }
            self.generate_thumbnail(asset_key, max_px, &thumb_path)
        })();

        // The lock guard was dropped when the generation closure returned.
        // Waiters hold their own `Arc`, so removing the map entry only stops
        // new requests from joining this generation round; it can never
        // invalidate a lock in use.
        let mut inflight = self
            .inflight
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if inflight
            .get(&gate_key)
            .is_some_and(|current| Arc::ptr_eq(current, &gate))
        {
            inflight.remove(&gate_key);
        }
        result
    }

    fn generate_thumbnail(
        &self,
        asset_key: &str,
        max_px: u32,
        thumb_path: &Path,
    ) -> Result<Option<Vec<u8>>, Error> {
        let Some(original) = self.original(asset_key)? else {
            return Ok(None);
        };
        #[cfg(test)]
        self.decode_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let thumbnail = scale_to(&original, max_px)?;

        #[cfg(test)]
        if self
            .fail_thumbnail_write
            .swap(false, std::sync::atomic::Ordering::SeqCst)
        {
            // Leave a truncated temp exactly where an interrupted
            // `write_atomically` would, then fail: the final path must stay
            // absent so no reader can observe a partial JPEG.
            let temp = thumb_path.with_extension("tmp");
            std::fs::write(&temp, b"partial thumbnail")
                .map_err(|source| Error::io("write thumbnail", source, temp.clone()))?;
            return Err(Error::io(
                "place cover",
                std::io::Error::other("injected thumbnail write failure"),
                thumb_path.to_path_buf(),
            ));
        }

        write_atomically(thumb_path, &thumbnail)?;
        Ok(Some(thumbnail))
    }

    /// Total bytes currently stored (diagnostics/GC).
    #[must_use]
    pub fn stored_bytes(&self) -> u64 {
        directory_bytes(&self.dir)
    }

    /// Bring the cache back under `max_capacity` by evicting rebuildable
    /// thumbnails from referenced entries.
    ///
    /// Only the two persisted `thumb-*.bin` names are candidates: the original
    /// and its mime record *are* the referenced asset, so a capacity rule can
    /// never delete them. When those non-rebuildable files alone exceed the
    /// cap, evicting everything would still fail and would force a pointless
    /// decode on every startup — keep the thumbnails and report it instead.
    fn reclaim_thumbnails(&self, survivors: &[(PathBuf, u64)], total: u64) {
        if total <= self.max_capacity {
            return;
        }
        let mut candidates: Vec<ReclaimCandidate> = Vec::new();
        for (entry_dir, _) in survivors {
            let hash = entry_dir
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default();
            let entries = match std::fs::read_dir(entry_dir) {
                Ok(entries) => entries,
                Err(error) => {
                    tracing::warn!(%error, entry = %hash, "cover cache entry scan failed");
                    continue;
                }
            };
            for entry in entries.flatten() {
                let name = entry.file_name();
                let tier = match name.to_string_lossy().as_ref() {
                    DETAIL_THUMB_FILE => 0,
                    LIST_THUMB_FILE => 1,
                    _ => continue,
                };
                let metadata = match entry.metadata() {
                    Ok(metadata) if metadata.is_file() => metadata,
                    Ok(_) => continue,
                    Err(error) => {
                        tracing::debug!(%error, entry = %hash, "thumbnail metadata unavailable");
                        continue;
                    }
                };
                candidates.push(ReclaimCandidate {
                    path: entry.path(),
                    len: metadata.len(),
                    tier,
                    hash: hash.clone(),
                });
            }
        }

        let reclaimable: u64 = candidates.iter().map(|c| c.len).sum();
        let unreclaimable = total.saturating_sub(reclaimable);
        if unreclaimable > self.max_capacity {
            tracing::warn!(
                total,
                capacity = self.max_capacity,
                unreclaimable,
                "cover cache over capacity; referenced originals alone exceed the limit, thumbnails retained"
            );
            return;
        }

        // Deterministic order: detail tier first, then the largest files, then
        // the content hash. Stop as soon as the cap is met so a startup GC
        // never clears more thumbnails than it has to.
        candidates.sort_by(|a, b| {
            a.tier
                .cmp(&b.tier)
                .then(b.len.cmp(&a.len))
                .then(a.hash.cmp(&b.hash))
        });
        let mut remaining = total;
        let mut evicted = 0_u64;
        let mut reclaimed = 0_u64;
        for candidate in candidates {
            if remaining <= self.max_capacity {
                break;
            }
            #[cfg(test)]
            if self
                .fail_thumbnail_eviction
                .swap(false, std::sync::atomic::Ordering::SeqCst)
            {
                tracing::warn!(entry = %candidate.hash, "cover cache thumbnail eviction failed (injected)");
                continue;
            }
            match std::fs::remove_file(&candidate.path) {
                Ok(()) => {}
                // A concurrent reader or another GC may have removed it.
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    // Best effort, as in pass 1: one unremovable file must not
                    // abort the whole GC or justify deleting a referenced asset.
                    tracing::warn!(%error, entry = %candidate.hash, "cover cache thumbnail eviction failed");
                    continue;
                }
            }
            remaining = remaining.saturating_sub(candidate.len);
            reclaimed = reclaimed.saturating_add(candidate.len);
            evicted = evicted.saturating_add(1);
        }
        tracing::debug!(
            total = remaining,
            capacity = self.max_capacity,
            evicted,
            reclaimed,
            "cover cache reclaimed rebuildable thumbnails"
        );
    }

    #[cfg(test)]
    fn decode_count(&self) -> usize {
        self.decode_count.load(std::sync::atomic::Ordering::SeqCst)
    }

    #[cfg(test)]
    fn fail_next_thumbnail_write(&self) {
        self.fail_thumbnail_write
            .store(true, std::sync::atomic::Ordering::SeqCst);
    }

    #[cfg(test)]
    fn fail_next_thumbnail_eviction(&self) {
        self.fail_thumbnail_eviction
            .store(true, std::sync::atomic::Ordering::SeqCst);
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
        Ok(crate::domain::media::cover_asset_key(&hash))
    }

    fn get(&self, asset_key: &str) -> Result<Option<Vec<u8>>, Error> {
        self.original(asset_key)
    }

    fn thumbnail(&self, asset_key: &str, detail: bool) -> Result<Option<Vec<u8>>, Error> {
        self.thumbnail_inner(asset_key, detail)
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
        // Pass 2: capacity reclamation only touches rebuildable thumbnails
        // inside the (already unreferenced-free) survivor set. Originals and
        // mime records belong to the referenced asset and are never removed.
        let total: u64 = survivors.iter().map(|(_, size)| *size).sum();
        self.reclaim_thumbnails(&survivors, total);
        Ok(())
    }
}

fn read_thumbnail(path: &Path) -> Result<Option<Vec<u8>>, Error> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(Error::io("read thumbnail", source, path.to_path_buf())),
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
    // JPEG has no alpha channel. Convert explicitly because newer image crate
    // releases reject RGBA input instead of silently dropping transparency.
    let scaled = image::DynamicImage::ImageRgb8(scaled.to_rgb8());
    let mut out = std::io::Cursor::new(Vec::new());
    scaled
        .write_to(&mut out, ImageFormat::Jpeg)
        .map_err(|source| Error::CorruptMedia {
            operation: "cover thumbnail encode".to_owned(),
            reason: source.to_string(),
        })?;
    Ok(out.into_inner())
}

// Capacity reclamation has its own scenario suite: keeping it beside the
// adapter (rather than growing `cover.rs` past the 1000-line hard limit).
#[cfg(test)]
#[path = "cover_capacity_tests.rs"]
mod capacity_tests;

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

    /// A lossy WebP original (the format most embedded covers use) must
    /// produce both thumbnail tiers. Regression for issue #134: the `image`
    /// crate's WebP decoder used to be compiled out, so every WebP cover
    /// failed as `corrupt_media` and the protocol answered 404.
    #[test]
    fn cover_cache_thumbnail_decodes_webp_original() {
        const WEBP: &[u8] = include_bytes!("fixtures/cover.webp");
        let dir = tempfile::tempdir().unwrap();
        let cache = DiskCoverCache::new(dir.path()).unwrap();
        let key = cache.put(WEBP, "image/webp").unwrap();

        for (detail, max_px) in [(false, LIST_THUMB_PX), (true, DETAIL_THUMB_PX)] {
            let bytes = cache
                .thumbnail(&key, detail)
                .expect("WebP cover thumbnails must be generated")
                .expect("stored cover resolves");
            assert_eq!(&bytes[..3], &[0xFF, 0xD8, 0xFF], "thumbnail is JPEG");
            let scaled = image::load_from_memory(&bytes).unwrap();
            assert!(
                scaled.width() <= max_px && scaled.height() <= max_px,
                "detail={detail} thumbnail exceeds {max_px}px"
            );
            assert!(
                thumb_path(&cache, &key, detail).exists(),
                "detail={detail} thumbnail is persisted"
            );
        }
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
        // A thumbnail of an unknown asset resolves to None, like the original.
        assert_eq!(cache.thumbnail(&unknown, false).unwrap(), None);

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

    pub(super) fn rgba_png(width: u32, height: u32, rgba: image::Rgba<u8>) -> Vec<u8> {
        let buffer = image::RgbaImage::from_pixel(width, height, rgba);
        let mut out = std::io::Cursor::new(Vec::new());
        buffer
            .write_to(&mut out, ImageFormat::Png)
            .expect("encode test png");
        out.into_inner()
    }

    pub(super) fn thumb_path(cache: &DiskCoverCache, key: &str, detail: bool) -> PathBuf {
        let name = if detail {
            DETAIL_THUMB_FILE
        } else {
            LIST_THUMB_FILE
        };
        cache
            .entry_dir(&DiskCoverCache::hash_of_key(key).unwrap())
            .join(name)
    }

    #[test]
    fn cover_cache_thumbnail_is_generated_on_demand_and_reused() {
        let dir = tempfile::tempdir().unwrap();
        let cache = DiskCoverCache::new(dir.path()).unwrap();
        let key = cache
            .put(
                &rgba_png(600, 400, image::Rgba([0, 255, 0, 255])),
                "image/png",
            )
            .unwrap();
        let path = thumb_path(&cache, &key, false);
        assert!(!path.exists(), "old entries have no thumbnail yet");

        let first = cache.thumbnail(&key, false).unwrap().unwrap();
        assert!(
            path.exists(),
            "first visit generates and persists the thumb"
        );
        assert_eq!(cache.decode_count(), 1);

        let second = cache.thumbnail(&key, false).unwrap().unwrap();
        assert_eq!(second, first, "later visits reuse the exact bytes");
        assert_eq!(cache.decode_count(), 1, "no second decode");
    }

    #[test]
    fn cover_cache_thumbnail_tiers_are_independent() {
        let dir = tempfile::tempdir().unwrap();
        let cache = DiskCoverCache::new(dir.path()).unwrap();
        let key = cache
            .put(
                &rgba_png(600, 400, image::Rgba([0, 255, 0, 255])),
                "image/png",
            )
            .unwrap();
        let list_path = thumb_path(&cache, &key, false);
        let detail_path = thumb_path(&cache, &key, true);

        let list = cache.thumbnail(&key, false).unwrap().unwrap();
        assert!(list_path.exists());
        assert!(!detail_path.exists(), "list does not generate detail");
        assert_eq!(cache.decode_count(), 1);

        let detail = cache.thumbnail(&key, true).unwrap().unwrap();
        assert!(detail_path.exists(), "detail generates its own file");
        assert_eq!(cache.decode_count(), 2);
        assert_ne!(list, detail);
        let list_image = image::load_from_memory(&list).unwrap();
        let detail_image = image::load_from_memory(&detail).unwrap();
        assert!(
            list_image.width() <= LIST_THUMB_PX && list_image.height() <= LIST_THUMB_PX,
            "list tier is capped"
        );
        assert!(
            detail_image.width() <= DETAIL_THUMB_PX && detail_image.height() <= DETAIL_THUMB_PX,
            "detail tier is capped"
        );
    }

    #[test]
    fn cover_cache_concurrent_thumbnail_generation_decodes_once() {
        use std::sync::Barrier;

        let dir = tempfile::tempdir().unwrap();
        let cache = DiskCoverCache::new(dir.path()).unwrap();
        let key = cache
            .put(
                &rgba_png(600, 400, image::Rgba([0, 255, 0, 255])),
                "image/png",
            )
            .unwrap();
        let barrier = std::sync::Arc::new(Barrier::new(8));
        // Spawn every worker before joining any: a single map chain would
        // spawn and join one thread at a time, and the first join would wait
        // forever on a barrier that has only one arrival.
        let mut workers = Vec::with_capacity(8);
        for _ in 0..8 {
            let cache = cache.clone();
            let key = key.clone();
            let barrier = std::sync::Arc::clone(&barrier);
            workers.push(std::thread::spawn(move || {
                barrier.wait();
                cache.thumbnail(&key, false).unwrap().unwrap()
            }));
        }
        let outputs: Vec<Vec<u8>> = workers
            .into_iter()
            .map(|handle| handle.join().expect("thumbnail worker"))
            .collect();
        assert_eq!(cache.decode_count(), 1, "one decode for eight callers");
        for output in &outputs {
            assert_eq!(output, &outputs[0], "every caller sees the same bytes");
        }
    }

    #[test]
    fn cover_cache_thumbnail_write_failure_leaves_no_partial_file() {
        let dir = tempfile::tempdir().unwrap();
        let cache = DiskCoverCache::new(dir.path()).unwrap();
        let key = cache
            .put(
                &rgba_png(600, 400, image::Rgba([0, 255, 0, 255])),
                "image/png",
            )
            .unwrap();
        let path = thumb_path(&cache, &key, false);
        let temp = path.with_extension("tmp");

        cache.fail_next_thumbnail_write();
        let error = cache
            .thumbnail(&key, false)
            .expect_err("injected write failure");
        assert_eq!(error.code(), "io");
        assert!(!path.exists(), "no final thumbnail is observable");
        assert!(
            temp.exists(),
            "the interrupted temp is exactly what a crash leaves"
        );

        // The next attempt is not poisoned by the failed generation and
        // replaces the leftover temp atomically.
        let bytes = cache.thumbnail(&key, false).unwrap().unwrap();
        assert!(path.exists());
        let image = image::load_from_memory(&bytes).unwrap();
        assert!(image.width() <= LIST_THUMB_PX);
    }

    #[test]
    fn cover_cache_thumbnail_drops_alpha_and_keeps_jpeg() {
        let dir = tempfile::tempdir().unwrap();
        let cache = DiskCoverCache::new(dir.path()).unwrap();
        // A fully transparent 64x64 PNG.
        let key = cache
            .put(&rgba_png(64, 64, image::Rgba([0, 255, 0, 0])), "image/png")
            .unwrap();
        let bytes = cache.thumbnail(&key, false).unwrap().unwrap();
        assert_eq!(&bytes[..3], &[0xFF, 0xD8, 0xFF], "JPEG magic");
        let image = image::load_from_memory(&bytes).unwrap();
        assert_eq!(
            image.color(),
            image::ColorType::Rgb8,
            "JPEG output has no alpha channel; transparency is dropped"
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
            let error = cache.thumbnail(bad, false).expect_err("bad thumbnail key");
            assert_eq!(error.code(), "validation", "thumbnail key {bad:?}");
        }
        // gc silently ignores malformed keep keys (they can never match an
        // entry dir, which is the exact 64-hex hash).
        cache.gc(&["bad-key".to_owned()]).unwrap();
    }
}
