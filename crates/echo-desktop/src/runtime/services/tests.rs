use super::*;
use std::str::FromStr;

use echo_core::application::ports::LibraryRepository;
use echo_core::application::root_switch::derive_root_id;
use echo_core::application::scan::StartScan;
use echo_core::application::testing::scan_fixture::ScanFixture;
use echo_core::application::testing::small_fakes::FakeTrash;
use echo_core::domain::catalog::{SongSortField, SortDirection};
use echo_core::domain::entities::LibraryRoot;
use echo_core::domain::ids::{OperationId, PlaylistId};
use echo_core::domain::library::PortableSerialize;
use echo_core::domain::media::{AudioFormat, ParsedMetadata};

use crate::platform::dialogs::TestDialogs;

/// A writable `AppServices` over the in-memory fixture: an active, writable
/// root plus a startup gate resolved to `Writable`, using the default
/// (cancelling) dialogs port.
fn services(fixture: &ScanFixture) -> AppServices {
    services_with_dialogs(fixture, TestDialogs::cancelling())
}

/// Like [`services`](self::services) but with an explicit dialogs port.
fn services_with_dialogs(fixture: &ScanFixture, dialogs: TestDialogs) -> AppServices {
    LibraryRepository::upsert(
        &fixture.database,
        &LibraryRoot::new(fixture.root, "/library".into(), true, true),
    )
    .expect("active root");
    let startup = StartupSupervisor::new();
    startup
        .run_recovery(&fixture.deps, &fixture.supervisor, &FakeTrash::new())
        .expect("clean recovery");
    AppServices::with_runtime(
        std::sync::Arc::clone(&fixture.deps),
        ScanSupervisor::new(),
        std::sync::Arc::new(startup),
        std::sync::Arc::new(dialogs),
        RootRegistry::new(),
        std::sync::Arc::new(fixture.database.clone()),
        Blockers::new(),
    )
}

/// A `ScanFixture` bound to a *fresh* directory, for the root-choice tests.
/// The dialog returns `dir`; the services register it under its derivable
/// root id (in the filesystem registry) and prepare/activate it.
fn choose_root_fixture(dir: &std::path::Path) -> ScanFixture {
    let fixture = ScanFixture::new();
    // Bind the real directory in the fake filesystem so prepare/activate
    // can scan it. `prepare` itself creates the root record.
    let canonical = dir.canonicalize().expect("canonical dir");
    let root_id = derive_root_id(&canonical);
    fixture.fs.add_root_at(root_id, canonical);
    fixture
}

/// Seed `n` songs by writing files + scripting probe/metadata, then run a
/// full scan so they land in the committed library.
fn seed_songs(fixture: &ScanFixture, n: usize) -> Vec<SongId> {
    for index in 0..n {
        let path = format!("song-{index}.flac");
        fixture.write_file(&path, format!("audio-{index}").as_bytes());
        fixture.set_audio(&path, &format!("标题{index}"), 1_000);
    }
    StartScan::new(&fixture.deps, &fixture.supervisor)
        .run(fixture.root)
        .expect("scan seeds the library");
    fixture
        .all_songs()
        .iter()
        .map(echo_core::domain::entities::Song::id)
        .collect()
}

/// `cover_keys` answers with the embedded artwork of the songs that have
/// one, keyed by an opaque cache key — and stays silent about the rest
/// (design §115: a file with no embedded cover keeps the palette
/// placeholder, it does not get a fabricated asset).
#[test]
fn cover_keys_returns_only_songs_with_embedded_artwork() {
    let fixture = ScanFixture::new();
    fixture.write_file("with-art.flac", b"audio-with-art");
    fixture.set_audio_with_cover("with-art.flac", "有封面", 1_000, &b"cover-".repeat(64));
    fixture.write_file("no-art.flac", b"audio-no-art");
    fixture.set_audio("no-art.flac", "无封面", 1_000);
    StartScan::new(&fixture.deps, &fixture.supervisor)
        .run(fixture.root)
        .expect("scan seeds the library");
    let app = services(&fixture);

    let songs = fixture.all_songs();
    let with_art = songs
        .iter()
        .find(|song| song.title() == Some("有封面"))
        .expect("song with artwork");
    let without_art = songs
        .iter()
        .find(|song| song.title() == Some("无封面"))
        .expect("song without artwork");

    let keys = app
        .cover_keys(&[with_art.id(), without_art.id()])
        .expect("cover keys");

    assert_eq!(keys.len(), 1, "only the song that carries artwork is keyed");
    let key = keys
        .get(&with_art.id().to_string())
        .expect("the song with embedded artwork has a key");
    // Whatever the cache emits must be resolvable by the `cover://`
    // boundary the WebView will hand it to — and that boundary rejects
    // anything that could be read as a path.
    assert!(
        crate::platform::security::CoverProtocol::is_valid_key(key),
        "cover key must pass the cover:// whitelist: {key}"
    );
    assert!(!keys.contains_key(&without_art.id().to_string()));
}

