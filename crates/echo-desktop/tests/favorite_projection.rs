//! 收藏的读模型投影契约 (issue #152).
//!
//! 收藏一次点击会跨过两个存储层：目录（`echo/` 权威库）同步提交，SQLite
//! 读模型投影。收藏是用户**单次点击即可感知**的写入，因此读模型必须在返回
//! 已保存之前完成投影 —— 否则返回后立即回读仍是旧值，前端的权威回读会把
//! 刚点亮的心形冲掉，用户必须再点一次（issue #152）。
//!
//! 这些用例刻意跑在**生产 SQLite 装配**上（`assemble()`）。既有服务层用例
//! 跑在 `MemoryDatabase` 上，而 `cfg!(test)` 对 echo-core 这个依赖 crate
//! 为假时才取 `requested_async_projection` 的真值 —— 只有真实装配才会暴露
//! 这个窗口。

use echo_core::application::ports::LibraryProjectionPort;
use echo_core::domain::catalog::{SongSort, SongSortField, SortDirection};
use echo_core::domain::ids::{LibraryRootId, SongId};
use echo_desktop::platform::dialogs::{RevealOutcome, SystemDialogs};
use echo_desktop::runtime::app::assemble;
use echo_desktop::runtime::services::AppServices;
use echo_desktop::runtime::StartupSupervisor;
use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;

const SORT: SongSort = SongSort {
    field: SongSortField::AddedAt,
    direction: SortDirection::Asc,
};

/// A dialog boundary that only answers the root pick; the fixture's audio is
/// already on disk below the chosen root, so no import picker is needed.
struct RootDialog {
    root: std::sync::Mutex<Option<std::path::PathBuf>>,
}

impl SystemDialogs for RootDialog {
    fn pick_library_directory(
        &self,
    ) -> Result<Option<std::path::PathBuf>, echo_core::error::Error> {
        Ok(self
            .root
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take())
    }
    fn pick_audio_files(
        &self,
    ) -> Result<Option<echo_desktop::platform::dialogs::PickedImport>, echo_core::error::Error>
    {
        Ok(None)
    }
    fn reveal(&self, _: &std::path::Path) -> Result<RevealOutcome, echo_core::error::Error> {
        Ok(RevealOutcome::Revealed)
    }
}

/// Copy the repository's fixed audio fixtures into the library root.
fn seed_media(root_dir: &Path) {
    let media = root_dir.join("media");
    std::fs::create_dir_all(&media).expect("media root");
    let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root (crates/echo-desktop/../../)");
    for (from, to) in [
        ("fixtures/audio/tone-short.mp3", "tone-a.mp3"),
        ("fixtures/audio/tone-short.flac", "tone-b.flac"),
    ] {
        std::fs::copy(repo.join(from), media.join(to)).expect("copy real fixture");
    }
}

/// A library with two scanned songs, plus their ids.
struct Library {
    /// The temp dirs own the library files; they must outlive the services.
    _data_dir: tempfile::TempDir,
    _root_dir: tempfile::TempDir,
    app: AppServices,
    database: Arc<echo_core::infrastructure::sqlite::SqliteDatabase>,
    root: LibraryRootId,
    songs: Vec<String>,
}

fn library() -> Library {
    let data_dir = tempfile::tempdir().expect("data dir");
    let root_dir = tempfile::tempdir().expect("root dir");
    seed_media(root_dir.path());

    let routed = assemble(
        &data_dir.path().join("echo.sqlite"),
        &data_dir.path().join("covers"),
    )
    .expect("assemble real SQLite + root registry");
    let database = routed.database.clone();
    let startup = StartupSupervisor::new();
    let scan_supervisor = echo_core::application::scan::ScanSupervisor::new();
    let trash = echo_desktop::platform::trash::DesktopTrash::default();
    startup
        .run_recovery(&routed.deps, &scan_supervisor, &trash)
        .expect("boot recovery resolves the gate");
    startup.on_ready();
    startup.mark_frontend_ready();
    let app = AppServices::with_runtime(
        routed.deps.clone(),
        scan_supervisor,
        Arc::new(startup),
        Arc::new(RootDialog {
            root: std::sync::Mutex::new(Some(root_dir.path().to_path_buf())),
        }),
        routed.registry.clone(),
        routed.database,
        echo_core::application::root_switch::Blockers::new(),
    );

    let status = app
        .choose_library_root()
        .expect("pick+activate root")
        .expect("configured");
    let root = LibraryRootId::from_str(&status.active_root).expect("root id");

    let snapshot = app.start_scan(root).expect("scan converges once");
    assert!(snapshot.processed >= 2, "at least the seeded songs");

    let songs: Vec<String> = app
        .all_songs(SORT, None, 16)
        .expect("all songs")
        .items
        .into_iter()
        .map(|song| song.id)
        .collect();
    assert!(songs.len() >= 2, "the scan produced songs to favorite");

    Library {
        _data_dir: data_dir,
        _root_dir: root_dir,
        app,
        database,
        root,
        songs,
    }
}

