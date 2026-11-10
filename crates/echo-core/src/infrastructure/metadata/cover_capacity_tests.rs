//! Capacity reclamation scenarios for `DiskCoverCache` (issue #126).
//!
//! Colocated with the adapter as a sibling module so `cover.rs` stays below
//! the `CODE_STANDARDS §2.1` 1000-line hard limit.

use super::tests::{rgba_png, thumb_path};
use super::*;

/// One referenced cover plus the byte count of `original.bin` + `mime.txt`.
fn fixture(dir: &Path) -> (DiskCoverCache, String, Vec<u8>, u64) {
    let cache = DiskCoverCache::new(dir).unwrap();
    let bytes = rgba_png(600, 400, image::Rgba([0, 255, 0, 255]));
    let key = cache.put(&bytes, "image/png").unwrap();
    let base = cache.stored_bytes();
    (cache, key, bytes, base)
}

fn both_tiers(cache: &DiskCoverCache, key: &str) {
    cache.thumbnail(key, false).unwrap().unwrap();
    cache.thumbnail(key, true).unwrap().unwrap();
}

fn file_len(path: &Path) -> u64 {
    std::fs::metadata(path).unwrap().len()
}

/// Regression for issue #126: list/detail thumbnails land inside an
/// already-referenced entry, so an over-capacity cache must reclaim the
/// rebuildable files instead of only logging the size.
#[test]
fn cover_cache_capacity_thumbnails_push_cache_over_limit() {
    let dir = tempfile::tempdir().unwrap();
    let (cache, key, bytes, base) = fixture(dir.path());
    both_tiers(&cache, &key);
    // Original + mime.txt fit; the generated thumbnails do not.
    let capacity = base + 1;
    let cache = cache.with_capacity(capacity);
    assert!(
        cache.stored_bytes() > capacity,
        "thumbnails push the entry over the cap: {} > {}",
        cache.stored_bytes(),
        capacity
    );

    cache.gc(std::slice::from_ref(&key)).unwrap();
    assert!(
        cache.stored_bytes() <= capacity,
        "gc must bring the cache back under the cap: {} <= {}",
        cache.stored_bytes(),
        capacity
    );
    assert_eq!(
        cache.original(&key).unwrap().as_deref(),
        Some(bytes.as_slice()),
        "referenced original is never evicted"
    );
}

/// When the referenced originals alone exceed the cap, evicting every
/// thumbnail cannot help: GC must keep them all and stay idempotent.
#[test]
fn cover_cache_capacity_unreachable_limit_retains_thumbnails() {
    let dir = tempfile::tempdir().unwrap();
    let (cache, key, bytes, base) = fixture(dir.path());
    let cache = cache.with_capacity(base - 1);
    both_tiers(&cache, &key);
    let before = cache.stored_bytes();
    let list_path = thumb_path(&cache, &key, false);
    let detail_path = thumb_path(&cache, &key, true);

    cache.gc(std::slice::from_ref(&key)).unwrap();
    assert_eq!(cache.stored_bytes(), before, "no pointless eviction");
    assert!(list_path.exists(), "list thumbnail retained");
    assert!(detail_path.exists(), "detail thumbnail retained");
    assert_eq!(
        cache.original(&key).unwrap().as_deref(),
        Some(bytes.as_slice())
    );

    cache.gc(std::slice::from_ref(&key)).unwrap();
    assert_eq!(
        cache.stored_bytes(),
        before,
        "a second startup GC is idempotent"
    );
    assert!(list_path.exists() && detail_path.exists());
}

#[test]
fn cover_cache_capacity_evicts_detail_tier_before_list_tier() {
    let dir = tempfile::tempdir().unwrap();
    let (cache, key, bytes, base) = fixture(dir.path());
    both_tiers(&cache, &key);
    let list_path = thumb_path(&cache, &key, false);
    let detail_path = thumb_path(&cache, &key, true);
    // Deleting only the 512 px tier is enough to reach the cap.
    let capacity = base + file_len(&list_path);
    let cache = cache.with_capacity(capacity);
    assert!(cache.stored_bytes() > capacity);

    cache.gc(std::slice::from_ref(&key)).unwrap();
    assert!(!detail_path.exists(), "512 px tier is evicted first");
    assert!(list_path.exists(), "128 px tier is retained");
    assert!(cache.stored_bytes() <= capacity);
    assert_eq!(
        cache.original(&key).unwrap().as_deref(),
        Some(bytes.as_slice())
    );
}