/// A stale or unknown id costs nothing: the batch still answers for the
/// rows that are still valid, so one dead row cannot blank the window.
#[test]
fn cover_keys_skips_unknown_ids_instead_of_failing_the_batch() {
    let fixture = ScanFixture::new();
    fixture.write_file("with-art.flac", b"audio-with-art");
    fixture.set_audio_with_cover("with-art.flac", "有封面", 1_000, &b"cover-".repeat(64));
    StartScan::new(&fixture.deps, &fixture.supervisor)
        .run(fixture.root)
        .expect("scan seeds the library");
    let app = services(&fixture);

    let known = fixture.all_songs()[0].id();
    let keys = app
        .cover_keys(&[known, SongId::new()])
        .expect("an unknown id is skipped, not fatal");

    assert_eq!(keys.len(), 1);
    assert!(keys.contains_key(&known.to_string()));
}

#[test]
fn playlist_playback_context_resolves_the_full_member_set() {
    // 视图播放重建队列数量 for playlists: the desktop resolves every
    // member (newest first, matching the playlist view), exactly once per
    // song, regardless of any paging the list UI may have done.
    let fixture = ScanFixture::new();
    let ids = seed_songs(&fixture, 4);
    let app = services(&fixture);
    let playlist = PlaylistId::from_str(
        &app.create_playlist(fixture.root, "播放歌单")
            .expect("create"),
    )
    .expect("playlist id");
    // Add in 0,1,2 order → view order (newest first) is 2,1,0.
    for id in [ids[0], ids[1], ids[2]] {
        app.add_to_playlists(id, &[playlist]).expect("add member");
    }

    let resolved = app
        .resolve_playlist_playback_context(playlist, ids[1])
        .expect("resolve");
    assert_eq!(
        resolved,
        vec![ids[2], ids[1], ids[0]],
        "the queue is the full member set newest-first, selected song present"
    );

    // A non-member selection is a conflict, never a silent partial queue.
    let err = app
        .resolve_playlist_playback_context(playlist, ids[3])
        .expect_err("selection not a member");
    assert!(
        matches!(err, Error::Conflict { .. }),
        "an out-of-set selection must be refused"
    );
}

#[test]
fn playlist_playback_context_skips_deleted_members_not_truncating_others() {
    // A deleted song is hidden from the playlist view, so the resolved
    // queue drops only that song — the remaining members still form the
    // full, deterministic queue (no partial/paged truncation).
    let fixture = ScanFixture::new();
    let ids = seed_songs(&fixture, 3);
    let app = services(&fixture);
    let playlist =
        PlaylistId::from_str(&app.create_playlist(fixture.root, "删除后").expect("create"))
            .expect("playlist id");
    let target = ids[0];
    for &id in &ids {
        app.add_to_playlists(id, &[playlist]).expect("add member");
    }
    app.delete_song(fixture.root, target)
        .expect("delete member");

    let resolved = app
        .resolve_playlist_playback_context(playlist, ids[1])
        .expect("resolve");
    assert_eq!(
        resolved,
        vec![ids[2], ids[1]],
        "the deleted member is skipped, the rest survive in order"
    );
}

#[test]
fn library_playback_context_resolves_the_full_all_view() {
    // 视图播放重建队列数量 for the 全部歌曲 view: the desktop resolves
    // every available song of the active root — reading through pages until
    // `is_last` — so no paged/partial client list can shorten the queue.
    let fixture = ScanFixture::new();
    let ids = seed_songs(&fixture, 6);
    let app = services(&fixture);
    let sort = SongSort {
        field: SongSortField::AddedAt,
        direction: SortDirection::Desc,
    };
    let resolved = app
        .resolve_library_playback_context("all", "", sort, ids[2])
        .expect("resolve the all view");
    assert_eq!(
        resolved.len(),
        ids.len(),
        "every song of the view is a queue member, never a first-page slice"
    );
    assert!(
        resolved.contains(&ids[2]),
        "the selected song is a member of the resolved queue"
    );
}

#[test]
fn library_playback_context_rejects_selection_outside_the_all_view() {
    // An out-of-view selection is a conflict, never a silent partial queue:
    // a stale/foreign selected id must not assemble a queue it is absent from.
    let fixture = ScanFixture::new();
    let ids = seed_songs(&fixture, 3);
    let app = services(&fixture);
    let sort = SongSort {
        field: SongSortField::AddedAt,
        direction: SortDirection::Desc,
    };
    let err = app
        .resolve_library_playback_context("all", "", sort, SongId::new())
        .expect_err("foreign selection");
    assert!(
        matches!(err, Error::Conflict { .. }),
        "a selection outside the view is refused"
    );
    let _ = ids;
}