/// The song's `favorite` flag as the three read paths see it right now.
struct ReadBack {
    row: bool,
    in_favorites_view: bool,
    count: u64,
}

fn read_back(app: &AppServices, song_id: &str) -> ReadBack {
    let row = app
        .all_songs(SORT, None, 16)
        .expect("all songs")
        .items
        .into_iter()
        .find(|song| song.id == song_id)
        .expect("the song is still listed")
        .favorite;
    let in_favorites_view = app
        .favorites(SORT, None, 16)
        .expect("favorites")
        .items
        .iter()
        .any(|song| song.id == song_id);
    let count = app.library_counts().expect("counts").favorites;
    ReadBack {
        row,
        in_favorites_view,
        count,
    }
}

/// LD-01「收藏写入返回后立即回读已是新值」。
#[test]
fn favorite_is_visible_to_every_read_path_the_moment_the_write_returns() {
    let lib = library();
    let song_id = lib.songs[0].clone();
    let song = SongId::from_str(&song_id).expect("song id");

    let before = read_back(&lib.app, &song_id);
    assert!(!before.row, "the song starts un-favorited");
    assert!(
        !before.in_favorites_view,
        "and is absent from the favorites view"
    );
    assert_eq!(before.count, 0, "and the favorites count starts at zero");

    // The write itself.
    let written = lib.app.set_favorite(song, true).expect("favorite");
    assert!(written.favorite, "the write reports the committed value");

    // The read model must already agree — no waiting for a background
    // projection to catch up.
    let after = read_back(&lib.app, &song_id);
    assert!(
        after.row,
        "the song row must read favorite=true the moment the write returns"
    );
    assert!(
        after.in_favorites_view,
        "the favorites view must list the song the moment the write returns"
    );
    assert_eq!(
        after.count, 1,
        "the favorites count must include the song the moment the write returns"
    );
}

/// LD-01「收藏的投影失败仍报告为已保存待投影」——目录提交已经发生，
/// 收藏意图不能丢，也不能变成第二个事实。
#[test]
fn unfavorite_is_visible_to_every_read_path_too() {
    let lib = library();
    let song_id = lib.songs[0].clone();
    let song = SongId::from_str(&song_id).expect("song id");

    lib.app.set_favorite(song, true).expect("favorite");
    assert!(read_back(&lib.app, &song_id).row, "favorited first");

    lib.app.set_favorite(song, false).expect("unfavorite");
    let after = read_back(&lib.app, &song_id);
    assert!(
        !after.row,
        "the song row must read favorite=false the moment the write returns"
    );
    assert!(
        !after.in_favorites_view,
        "the favorites view must drop the song the moment the write returns"
    );
    assert_eq!(after.count, 0, "the favorites count must drop it too");
}

/// 收藏计数不得因重复操作重复累加：同一首歌在读模型滞后期间连续两次以
/// 「喜欢」写入，计数相对写入前只增加 1。
#[test]
fn favoriting_the_same_song_twice_counts_it_once() {
    let lib = library();
    let song_id = lib.songs[0].clone();
    let song = SongId::from_str(&song_id).expect("song id");

    let before = read_back(&lib.app, &song_id).count;

    lib.app.set_favorite(song, true).expect("favorite");
    lib.app.set_favorite(song, true).expect("favorite again");

    let after = read_back(&lib.app, &song_id).count;
    assert_eq!(
        after,
        before + 1,
        "the same song favorited twice must count once, not twice"
    );
}

/// 收藏写入只追平增量投影：同步投影不得重放整库历史。水位只前进本次写入
/// 对应的那一条提交。
#[test]
fn a_favorite_projects_only_the_incremental_commit() {
    let lib = library();
    let song_id = lib.songs[0].clone();
    let song = SongId::from_str(&song_id).expect("song id");

    let before = lib
        .database
        .projection_status(lib.root)
        .expect("projection status before the favorite")
        .applied_library_commit_seq;

    lib.app.set_favorite(song, true).expect("favorite");

    let after = lib
        .database
        .projection_status(lib.root)
        .expect("projection status after the favorite")
        .applied_library_commit_seq;
    assert_eq!(
        after,
        before + 1,
        "the favorite must advance the projection watermark by exactly one commit, \
         not replay the library history"
    );
}
