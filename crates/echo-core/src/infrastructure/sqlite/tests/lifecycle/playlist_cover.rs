//! Artwork references, derived keys and frozen bodies survive ordered recovery.
use super::*;
use crate::application::ports::{CoverCache, LibraryProjectionPort, PlaylistRepository};
use crate::domain::library::durability::{FrozenMutation, StateTarget};
use crate::infrastructure::metadata::DiskCoverCache;

#[test]
fn library_durability_playlist_cover_reuses_original_and_clear_survives_rebuild() {
    let stack = ScanStack::new();
    let (_, control, deps) = configured_roots(&stack);
    let edits = DurableLibraryEdits {
        control: deps.control.as_ref(),
        songs: deps.songs.as_ref(),
        playlists: deps.playlists.as_ref(),
        unit: deps.uow.as_ref(),
        device: deps.device_id.current_device_id(),
    };
    let first = edits.create_playlist(stack.root, "first cover").unwrap();
    let second = edits.create_playlist(stack.root, "second cover").unwrap();
    let original = original_png();
    for id in [first, second] {
        edits
            .set_playlist_cover(stack.root, id, Some(&original), Some("image/png"))
            .unwrap();
    }
    let hash = blake3::hash(&original).to_hex().to_string();
    assert_eq!(
        std::fs::read_dir(stack.library_dir.join("echo/assets"))
            .unwrap()
            .count(),
        1
    );
    edits
        .set_playlist_cover(stack.root, first, None, None)
        .unwrap();
    edits
        .rename_playlist(stack.root, second, "cover preserved")
        .unwrap();
    let replay = control.durability().unwrap().recover(stack.root).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let rebuilt =
        crate::infrastructure::sqlite::SqliteDatabase::open(directory.path().join("fresh.sqlite"))
            .unwrap();
    LibraryRepository::upsert(
        &rebuilt,
        &LibraryRoot::new(stack.root, stack.library_dir.clone(), true, true),
    )
    .unwrap();
    for _ in 0..2 {
        rebuilt
            .project_library_commits(stack.root, &replay)
            .unwrap();
        assert!(
            PlaylistRepository::cover_key(&rebuilt, first)
                .unwrap()
                .is_none()
        );
        assert_eq!(
            PlaylistRepository::cover_key(&rebuilt, second).unwrap(),
            Some(crate::domain::media::cover_asset_key(&hash))
        );
    }
    assert_eq!(
        control
            .durability()
            .unwrap()
            .read_asset(stack.root, &hash)
            .unwrap(),
        Some(original)
    );
}

#[test]
fn library_durability_playlist_cover_missing_or_corrupt_original_is_not_recovered() {
    let stack = ScanStack::new();
    let (_, control, deps) = configured_roots(&stack);
    let edits = DurableLibraryEdits {
        control: deps.control.as_ref(),
        songs: deps.songs.as_ref(),
        playlists: deps.playlists.as_ref(),
        unit: deps.uow.as_ref(),
        device: deps.device_id.current_device_id(),
    };
    let playlist = edits.create_playlist(stack.root, "damaged cover").unwrap();
    let original = original_png();
    edits
        .set_playlist_cover(stack.root, playlist, Some(&original), Some("image/png"))
        .unwrap();
    let hash = blake3::hash(&original).to_hex().to_string();
    let path = stack.library_dir.join("echo/assets").join(&hash);
    let directory = tempfile::tempdir().unwrap();
    let cache = DiskCoverCache::new(directory.path()).unwrap();
    for corrupt in [true, false] {
        if corrupt {
            std::fs::write(&path, b"corrupted").unwrap();
        } else {
            std::fs::remove_file(&path).unwrap();
        }
        assert!(
            crate::application::playlist::cover::resolve_playlist_cover(
                control.as_ref(),
                &cache,
                stack.root,
                playlist,
                None,
            )
            .is_err()
        );
        assert!(
            cache
                .get(&crate::domain::media::cover_asset_key(&hash))
                .unwrap()
                .is_none()
        );
    }
    assert!(
        matches!(control.read_record(stack.root, crate::domain::library::RecordKind::Playlist, &playlist.to_string()).unwrap(), Some(PortableRecord::Playlist(record)) if record.cover_asset.is_some())
    );
}

#[test]
fn library_durability_playlist_cover_asset_write_failure_keeps_saved_playlist() {
    let stack = ScanStack::new();
    let (_, control, deps) = configured_roots(&stack);
    let edits = DurableLibraryEdits {
        control: deps.control.as_ref(),
        songs: deps.songs.as_ref(),
        playlists: deps.playlists.as_ref(),
        unit: deps.uow.as_ref(),
        device: deps.device_id.current_device_id(),
    };
    let playlist = edits
        .create_playlist(stack.root, "unchanged cover")
        .unwrap();
    let before = control.durability().unwrap().recover(stack.root).unwrap();
    let blocker = stack.library_dir.join("echo/assets");
    std::fs::write(&blocker, b"asset directory blocker").unwrap();
    assert!(
        edits
            .set_playlist_cover(
                stack.root,
                playlist,
                Some(&original_png()),
                Some("image/png")
            )
            .is_err()
    );
    assert_eq!(
        control.durability().unwrap().recover(stack.root).unwrap(),
        before
    );
    assert!(deps.playlists.cover_key(playlist).unwrap().is_none());
    assert_eq!(std::fs::read(&blocker).unwrap(), b"asset directory blocker");
}