#[test]
fn library_playback_context_resolves_only_favorites() {
    // 喜欢的音乐 view: the queue is exactly the favorited member set — no
    // non-favorite leaks in, and favorites that page beyond one window are
    // all present.
    let fixture = ScanFixture::new();
    let ids = seed_songs(&fixture, 5);
    let app = services(&fixture);
    for fav in [ids[0], ids[2], ids[4]] {
        app.set_favorite(fav, true).expect("favorite");
    }
    let sort = SongSort {
        field: SongSortField::Title,
        direction: SortDirection::Asc,
    };
    let resolved = app
        .resolve_library_playback_context("favorites", "", sort, ids[2])
        .expect("resolve the favorites view");
    let mut expected = vec![ids[0], ids[2], ids[4]];
    expected.sort();
    let mut got = resolved;
    got.sort();
    assert_eq!(
        got, expected,
        "only favorited songs form the queue, all of them"
    );
}

#[test]
fn library_playback_context_resolves_search_within_the_view() {
    // 资料库搜索: the desktop applies the same query the list shows, so the
    // queue is the matching subset — never a partial page of it. The
    // selected song must appear in the result for the command to accept it.
    let fixture = ScanFixture::new();
    let ids = seed_songs(&fixture, 4);
    let app = services(&fixture);
    // All four titles contain "标题" so this query matches the full set;
    // a query string shorter than three Unicode scalars uses LIKE, which
    // is a reliable substring match for CJK text in SQLite.
    let sort = SongSort {
        field: SongSortField::Title,
        direction: SortDirection::Asc,
    };
    let resolved = app
        .resolve_library_playback_context("all", "标题", sort, ids[2])
        .expect("resolve the search subset");
    assert_eq!(
        resolved.len(),
        ids.len(),
        "every title-matching song is a queue member — not a page slice"
    );
    assert!(
        resolved.contains(&ids[2]),
        "the selected song is a member of the resolved queue"
    );
}

#[test]
fn playlist_cover_is_empty_then_follows_latest_member_unless_manually_set() {
    let fixture = ScanFixture::new();
    fixture.write_file("older.flac", b"older-audio");
    fixture.write_file("newer.flac", b"newer-audio");
    fixture.write_file("no-art.flac", b"no-art-audio");
    fixture.set_audio_with_cover("older.flac", "旧封面", 1_000, b"older-cover");
    fixture.set_audio_with_cover("newer.flac", "新封面", 1_000, b"newer-cover");
    fixture.set_audio("no-art.flac", "无封面", 1_000);
    StartScan::new(&fixture.deps, &fixture.supervisor)
        .run(fixture.root)
        .expect("scan");
    let app = services(&fixture);
    let playlist = app
        .create_playlist(fixture.root, "测试歌单")
        .expect("create");
    let playlist = PlaylistId::from_str(&playlist).expect("playlist id");

    assert!(
        app.playlists().expect("list")[0].cover_key.is_none(),
        "new playlist is blank"
    );

    let songs = fixture.all_songs();
    let older = songs
        .iter()
        .find(|song| song.title() == Some("旧封面"))
        .expect("older");
    let newer = songs
        .iter()
        .find(|song| song.title() == Some("新封面"))
        .expect("newer");
    let no_art = songs
        .iter()
        .find(|song| song.title() == Some("无封面"))
        .expect("no-art");
    app.add_to_playlists(older.id(), &[playlist])
        .expect("add older");
    app.add_to_playlists(newer.id(), &[playlist])
        .expect("add newer");
    let latest_key =
        app.cover_keys(&[newer.id()]).expect("cover key")[&newer.id().to_string()].clone();
    assert_eq!(
        app.playlists().expect("list")[0].cover_key.as_deref(),
        Some(latest_key.as_str())
    );
    app.add_to_playlists(no_art.id(), &[playlist])
        .expect("add song without art");
    assert_eq!(
        app.playlists().expect("list")[0].cover_key.as_deref(),
        Some(latest_key.as_str()),
        "a later song without artwork leaves the prior automatic cover intact"
    );

    app.set_playlist_cover(playlist, Some(b"manual-cover".to_vec()), Some("image/png"))
        .expect("set manual cover");
    let manual_key = app.playlists().expect("list")[0]
        .cover_key
        .clone()
        .expect("manual key");
    assert_ne!(
        manual_key, latest_key,
        "manual choice wins over automatic artwork"
    );

    app.set_playlist_cover(playlist, None, None)
        .expect("restore automatic");
    assert_eq!(
        app.playlists().expect("list")[0].cover_key.as_deref(),
        Some(latest_key.as_str())
    );
}