#[test]
fn cover_cache_capacity_stops_evicting_once_under_limit() {
    let dir = tempfile::tempdir().unwrap();
    let (cache, keep, _, _) = fixture(dir.path());
    let other_bytes = rgba_png(320, 240, image::Rgba([0, 0, 255, 255]));
    let other = cache.put(&other_bytes, "image/png").unwrap();
    both_tiers(&cache, &keep);
    cache.thumbnail(&other, false).unwrap().unwrap();
    let detail = thumb_path(&cache, &keep, true);
    let keep_list = thumb_path(&cache, &keep, false);
    let other_list = thumb_path(&cache, &other, false);
    let total = cache.stored_bytes();
    let capacity = total - file_len(&detail);
    let cache = cache.with_capacity(capacity);

    cache.gc(&[keep, other]).unwrap();
    assert!(!detail.exists(), "highest-priority candidate evicted");
    assert!(keep_list.exists(), "eviction stopped at the cap");
    assert!(other_list.exists(), "other entry's thumbnail retained");
    assert_eq!(cache.stored_bytes(), capacity);
}

#[test]
fn cover_cache_capacity_never_touches_originals_mime_or_temp_files() {
    let dir = tempfile::tempdir().unwrap();
    let (cache, key, bytes, base) = fixture(dir.path());
    both_tiers(&cache, &key);
    let list_path = thumb_path(&cache, &key, false);
    let detail_path = thumb_path(&cache, &key, true);
    let list_tmp = list_path.with_extension("tmp");
    let detail_tmp = detail_path.with_extension("tmp");
    std::fs::write(&list_tmp, b"interrupted list").unwrap();
    std::fs::write(&detail_tmp, b"interrupted detail").unwrap();
    let mime_path = cache
        .entry_dir(&DiskCoverCache::hash_of_key(&key).unwrap())
        .join("mime.txt");
    let mime = std::fs::read(&mime_path).unwrap();
    let tmp_bytes = file_len(&list_tmp) + file_len(&detail_tmp);
    let cache = cache.with_capacity(base + tmp_bytes + 1);

    cache.gc(std::slice::from_ref(&key)).unwrap();
    assert!(
        !list_path.exists() && !detail_path.exists(),
        "thumbs evicted"
    );
    assert_eq!(
        cache.original(&key).unwrap().as_deref(),
        Some(bytes.as_slice())
    );
    assert_eq!(std::fs::read(&mime_path).unwrap(), mime);
    assert_eq!(std::fs::read(&list_tmp).unwrap(), b"interrupted list");
    assert_eq!(std::fs::read(&detail_tmp).unwrap(), b"interrupted detail");
}

#[test]
fn cover_cache_capacity_eviction_failure_continues_with_other_candidates() {
    let dir = tempfile::tempdir().unwrap();
    let (cache, key, bytes, base) = fixture(dir.path());
    both_tiers(&cache, &key);
    let cache = cache.with_capacity(base + 1);
    cache.fail_next_thumbnail_eviction();

    cache.gc(std::slice::from_ref(&key)).unwrap();
    assert!(
        thumb_path(&cache, &key, true).exists(),
        "first candidate's removal failed"
    );
    assert!(
        !thumb_path(&cache, &key, false).exists(),
        "later candidates are still processed"
    );
    assert_eq!(
        cache.original(&key).unwrap().as_deref(),
        Some(bytes.as_slice()),
        "a failed eviction never escalates to the original"
    );
}

#[test]
fn cover_cache_capacity_still_removes_unreferenced_entries_entirely() {
    let dir = tempfile::tempdir().unwrap();
    let (cache, keep, bytes, base) = fixture(dir.path());
    let dropped = cache.put(b"unreferenced-cover", "image/png").unwrap();
    both_tiers(&cache, &keep);
    let dropped_dir = cache.entry_dir(&DiskCoverCache::hash_of_key(&dropped).unwrap());
    let cache = cache.with_capacity(base + 1);

    cache.gc(std::slice::from_ref(&keep)).unwrap();
    assert!(!dropped_dir.exists(), "pass 1 removes the whole directory");
    assert_eq!(
        cache.original(&keep).unwrap().as_deref(),
        Some(bytes.as_slice())
    );
    assert!(cache.stored_bytes() <= base + 1);
}

#[test]
fn cover_cache_capacity_ignores_malformed_reference_keys() {
    let dir = tempfile::tempdir().unwrap();
    let (cache, key, bytes, base) = fixture(dir.path());
    both_tiers(&cache, &key);
    let cache = cache.with_capacity(base + 1);

    cache
        .gc(&[key.clone(), "bad-key".to_owned(), "cv1-zzz".to_owned()])
        .unwrap();
    assert!(cache.stored_bytes() <= base + 1);
    assert_eq!(
        cache.original(&key).unwrap().as_deref(),
        Some(bytes.as_slice())
    );
}

