//! Library-private artist artwork is recoverable without SQLite or app caches.
use super::*;
use crate::application::artist_cover::{resolve_artist_cover, set_artist_cover};
use crate::application::ports::{CoverCache, CoverRepository, LibraryProjectionPort};
use crate::domain::library::durability::{ArtistCoverState, StateKind};
use crate::infrastructure::metadata::DiskCoverCache;

fn save(
    stack: &ScanStack,
    control: &dyn ControlPlanePort,
    artist: &str,
    bytes: Option<&[u8]>,
) -> Result<(), crate::error::Error> {
    set_artist_cover(
        control,
        stack.database.as_ref(),
        stack.root,
        artist,
        bytes,
        bytes.map(|_| "image/png"),
        stack.database.current_device_id(),
    )
}

#[test]
fn library_durability_artist_cover_reuses_original_and_restores_after_database_and_cache_loss() {
    let stack = ScanStack::new();
    let (other_root, control, _) = configured_roots(&stack);
    let bytes = super::playlist_cover::original_png();
    save(&stack, control.as_ref(), "歌手", Some(&bytes)).unwrap();
    save(&stack, control.as_ref(), "other artist", Some(&bytes)).unwrap();
    let replay = control.durability().unwrap().recover(stack.root).unwrap();
    save(&stack, control.as_ref(), "歌手", Some(&bytes)).unwrap();
    assert_eq!(
        control.durability().unwrap().recover(stack.root).unwrap(),
        replay
    );
    assert_eq!(
        std::fs::read_dir(stack.library_dir.join("echo/assets"))
            .unwrap()
            .count(),
        1
    );
    assert!(
        CoverRepository::artist_cover_key(stack.database.as_ref(), other_root, "歌手")
            .unwrap()
            .is_none()
    );
    let (directory, rebuilt) = fresh_database(&stack);
    let cache = DiskCoverCache::new(directory.path().join("covers")).unwrap();
    std::fs::remove_file(stack.library_dir.join("echo/write.lock")).unwrap();
    for _ in 0..2 {
        rebuilt
            .project_library_commits(stack.root, &replay)
            .unwrap();
        let asset = CoverRepository::artist_cover_original(&rebuilt, stack.root, "歌手")
            .unwrap()
            .unwrap();
        assert_eq!(asset.byte_len, bytes.len() as u64);
        assert_eq!(asset.mime, "image/png");
        assert_eq!(asset.content_hash, blake3::hash(&bytes).to_hex().as_str());
        let projected_key = CoverRepository::artist_cover_key(&rebuilt, stack.root, "歌手")
            .unwrap()
            .unwrap();
        let key = resolve_artist_cover(
            control.as_ref(),
            &cache,
            stack.root,
            Some(&asset),
            Some(&projected_key),
        )
        .unwrap()
        .unwrap();
        assert_eq!(key, projected_key);
        assert_eq!(cache.get(&key).unwrap().as_deref(), Some(bytes.as_slice()));
        let thumbnail = cache.thumbnail(&key, false).unwrap().unwrap();
        assert_eq!(image::load_from_memory(&thumbnail).unwrap().width(), 4);
        cache.gc(&[]).unwrap();
    }
    assert!(
        !stack.library_dir.join("echo/write.lock").exists(),
        "artwork reads never acquire a writer lock"
    );
    assert_eq!(
        rebuilt
            .with_reader(|c| c
                .query_row("SELECT COUNT(*) FROM sync_mutations", [], |r| r
                    .get::<_, u64>(0))
                .map_err(storage))
            .unwrap(),
        0
    );
    save(&stack, control.as_ref(), "歌手", None).unwrap();
    let replay = control.durability().unwrap().recover(stack.root).unwrap();
    let (_cleared_dir, cleared) = fresh_database(&stack);
    cleared
        .project_library_commits(stack.root, &replay)
        .unwrap();
    assert!(
        CoverRepository::artist_cover_key(&cleared, stack.root, "歌手")
            .unwrap()
            .is_none()
    );
    assert!(
        CoverRepository::artist_cover_original(&cleared, stack.root, "歌手")
            .unwrap()
            .is_none()
    );
    assert!(
        CoverRepository::artist_cover_key(&cleared, stack.root, "other artist")
            .unwrap()
            .is_some()
    );
    assert_eq!(
        std::fs::read_dir(stack.library_dir.join("echo/assets"))
            .unwrap()
            .count(),
        1,
        "clear retains historically referenced originals"
    );
}