#[test]
fn set_favorite_returns_the_committed_authoritative_song() {
    let fixture = ScanFixture::new();
    let ids = seed_songs(&fixture, 1);
    let app = services(&fixture);

    let view = app
        .set_favorite(ids[0], true)
        .expect("favourite returns committed snapshot");
    assert!(view.favorite, "returned view reflects the committed write");

    // The same SongId drives the favorites view: it must now appear.
    let favs = app.favorites(SongSort::default(), None, 100).expect("favs");
    assert_eq!(favs.items.len(), 1);
    assert_eq!(favs.items[0].id, ids[0].to_string());
}

#[test]
fn favorite_commit_materializes_a_portable_record_without_local_state() {
    let fixture = ScanFixture::new();
    let ids = seed_songs(&fixture, 1);
    let app = services(&fixture);

    app.set_favorite(ids[0], true).expect("favourite");

    let record = fixture
        .deps
        .control
        .read_record(
            fixture.root,
            echo_core::domain::library::RecordKind::Favorite,
            &ids[0].to_string(),
        )
        .expect("read record")
        .expect("favorite materialized");
    let json = record.to_canonical_json().expect("portable json");
    assert!(json.contains("is_favorite"));
    for forbidden in ["/library", "sqlite", "credential", "play_count"] {
        assert!(
            !json.contains(forbidden),
            "portable favorite must not expose {forbidden}"
        );
    }
}

/// PLL-R02-S06: un-favouriting is a *state*, not a deletion. The record must
/// survive as `is_favorite: false` so reopening the library never resurrects
/// the song into 「我的喜欢」.
#[test]
fn unfavorite_materializes_a_false_record_so_it_never_returns() {
    let fixture = ScanFixture::new();
    let ids = seed_songs(&fixture, 1);
    let app = services(&fixture);

    app.set_favorite(ids[0], true).expect("favourite");
    app.set_favorite(ids[0], false).expect("unfavourite");

    let record = fixture
        .deps
        .control
        .read_record(
            fixture.root,
            echo_core::domain::library::RecordKind::Favorite,
            &ids[0].to_string(),
        )
        .expect("read record")
        .expect("the unfavorite overwrote the record instead of dropping it");
    let json = record.to_canonical_json().expect("portable json");
    assert!(
        json.contains("\"is_favorite\":false"),
        "the portable record must carry the cancelled state: {json}"
    );

    // And the local view agrees — the song is gone from 「我的喜欢」.
    let favs = app.favorites(SongSort::default(), None, 100).expect("favs");
    assert!(
        favs.items.is_empty(),
        "a cancelled favorite must not reappear in the favorites view"
    );
}

#[test]
fn playlist_mutations_return_snapshots_seen_by_subsequent_reads() {
    let fixture = ScanFixture::new();
    let ids = seed_songs(&fixture, 2);
    let app = services(&fixture);

    let playlist = app
        .create_playlist(fixture.root, "我的歌单")
        .expect("create");
    let id = PlaylistId::from_str(&playlist).expect("valid playlist id");

    // Add both songs; the list and members reflect them committed.
    app.add_to_playlists(ids[0], &[id]).expect("add 1");
    app.add_to_playlists(ids[1], &[id]).expect("add 2");
    let members = app.playlist_members(id).expect("members");
    assert_eq!(members.len(), 2, "both members are committed");
    assert_eq!(
        members[0].id,
        ids[1].to_string(),
        "the last-added song is the first playlist row"
    );

    let views = app.playlists().expect("list");
    assert_eq!(views.len(), 1);
    assert_eq!(views[0].member_count, 2);

    // Removing a member is reflected by the next read; a non-member is a
    // no-op success.
    app.remove_playlist_song(id, ids[0]).expect("remove");
    assert_eq!(app.playlist_members(id).expect("after").len(), 1);
    app.remove_playlist_song(id, ids[0]).expect("no-op"); // already gone
}

#[test]
fn delete_and_undo_roundtrip_restores_the_song() {
    let fixture = ScanFixture::new();
    let ids = seed_songs(&fixture, 2);
    let app = services(&fixture);

    let operation = app
        .delete_song(fixture.root, ids[0])
        .expect("delete returns undo op");
    // The song is hidden from the catalog (pending-delete) right away.
    let gone = app.all_songs(SongSort::default(), None, 100).expect("all");
    assert!(
        gone.items.iter().all(|s| s.id != ids[0].to_string()),
        "pending-delete song hides from the catalog"
    );

    let restored = app
        .undo_delete(fixture.root, OperationId::from_str(&operation).expect("op"))
        .expect("undo restores");
    assert_eq!(restored, ids[0].to_string());
    let back = app
        .all_songs(SongSort::default(), None, 100)
        .expect("after");
    assert!(back.items.iter().any(|s| s.id == ids[0].to_string()));
}

