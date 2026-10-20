//! Convergence across rescans: lyrics/cover persistence, revision stability, root remount, watcher permission degradation and Unicode paths.

#[allow(clippy::wildcard_imports)]
use super::*;

#[test]
fn embedded_and_sidecar_lyrics_are_persisted_per_source() {
    let fixture = ScanFixture::new();
    fixture.write_file("song.mp3", b"audio");
    fixture.write_file("song.lrc", b"[00:01.00]sidecar line");
    fixture.set_audio("song.mp3", "Song", 1_000);
    // The reader reports embedded lyrics; the parser turns them into lines.
    fixture.metadata.set(
        "song.mp3",
        crate::domain::media::ParsedMetadata {
            title: Some("Song".to_owned()),
            artist: Some("歌手".to_owned()),
            album: Some("专辑".to_owned()),
            duration: Some(std::time::Duration::from_secs(1)),
            format: crate::domain::media::AudioFormat::Flac,
            embedded_lyrics: Some("[00:01.00]embedded line".to_owned()),
            ..crate::domain::media::ParsedMetadata::default()
        },
    );
    start_scan(&fixture).run(fixture.root).expect("scan");
    let song = fixture.all_songs().into_iter().next().expect("song");
    let candidates =
        crate::application::ports::LyricsRepository::candidates(&fixture.database, song.id())
            .expect("candidates");
    let sources: Vec<_> = candidates
        .iter()
        .map(crate::domain::entities::LyricsCandidate::source)
        .collect();
    assert!(sources.contains(&crate::domain::entities::LyricsSource::Embedded));
    assert!(sources.contains(&crate::domain::entities::LyricsSource::Sidecar));
    // No stale candidates survive a rescan: the audio changes (so the
    // file is re-parsed, not fast-skipped) and the sidecar is gone.
    fixture.write_file("song.mp3", b"audio-v2");
    fixture.remove_file("song.lrc");
    fixture.set_audio("song.mp3", "Song", 1_000);
    start_scan(&fixture).run(fixture.root).expect("rescan");
    let candidates =
        crate::application::ports::LyricsRepository::candidates(&fixture.database, song.id())
            .expect("candidates");
    assert!(
        !candidates
            .iter()
            .any(|c| c.source() == crate::domain::entities::LyricsSource::Sidecar),
        "cleared sidecar must not linger: {candidates:?}"
    );
}

#[test]
fn cover_cache_key_and_db_reference_stay_consistent() {
    let fixture = ScanFixture::new();
    fixture.write_file("song.mp3", b"audio");
    fixture.set_audio("song.mp3", "Song", 1_000);
    fixture.metadata.set(
        "song.mp3",
        crate::domain::media::ParsedMetadata {
            title: Some("Song".to_owned()),
            cover: Some(crate::domain::media::EmbeddedCover {
                bytes: b"cover-bytes".to_vec(),
                mime: "image/png".to_owned(),
            }),
            ..crate::domain::media::ParsedMetadata::default()
        },
    );
    start_scan(&fixture).run(fixture.root).expect("scan");
    let song = fixture.all_songs().into_iter().next().expect("song");
    let cover = crate::application::ports::CoverRepository::cover_of(&fixture.database, song.id())
        .expect("cover ref")
        .expect("cover attached");
    // The asset key round-trips through the cache: opaque, content keyed.
    assert_eq!(
        fixture
            .deps
            .cover_cache
            .get(&cover.asset_key)
            .expect("get")
            .as_deref(),
        Some(b"cover-bytes".as_slice())
    );
    assert_eq!(cover.mime, "image/png");
    // And the referenced key is exactly the GC keep-set.
    let keys = crate::application::ports::CoverRepository::referenced_asset_keys(
        &fixture.database,
        fixture.root,
    )
    .expect("keys");
    assert_eq!(keys, vec![cover.asset_key]);
}

#[test]
fn revision_and_added_at_survive_rescans() {
    let fixture = ScanFixture::new();
    fixture.write_file("a.mp3", b"audio");
    fixture.set_audio("a.mp3", "A", 1_000);
    start_scan(&fixture).run(fixture.root).expect("scan");
    let song = fixture.all_songs().into_iter().next().unwrap();
    let added_at = song.added_at();
    assert_eq!(
        added_at,
        fixture.clock.wall_ms(),
        "uses the injected wall clock"
    );
    // Change the file content and its tags → the rescan re-parses and
    // updates, not re-creates (an unchanged file is a fast-skip by
    // design).
    fixture.write_file("a.mp3", b"audio-v2-longer");
    fixture.set_audio("a.mp3", "A2", 1_000);
    start_scan(&fixture).run(fixture.root).expect("rescan");
    let updated = fixture.all_songs().into_iter().next().unwrap();
    assert_eq!(updated.id(), song.id(), "identity stable across rescans");
    assert_eq!(updated.title(), Some("A2"));
    assert_eq!(updated.added_at(), added_at, "added_at never changes");
    assert!(updated.revision() >= Revision::INITIAL);
}

// ------------------------------------------------------------------
// 13.4 — watcher/root matrix: every row converges after a manual rescan
// and relationships stay stable. Each test simulates the fs-level fault
// (unmount, permission revocation) with the fake's fault injection, then
// proves a subsequent scan does not silently trust stale data.
// ------------------------------------------------------------------