#[test]
fn library_durability_artist_cover_never_projects_before_commit_and_retries_saved_original() {
    let stack = ScanStack::new();
    let (_, control, _) = configured_roots(&stack);
    let bytes = super::playlist_cover::original_png();
    let rejecting = Arc::new(control.as_ref().clone().with_commit_hook(|phase, _| {
        if phase == "prepared" {
            return Err(crate::error::Error::unavailable(
                "artist artwork",
                "injected directory failure",
            ));
        }
        Ok(())
    }));
    stack.database.configure_library_control(rejecting.clone());
    assert!(save(&stack, rejecting.as_ref(), "artist", Some(&bytes)).is_err());
    assert!(
        CoverRepository::artist_cover_key(stack.database.as_ref(), stack.root, "artist")
            .unwrap()
            .is_none()
    );
    assert!(control
        .durability()
        .unwrap()
        .recover(stack.root)
        .unwrap()
        .commits
        .is_empty());
    stack.database.configure_library_control(control.clone());
    stack.database.writer.run(|c| c.execute_batch("CREATE TRIGGER fail_artist_cover BEFORE INSERT ON artist_covers BEGIN SELECT RAISE(ABORT,'injected projection failure'); END;").map_err(storage)).unwrap();
    let error = save(&stack, control.as_ref(), "artist", Some(&bytes)).unwrap_err();
    assert!(matches!(
        error,
        crate::error::Error::LibrarySavedPending { .. }
    ));
    assert!(
        CoverRepository::artist_cover_key(stack.database.as_ref(), stack.root, "artist")
            .unwrap()
            .is_none()
    );
    let replay = control.durability().unwrap().recover(stack.root).unwrap();
    let state = replay
        .states()
        .find(|state| state.kind == StateKind::ArtistCover)
        .unwrap();
    let artwork = ArtistCoverState::from_change(state).unwrap();
    assert_eq!(artwork.asset.unwrap().byte_len, bytes.len() as u64);
    stack
        .database
        .writer
        .run(|c| {
            c.execute_batch("DROP TRIGGER fail_artist_cover")
                .map_err(storage)
        })
        .unwrap();
    save(&stack, control.as_ref(), "artist", Some(&bytes)).unwrap();
    assert_eq!(
        control.durability().unwrap().recover(stack.root).unwrap(),
        replay,
        "retry projects the same saved state without a second edit"
    );
    assert!(
        CoverRepository::artist_cover_original(stack.database.as_ref(), stack.root, "artist")
            .unwrap()
            .is_some()
    );
}

#[test]
fn library_durability_artist_cover_reports_historical_gap_and_missing_original_without_writes() {
    let stack = ScanStack::new();
    let (_, control, _) = configured_roots(&stack);
    let directory = tempfile::tempdir().unwrap();
    let cache = DiskCoverCache::new(directory.path()).unwrap();
    let bytes = super::playlist_cover::original_png();
    let legacy_key = cache.put(&bytes, "image/png").unwrap();
    assert_eq!(
        resolve_artist_cover(
            control.as_ref(),
            &cache,
            stack.root,
            None,
            Some(&legacy_key)
        )
        .unwrap(),
        Some(legacy_key.clone())
    );
    let before = control.durability().unwrap().recover(stack.root).unwrap();
    cache.gc(&[]).unwrap();
    assert!(resolve_artist_cover(
        control.as_ref(),
        &cache,
        stack.root,
        None,
        Some(&legacy_key)
    )
    .is_err());
    assert_eq!(
        control.durability().unwrap().recover(stack.root).unwrap(),
        before
    );
    save(&stack, control.as_ref(), "artist", Some(&bytes)).unwrap();
    let asset =
        CoverRepository::artist_cover_original(stack.database.as_ref(), stack.root, "artist")
            .unwrap()
            .unwrap();
    let original = stack
        .library_dir
        .join("echo/assets")
        .join(&asset.content_hash);
    std::fs::write(&original, b"corrupt").unwrap();
    assert!(
        resolve_artist_cover(control.as_ref(), &cache, stack.root, Some(&asset), None).is_err()
    );
    std::fs::remove_file(&original).unwrap();
    assert!(
        resolve_artist_cover(control.as_ref(), &cache, stack.root, Some(&asset), None).is_err()
    );
}