/// The sidebar playlist count must drop within the delete undo window even
/// though the membership row survives (finalization only cascades it away via
/// `delete_song`). The count is a *live* projection of the members' song
/// availability, exactly like the playlist view's `playlist_songs_query`, so a
/// just-deleted song stops counting immediately and undo restores the count.
#[test]
fn delete_then_undo_reflects_playlist_member_count_within_undo_window() {
    let fixture = ScanFixture::new();
    let ids = seed_songs(&fixture, 2);
    let app = services(&fixture);

    let playlist = app
        .create_playlist(fixture.root, "删除计数")
        .expect("create");
    let id = PlaylistId::from_str(&playlist).expect("valid playlist id");
    app.add_to_playlists(ids[0], &[id])
        .expect("add to playlist");
    app.add_to_playlists(ids[1], &[id])
        .expect("add second song");

    let count = |app: &AppServices| app.playlists().expect("list")[0].member_count;
    assert_eq!(count(&app), 2, "both members counted before delete");

    let operation = app
        .delete_song(fixture.root, ids[0])
        .expect("delete returns undo op");
    assert_eq!(
        count(&app),
        1,
        "pending-delete member must not count within the undo window"
    );

    app.undo_delete(fixture.root, OperationId::from_str(&operation).expect("op"))
        .expect("undo restores");
    assert_eq!(count(&app), 2, "undo restores the playlist count");
}

#[test]
fn library_status_reflects_configuration_and_scan_in_flight() {
    let fixture = ScanFixture::new();
    let app = services(&fixture);
    // Active writable root.
    let status = app.library_status().expect("status");
    assert!(status.configured);
    assert!(!status.read_only);
    assert!(!status.unavailable);
    assert_eq!(
        status.active_root.as_deref(),
        Some(fixture.root.to_string().as_str())
    );
}

#[test]
fn library_status_marks_an_unresolved_write_gate_as_read_only() {
    let fixture = ScanFixture::new();
    LibraryRepository::upsert(
        &fixture.database,
        &LibraryRoot::new(fixture.root, "/library".into(), true, true),
    )
    .expect("active root");
    let app = AppServices::new(
        std::sync::Arc::clone(&fixture.deps),
        ScanSupervisor::new(),
        StartupSupervisor::new(),
    );

    let status = app.library_status().expect("status");

    assert!(status.configured);
    assert!(
        status.read_only,
        "the UI must match the mutation write gate"
    );
    assert!(!status.unavailable);
}

#[test]
fn cancelled_library_root_dialog_is_a_noop_never_a_success() {
    let fixture = ScanFixture::new();
    // Cancelling dialogs port; writes are allowed, so a real switch *could*
    // run — the cancel must still map to `None`, never an empty/fake root.
    let app = services(&fixture);
    assert!(app
        .choose_library_root()
        .expect("a cancelled dialog is not an error")
        .is_none());
}

#[test]
fn choose_library_root_prepares_and_activates_a_directory() {
    let dir = tempfile::tempdir().expect("temp dir");
    let fixture = choose_root_fixture(dir.path());
    let dialogs = TestDialogs::with_directory(Some(dir.path().to_path_buf()));
    let app = services_with_dialogs(&fixture, dialogs);

    let maybe = app.choose_library_root().expect("choose");
    assert!(maybe.is_some(), "a confirmed directory is a success");
    let status = maybe.expect("some");
    assert!(status.configured);
    assert_ne!(status.active_root, fixture.root.to_string());
    // The old default root is not active; the chosen one is.
    let active = fixture
        .database
        .active_root()
        .expect("read")
        .expect("now configured");
    assert_ne!(
        active.id(),
        fixture.root,
        "the picked root supersedes the default"
    );
    assert_eq!(active.id().to_string(), status.active_root);
}

#[test]
fn choose_library_root_cancelled_preserves_previous_configuration() {
    let fixture = ScanFixture::new();
    let app = services(&fixture);
    // A cancelled dialog: old configuration intact.
    let maybe = app.choose_library_root().expect("cancel");
    assert!(maybe.is_none());
    let status = app.library_status().expect("status");
    assert!(status.configured, "previous active root preserved");
}