#[test]
fn root_unmount_then_remount_converges_after_manual_rescan() {
    // A root that vanishes (unmounted) must surface as a failed scan, then
    // a successful scan once remounted sees the current files — and never
    // marks songs missing from a failed run.
    let fixture = ScanFixture::new();
    fixture.write_file("a.mp3", b"audio");
    fixture.set_audio("a.mp3", "A", 1_000);
    start_scan(&fixture).run(fixture.root).expect("first scan");

    // Unmount: enumeration fails. The failed run must NOT mark missing.
    fixture
        .fs
        .inject_fault(Error::unavailable("library root", "unmounted"));
    let error = start_scan(&fixture).run(fixture.root).unwrap_err();
    assert_eq!(error.code(), "storage");
    let record = song_by_path(&fixture, &fixture.path("a.mp3"))
        .expect("lookup")
        .expect("kept from the failed run");
    assert_eq!(record.availability(), SongAvailability::Available);

    // Remount (clear the fault) + a modified file → the manual rescan
    // converges: the song is still there, identity stable.
    fixture.fs.clear_fault();
    fixture.write_file("a.mp3", b"audio-v2");
    fixture.set_audio("a.mp3", "A2", 1_000);
    let summary = start_scan(&fixture)
        .run(fixture.root)
        .expect("remounted scan");
    assert_eq!(
        summary.progress.updated, 1,
        "remounted scan re-parses the changed file by identity"
    );
    let after = song_by_path(&fixture, &fixture.path("a.mp3"))
        .expect("lookup")
        .expect("present after remount");
    assert_eq!(
        after.id(),
        record.id(),
        "UUID stable across unmount/remount"
    );
    assert_eq!(after.title(), Some("A2"));
}

#[test]
fn permission_revocation_mid_watch_degrades_to_rescan_and_converges() {
    // Permission revoked (reads fail) → scan fails without marking songs
    // missing; after re-permission, a manual rescan converges.
    let fixture = ScanFixture::new();
    fixture.write_file("a.mp3", b"audio");
    fixture.set_audio("a.mp3", "A", 1_000);
    start_scan(&fixture).run(fixture.root).expect("first scan");

    fixture.fs.inject_fault(Error::permission(
        "read directory",
        crate::error::PermKind::Denied,
    ));
    let error = start_scan(&fixture).run(fixture.root).unwrap_err();
    // The fake's fault is storage-shaped whatever variant is injected; the
    // contract under test is the failed run, not the error variant.
    assert_eq!(error.code(), "storage");
    // Read-only: scan is refused, the record stays available (no missing).
    let record = song_by_path(&fixture, &fixture.path("a.mp3"))
        .expect("lookup")
        .expect("kept under permission loss");
    assert_eq!(record.availability(), SongAvailability::Available);

    fixture.fs.clear_fault();
    let summary = start_scan(&fixture)
        .run(fixture.root)
        .expect("re-permission scan");
    assert_eq!(summary.progress.skipped, 1, "unchanged file fast-skipped");
    let after = song_by_path(&fixture, &fixture.path("a.mp3"))
        .expect("lookup")
        .expect("still present");
    assert_eq!(
        after.id(),
        record.id(),
        "identity stable across permission loss"
    );
}

#[test]
fn deep_unicode_paths_and_external_moves_converge_after_rescan() {
    // Unicode component + deep nesting: the scan indexes it, an external
    // rename keeps the UUID + relationships, and a manual rescan converges.
    let fixture = ScanFixture::new();
    // A deep Unicode path (multiple components + composed CJK/emoji).
    let deep = "歌手/黑胶 精选/🎧-单曲編號/曲目一-合成字.mp3";
    fixture.write_file(deep, b"deep-unicode-bytes");
    fixture.set_audio(deep, "深巷", 2_000);
    let summary = start_scan(&fixture)
        .run(fixture.root)
        .expect("unicode scan");
    assert_eq!(
        summary.progress.created, 1,
        "a deep unicode path is indexed as one song"
    );

    // Favorite goes through its dedicated mutation: a metadata upsert
    // never carries user state (the SQLite `DO UPDATE` list deliberately
    // omits `is_favorite`, `play_count` and `availability`).
    let original = song_by_path(&fixture, &fixture.path(deep))
        .expect("lookup")
        .expect("deep-path song");
    SongRepository::set_favorite(&fixture.database, original.id(), true).expect("favorite set");

    // External move (rename) on disk → same content, new path, same UUID.
    let moved = "歌手/黑胶 精选/新目录/曲目一-移动后.flac";
    fixture.write_file(moved, b"deep-unicode-bytes");
    fixture.remove_file(deep);
    fixture.set_audio(moved, "深巷", 2_000);
    let summary = start_scan(&fixture).run(fixture.root).expect("rename scan");
    assert_eq!(summary.progress.updated, 1, "external move re-links");
    let after = song_by_path(&fixture, &fixture.path(moved))
        .expect("lookup")
        .expect("re-linked at the new path");
    assert_eq!(after.id(), original.id(), "UUID survives the external move");
    assert!(after.favorite(), "favorite survives the external move");
    assert_eq!(
        after.availability(),
        SongAvailability::Available,
        "only the new path hosts the record; no duplicate"
    );
    assert!(
        fixture
            .all_songs()
            .iter()
            .filter(|s| s.id() == original.id())
            .count()
            == 1,
        "exactly one record for the moved song"
    );
}