#[test]
fn library_durability_playlist_cover_legacy_cache_gap_is_explicit() {
    let stack = ScanStack::new();
    let (_, control, deps) = configured_roots(&stack);
    let edits = DurableLibraryEdits {
        control: deps.control.as_ref(),
        songs: deps.songs.as_ref(),
        playlists: deps.playlists.as_ref(),
        unit: deps.uow.as_ref(),
        device: deps.device_id.current_device_id(),
    };
    let playlist = edits
        .create_playlist(stack.root, "legacy cache evidence")
        .unwrap();
    let directory = tempfile::tempdir().unwrap();
    let cache = DiskCoverCache::new(directory.path()).unwrap();
    let key = cache.put(&original_png(), "image/png").unwrap();
    let before = control.durability().unwrap().recover(stack.root).unwrap();
    let resolve = || {
        crate::application::playlist::cover::resolve_playlist_cover(
            control.as_ref(),
            &cache,
            stack.root,
            playlist,
            Some(&key),
        )
    };
    assert_eq!(resolve().unwrap(), Some(key.clone()));
    cache.gc(&[]).unwrap();
    assert!(resolve().is_err());
    assert_eq!(
        control.durability().unwrap().recover(stack.root).unwrap(),
        before
    );
    assert!(
        crate::application::playlist::cover::resolve_playlist_cover(
            control.as_ref(),
            &cache,
            stack.root,
            playlist,
            None,
        )
        .unwrap()
        .is_none(),
        "absence of evidence does not invent historical loss"
    );
}

pub(super) fn original_png() -> Vec<u8> {
    let image = image::RgbaImage::from_pixel(4, 4, image::Rgba([200, 50, 50, 255]));
    let mut bytes = std::io::Cursor::new(Vec::new());
    image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
    bytes.into_inner()
}

#[test]
fn library_durability_playlist_cover_saved_pending_rebuilds_without_cache() {
    let stack = ScanStack::new();
    let (_, control, deps) = configured_roots(&stack);
    let edits = DurableLibraryEdits {
        control: deps.control.as_ref(),
        songs: deps.songs.as_ref(),
        playlists: deps.playlists.as_ref(),
        unit: deps.uow.as_ref(),
        device: deps.device_id.current_device_id(),
    };
    let playlist = edits.create_playlist(stack.root, "cover").unwrap();
    let original = original_png();
    stack.database.writer.run(|c| c.execute_batch(
        "CREATE TRIGGER fail_cover_projection BEFORE INSERT ON library_projection_watermarks BEGIN SELECT RAISE(ABORT,'injected cover failure'); END;"
    ).map_err(storage)).unwrap();
    assert!(matches!(
        edits.set_playlist_cover(stack.root, playlist, Some(&original), Some("image/png")),
        Err(crate::error::Error::LibrarySavedPending { .. })
    ));
    assert!(deps.playlists.cover_key(playlist).unwrap().is_none());
    let replay = control.durability().unwrap().recover(stack.root).unwrap();
    let batch = &replay.commits.last().unwrap().batch;
    let PortableRecord::Playlist(record) = batch.records[0].record() else {
        panic!("playlist expected")
    };
    let asset = record.cover_asset.as_ref().unwrap();
    assert_eq!(batch.resources.len(), 1);
    let frozen: FrozenMutation = batch
        .state
        .iter()
        .find(|state| {
            matches!(
                state.key.as_ref().map(|key| &key.target),
                Some(StateTarget::Mutation { .. })
            )
        })
        .map(|state| serde_json::from_value(state.payload["frozen"].clone()).unwrap())
        .unwrap();
    assert_eq!(
        frozen.candidate.payload["cover_asset"],
        serde_json::to_value(asset).unwrap()
    );
    let directory = tempfile::tempdir().unwrap();
    let rebuilt =
        crate::infrastructure::sqlite::SqliteDatabase::open(directory.path().join("fresh.sqlite"))
            .unwrap();
    LibraryRepository::upsert(
        &rebuilt,
        &LibraryRoot::new(stack.root, stack.library_dir.clone(), true, true),
    )
    .unwrap();
    let cache = DiskCoverCache::new(directory.path().join("cache")).unwrap();
    for _ in 0..2 {
        rebuilt
            .project_library_commits(stack.root, &replay)
            .unwrap();
        let key = PlaylistRepository::cover_key(&rebuilt, playlist)
            .unwrap()
            .expect("derived key");
        let actual = crate::application::playlist::cover::resolve_playlist_cover(
            control.as_ref(),
            &cache,
            stack.root,
            playlist,
            Some(&key),
        )
        .unwrap()
        .unwrap();
        assert_eq!(actual, key);
        assert_eq!(
            cache.get(&actual).unwrap().as_deref(),
            Some(original.as_slice())
        );
        let thumbnail = cache.thumbnail(&actual, false).unwrap().unwrap();
        let decoded = image::load_from_memory(&thumbnail).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (4, 4));
        cache.gc(&[]).unwrap();
    }
}