#[test]
fn choose_library_root_is_refused_when_fs_is_unavailable() {
    let dir = tempfile::tempdir().expect("temp dir");
    // Simulate an unreadable directory (prepare fails, no activation). The fake
    // reproduces any injected fault as `Storage`; the contract is that a
    // root-level failure is surfaced and nothing activates — the candidate
    // record stays but is never the active root.
    let fixture = choose_root_fixture(dir.path());
    fixture
        .fs
        .inject_fault(Error::unavailable("library root", "unreadable"));
    let dialogs = TestDialogs::with_directory(Some(dir.path().to_path_buf()));
    // No pre-registered active root: a clean first-launch state.
    let startup = StartupSupervisor::new();
    let app = AppServices::with_runtime(
        std::sync::Arc::clone(&fixture.deps),
        ScanSupervisor::new(),
        std::sync::Arc::new(startup),
        std::sync::Arc::new(dialogs),
        RootRegistry::new(),
        std::sync::Arc::new(fixture.database.clone()),
        Blockers::new(),
    );
    let err = app.choose_library_root().expect_err("prepare rejects");
    assert!(matches!(err, Error::Storage { .. }));
    let status = app.library_status().expect("status");
    assert_eq!(
        status.active_root, None,
        "a failed prepare never activates a root"
    );
}

#[test]
fn destructive_commands_obey_the_write_gate() {
    // A gate-left-unresolved (no recovery yet) must refuse writes: the
    // coarse commands are thin but never bypass the readiness gate.
    let fixture = ScanFixture::new();
    let ids = seed_songs(&fixture, 1);
    let startup = StartupSupervisor::new(); // gate: None → writes disabled
    let app = AppServices::new(
        std::sync::Arc::clone(&fixture.deps),
        ScanSupervisor::new(),
        startup,
    );

    let err = app.set_favorite(ids[0], true).expect_err("writes disabled");
    assert!(
        matches!(err, Error::Unavailable { .. }),
        "the gate refuses writes before recovery resolves"
    );
    assert!(
        !app.cancel_scan(fixture.root),
        "no scan in flight on a fresh supervisor"
    );
}

#[test]
fn cancelled_import_dialog_is_a_noop_never_a_success() {
    let fixture = ScanFixture::new();
    // `services()` resolves the gate to Writable and keeps the default
    // dialogs port (always cancel). Writes are allowed, so a real import
    // *could* run — the cancel must still map to `None`, never a fake
    // empty or successful batch.
    let app = services(&fixture);
    assert!(app
        .choose_and_import_files()
        .expect("a cancelled dialog is not an error")
        .is_none());
}

#[test]
fn import_dialog_is_refused_before_the_write_gate_resolves() {
    let fixture = ScanFixture::new();
    let app = AppServices::new(
        std::sync::Arc::clone(&fixture.deps),
        ScanSupervisor::new(),
        StartupSupervisor::new(), // gate unresolved -> writes disabled
    );
    let err = app
        .choose_and_import_files()
        .expect_err("write gate blocks import before recovery");
    assert!(matches!(err, Error::Unavailable { .. }));
}

#[test]
fn reveal_song_reveals_by_id_and_returns_only_the_relative_path() {
    let fixture = ScanFixture::new();
    let ids = seed_songs(&fixture, 1);
    let dialogs = TestDialogs::cancelling();
    let app = services_with_dialogs(&fixture, dialogs);

    let view = app.reveal_song(ids[0]).expect("reveal");
    // Never an absolute path: only the library-relative path reaches the UI.
    assert!(!view.relative_path.starts_with('/'));
    assert!(view.revealed);
    assert_eq!(view.song_id, ids[0].to_string());
}

/// Every committed playlist/membership change must reach `echo/records/`,
/// otherwise the next "open this directory" (on a wiped app-data directory or
/// another machine) cannot rebuild it — the defect behind issue #1. Deletes are
/// materialized as tombstones, never as a silent disappearance.
#[test]
fn playlist_mutations_materialize_records_and_tombstones() {
    let fixture = ScanFixture::new();
    let ids = seed_songs(&fixture, 2);
    let app = services(&fixture);

    let playlist = PlaylistId::from_str(
        &app.create_playlist(fixture.root, "通勤路上")
            .expect("create"),
    )
    .expect("playlist id");
    app.add_to_playlists(ids[0], &[playlist])
        .expect("add member");

    let kinds = |records: &[echo_core::domain::library::PortableRecord]| {
        let mut set: Vec<echo_core::domain::library::RecordKind> =
            records.iter().map(PortableRecord::kind).collect();
        set.sort_by_key(|kind| kind.dir_name());
        set
    };
    let records = fixture.control.records_of(fixture.root);
    assert!(
        kinds(&records).contains(&echo_core::domain::library::RecordKind::Playlist),
        "the playlist is materialized: {records:?}"
    );
    assert!(
        kinds(&records).contains(&echo_core::domain::library::RecordKind::PlaylistItem),
        "the membership is materialized with its own identity: {records:?}"
    );
    // The member record carries the *member* UUID, so a remove can be
    // tombstoned without touching the playlist or the song.
    let item = records
        .iter()
        .find(|record| record.kind() == echo_core::domain::library::RecordKind::PlaylistItem)
        .expect("item record")
        .clone();

    app.remove_playlist_song(playlist, ids[0])
        .expect("remove member");
    let records = fixture.control.records_of(fixture.root);
    let tombstones: Vec<_> = records
        .iter()
        .filter(|record| record.is_tombstone())
        .collect();
    assert_eq!(tombstones.len(), 1, "one tombstone per removed member");
    assert_eq!(
        tombstones[0].object_uuid(),
        item.object_uuid(),
        "the tombstone names the member UUID"
    );

    app.delete_playlist(playlist).expect("delete playlist");
    let records = fixture.control.records_of(fixture.root);
    assert!(
        records
            .iter()
            .filter(|record| record.is_tombstone())
            .any(|record| record.object_uuid() == playlist.as_uuid()),
        "a deleted playlist is tombstoned, so it cannot be resurrected"
    );
}