#[test]
fn cover_cache_capacity_keeps_every_referenced_original() {
    let dir = tempfile::tempdir().unwrap();
    let (cache, first, first_bytes, _) = fixture(dir.path());
    let second_bytes = rgba_png(700, 300, image::Rgba([255, 0, 0, 255]));
    let second = cache.put(&second_bytes, "image/png").unwrap();
    both_tiers(&cache, &first);
    both_tiers(&cache, &second);
    let total = cache.stored_bytes();
    let cache = cache.with_capacity(total / 2);

    cache.gc(&[first.clone(), second.clone()]).unwrap();
    assert_eq!(
        cache.original(&first).unwrap().as_deref(),
        Some(first_bytes.as_slice())
    );
    assert_eq!(
        cache.original(&second).unwrap().as_deref(),
        Some(second_bytes.as_slice())
    );
    assert!(cache.stored_bytes() < total, "thumbnails were reclaimed");
}

#[test]
fn cover_cache_capacity_evicted_thumbnail_is_rebuilt_and_reused() {
    let dir = tempfile::tempdir().unwrap();
    let (cache, key, bytes, base) = fixture(dir.path());
    both_tiers(&cache, &key);
    let cache = cache.with_capacity(base + 1);
    let list_path = thumb_path(&cache, &key, false);
    cache.gc(std::slice::from_ref(&key)).unwrap();
    assert!(!list_path.exists());

    let decodes = cache.decode_count();
    let first = cache.thumbnail(&key, false).unwrap().unwrap();
    assert!(list_path.exists(), "rebuilt from the retained original");
    assert_eq!(cache.decode_count(), decodes + 1);
    let second = cache.thumbnail(&key, false).unwrap().unwrap();
    assert_eq!(first, second, "disk hit returns the same bytes");
    assert_eq!(cache.decode_count(), decodes + 1, "disk hit is reused");
    let image = image::load_from_memory(&second).unwrap();
    assert!(image.width() <= LIST_THUMB_PX && image.height() <= LIST_THUMB_PX);
    assert_eq!(
        cache.original(&key).unwrap().as_deref(),
        Some(bytes.as_slice())
    );
}

#[test]
fn cover_cache_capacity_rebuild_is_single_flight() {
    let dir = tempfile::tempdir().unwrap();
    let (cache, key, _, base) = fixture(dir.path());
    cache.thumbnail(&key, false).unwrap().unwrap();
    let cache = cache.with_capacity(base + 1);
    cache.gc(std::slice::from_ref(&key)).unwrap();
    let decodes = cache.decode_count();

    let outputs: Vec<Vec<u8>> = (0..8)
        .map(|_| {
            let cache = cache.clone();
            let key = key.clone();
            std::thread::spawn(move || cache.thumbnail(&key, false).unwrap().unwrap())
        })
        .map(|handle| handle.join().expect("thumbnail worker"))
        .collect();
    assert_eq!(cache.decode_count(), decodes + 1, "one decode after GC");
    assert!(outputs.iter().all(|bytes| bytes == &outputs[0]));
}

#[test]
fn cover_cache_capacity_rebuild_failure_leaves_no_partial_file() {
    let dir = tempfile::tempdir().unwrap();
    let (cache, key, bytes, base) = fixture(dir.path());
    cache.thumbnail(&key, false).unwrap().unwrap();
    let cache = cache.with_capacity(base + 1);
    let list_path = thumb_path(&cache, &key, false);
    let temp = list_path.with_extension("tmp");
    cache.gc(std::slice::from_ref(&key)).unwrap();
    assert!(!list_path.exists());

    cache.fail_next_thumbnail_write();
    let error = cache
        .thumbnail(&key, false)
        .expect_err("injected write failure");
    assert_eq!(error.code(), "io");
    assert!(!list_path.exists(), "no readable partial thumbnail");
    assert!(
        temp.exists(),
        "the interrupted temp is where a crash leaves it"
    );
    assert_eq!(
        cache.original(&key).unwrap().as_deref(),
        Some(bytes.as_slice())
    );

    let rebuilt = cache.thumbnail(&key, false).unwrap().unwrap();
    assert!(list_path.exists());
    assert!(image::load_from_memory(&rebuilt).unwrap().width() <= LIST_THUMB_PX);
}