/// Play statistics are materialized per device so two devices merge additively
/// (design D4) and a rebuild restores the merged count.
#[test]
fn recorded_plays_materialize_additive_play_stats() {
    let fixture = ScanFixture::new();
    let ids = seed_songs(&fixture, 1);
    let device = fixture.deps.device_id.current_device_id();
    let song = ids[0];

    let record = |count: u64| echo_core::domain::library::PlayStatsRecord {
        song_uuid: song.as_uuid(),
        revision: echo_core::domain::ids::Revision::INITIAL,
        updated_by_device_id: device,
        hlc: echo_core::domain::library::HybridLogicalClock::default(),
        by_device: std::collections::BTreeMap::from([(device.as_uuid(), count)]),
    };
    let other_device = echo_core::domain::library::DeviceId::new();

    // Two devices each play once, and this device plays a second time.
    let merged = echo_core::application::portable_materialize::merge_play_stats(
        None,
        device,
        echo_core::domain::library::HybridLogicalClock::default(),
        song,
    );
    assert_eq!(merged.total(), 1);
    let merged = echo_core::application::portable_materialize::merge_play_stats(
        Some(record(1)),
        other_device,
        echo_core::domain::library::HybridLogicalClock::default(),
        song,
    );
    assert_eq!(
        merged.total(),
        2,
        "a second device's play adds instead of overwriting"
    );
    let merged = echo_core::application::portable_materialize::merge_play_stats(
        Some(record(1)),
        device,
        echo_core::domain::library::HybridLogicalClock::default(),
        song,
    );
    assert_eq!(merged.total(), 2, "this device's own second play counts");
}

use echo_core::domain::library::PortableRecord;

// ==== system file-open dispatch (SFI-R06, normalize-os-file-open-paths) ====
//
// The shell hands a decoded absolute path to `play_temporary_file`; its first
// move is `active_song_for_path`, whose answer decides between "play the
// existing library identity" and "create a session temporary item". These
// tests pin the predicate the dispatch depends on — the coordinator's
// temporary-item behavior itself is covered by
// `coordinator::tests::play_temporary_is_session_only`.

#[test]
fn open_path_outside_the_active_library_is_not_a_library_uuid() {
    // SFI-R06-S01: a system-opened file that matches no library record must
    // resolve to `None`, so the caller creates a temporary item instead of
    // borrowing a wrong identity.
    let f = ScanFixture::new();
    let svc = services(&f);

    let resolved = svc
        .active_song_for_path(std::path::Path::new("/tmp/We Will Rock You - Queen.flac"))
        .expect("read");

    assert_eq!(
        resolved, None,
        "a non-library file must not resolve to a song"
    );
}

#[test]
fn open_path_of_an_active_library_song_resolves_to_its_uuid() {
    // SFI-R06-S02: a path inside the active root that matches an existing
    // record resolves to that song's UUID — the caller plays the existing
    // identity (its overrides, favorites, stats) instead of a temporary item.
    let f = ScanFixture::new();
    f.write_file("a.flac", b"audio");
    f.set_audio("a.flac", "标题", 1_000);
    StartScan::new(&f.deps, &f.supervisor)
        .run(f.root)
        .expect("scan");
    let svc = services(&f);

    let root = f
        .deps
        .roots
        .active_root()
        .expect("root query")
        .expect("active root");
    let song = &f.deps.songs.all_in_root(f.root).expect("songs")[0];
    let opened = root.resolve_song_path(song).expect("resolve");

    let resolved = svc.active_song_for_path(&opened).expect("read");
    assert_eq!(
        resolved,
        Some(song.id()),
        "an in-library file must resolve to its recorded UUID"
    );
}

#[test]
fn open_path_of_a_non_active_root_never_resolves_to_the_old_uuid() {
    // SFI-R06-S03: a file under a retained-but-inactive root must not resolve
    // to the old root's song identity — the active-root isolation is not
    // bypassed by a UUID, so the file plays as a session temporary item.
    use echo_core::domain::entities::Song;
    use echo_core::domain::ids::{LibraryRootId, RelativeMediaPath, Revision, SongId};

    let f = ScanFixture::new();
    let old_root = LibraryRootId::new();
    LibraryRepository::upsert(
        &f.database,
        &LibraryRoot::new(old_root, "/old-library".into(), false, true),
    )
    .expect("old root");
    let old_song = Song::new(
        SongId::new(),
        old_root,
        RelativeMediaPath::new("media/old.flac").unwrap(),
        Revision::INITIAL,
    );
    f.deps.songs.upsert(&old_song).expect("old song");
    let svc = services(&f);

    let resolved = svc
        .active_song_for_path(std::path::Path::new("/old-library/media/old.flac"))
        .expect("read");

    assert_eq!(
        resolved, None,
        "an old-root file must never play through the old root's UUID"
    );
}

/// D3 of deliver-file-opens-after-frontend-ready: there is exactly one startup
/// supervisor. The shell registers an `Arc<StartupSupervisor>` (so the OS
/// file-open handler can consult it before `AppServices` exists) and hands the
/// same `Arc` to the composition root — a second, independently-constructed
/// instance would let the shell's file-open delivery gate and this root's
/// readiness gate diverge, and the divergence would be invisible: both would
/// answer, just differently.
#[test]
fn the_composition_root_shares_the_shells_startup_supervisor() {
    let fixture = ScanFixture::new();
    let shell = std::sync::Arc::new(StartupSupervisor::new());
    let app = AppServices::with_runtime(
        std::sync::Arc::clone(&fixture.deps),
        ScanSupervisor::new(),
        std::sync::Arc::clone(&shell),
        std::sync::Arc::new(TestDialogs::cancelling()),
        RootRegistry::new(),
        std::sync::Arc::new(fixture.database.clone()),
        Blockers::new(),
    );

    assert!(
        std::ptr::eq(app.startup(), &*shell),
        "the composition root must expose the shell's supervisor, not a copy"
    );

    // Behavior across the two handles, not just pointer identity: a path queued
    // through the shell's handle is drained through the root's handle, and the
    // root sees the phase the shell set.
    shell.on_ready();
    assert_eq!(app.startup().phase(), crate::runtime::StartupPhase::Ready);
    assert_eq!(
        shell.receive_file_open(std::path::PathBuf::from("/music/shared.flac")),
        None,
        "queued: the frontend listener has not registered yet"
    );
    assert_eq!(
        app.startup().mark_frontend_ready(),
        vec![std::path::PathBuf::from("/music/shared.flac")]
    );
    assert_eq!(
        app.startup()
            .receive_file_open(std::path::PathBuf::from("/music/after.flac")),
        Some(std::path::PathBuf::from("/music/after.flac"))
    );
}

/// A file opened from the file browser is read for its presentation metadata
/// before it becomes a queue item. Duration is probe-owned — the tag reader
/// deliberately never fills it — and a file outside the library gets no scan
/// pass, so this read is a temporary item's only duration source. Without it
/// the queue row read 时长未知 while the progress bar, fed by the engine,
/// already showed the real length.
#[test]
fn temporary_metadata_duration_comes_from_the_probe() {
    let fixture = ScanFixture::new();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("西楼别序.mp3");
    let content: &[u8] = b"outside-the-library-bytes";
    std::fs::write(&path, content).unwrap();

    // The seeded tags carry only the display fields: the real tag reader
    // returns `duration: None`, so seeding one would test a situation that
    // cannot happen.
    fixture.metadata.set_bytes(
        content,
        ParsedMetadata {
            title: Some("西楼别序".to_owned()),
            artist: Some("尹昔眠".to_owned()),
            ..ParsedMetadata::default()
        },
    );
    fixture.probe.set_bytes(
        content,
        ProbeOutcome::Audio {
            format: AudioFormat::Mpeg,
            duration: Some(std::time::Duration::from_secs(227)),
        },
    );

    let metadata = services(&fixture).read_temporary_metadata(&path);

    assert_eq!(metadata.title.as_deref(), Some("西楼别序"));
    assert_eq!(metadata.artist.as_deref(), Some("尹昔眠"));
    assert_eq!(
        metadata.duration,
        Some(227.0),
        "the probe is a temporary item's only duration source"
    );
}

/// The other half of the contract: content the probe cannot classify keeps an
/// unknown duration unknown. A row showing a made-up length would be worse
/// than one admitting it does not know.
#[test]
fn temporary_metadata_keeps_an_unsupported_duration_unknown() {
    let fixture = ScanFixture::new();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mystery.mp3");
    let content: &[u8] = b"not-media-at-all";
    std::fs::write(&path, content).unwrap();
    fixture.probe.set_bytes(content, ProbeOutcome::Unsupported);

    let metadata = services(&fixture).read_temporary_metadata(&path);

    assert_eq!(metadata.duration, None);
}
