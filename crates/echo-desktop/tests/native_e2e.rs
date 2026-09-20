//! Native end-to-end driver (task 13.2 / 13.3).
//!
//! Drives the REAL composition root that the Tauri shell uses — `assemble` +
//! `AppServices` with a scripted dialog boundary — over a **temp `SQLite`
//! library**, exercising the complete local loop a real session touches:
//!
//!   scan → search → favorite → playlist → import → delete/undo → restart
//!
//! The "restart" half is a second `AppServices` assembly over the SAME data
//! dir (recovery + re-activation), asserting the data/UUID/playlist/favorite/
//! queue preference state is consistent across the boundary — exactly what a
//! process restart would observe.
//!
//! This is the echo-desktop-hosted driver the `task-13.2` check runs (and the
//! seam the 13.3 kill/restart check re-drives). It is hermetic: no `WebView`, no
//! OS dialogs, no real libmpv (real player smoke is the `player_smoke` suite
//! and the 13.5 manual track).

use echo_core::application::ports::{ImportSource, ImportSourceInfo, ImportSourceReader};
use echo_core::domain::catalog::{SongSort, SongSortField, SortDirection};
use echo_desktop::platform::dialogs::{PickedImport, RevealOutcome, SystemDialogs};
use echo_desktop::runtime::app::assemble;
use echo_desktop::runtime::services::AppServices;
use echo_desktop::runtime::StartupSupervisor;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

/// A deterministic dialog boundary that answers the env-scripted picks the
/// driver sets, so `AppServices` behaves like a user who confirmed a chosen
/// root and an import batch.
#[derive(Default)]
struct ScriptedDialogs {
    root: std::sync::Mutex<Option<PathBuf>>,
    import: std::sync::Mutex<Option<PickedImport>>,
}

impl ScriptedDialogs {
    fn with_root_and_import(root: PathBuf, paths: Vec<PathBuf>) -> Self {
        let batch = SinglePathBatch::from_paths(paths).expect("batch over picked paths");
        Self {
            root: std::sync::Mutex::new(Some(root)),
            import: std::sync::Mutex::new(Some(PickedImport {
                sources: batch.files.iter().map(|f| f.source().clone()).collect(),
                reader: Box::new(batch),
            })),
        }
    }
}

impl SystemDialogs for ScriptedDialogs {
    fn pick_library_directory(
        &self,
    ) -> Result<Option<std::path::PathBuf>, echo_core::error::Error> {
        Ok(self
            .root
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take())
    }
    fn pick_audio_files(&self) -> Result<Option<PickedImport>, echo_core::error::Error> {
        Ok(self
            .import
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take())
    }
    fn reveal(
        &self,
        _absolute: &std::path::Path,
    ) -> Result<RevealOutcome, echo_core::error::Error> {
        Ok(RevealOutcome::Revealed)
    }
}

/// A minimal single-path import reader for the driver (from the picker-selected
/// audio file). Uses `echo_desktop::platform::import::SingleFileImport`.
struct SinglePathBatch {
    files: Vec<echo_desktop::platform::import::SingleFileImport>,
}

impl SinglePathBatch {
    fn from_paths(paths: Vec<PathBuf>) -> Result<Self, echo_core::error::Error> {
        let files = paths
            .into_iter()
            .map(|p| {
                let name = p
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("audio")
                    .to_owned();
                echo_desktop::platform::import::single_file_import(&p, name)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { files })
    }
    fn by_key(
        &self,
        source: &ImportSource,
    ) -> Option<&echo_desktop::platform::import::SingleFileImport> {
        self.files.iter().find(|f| f.source() == source)
    }
}

impl ImportSourceReader for SinglePathBatch {
    fn describe(&self, source: &ImportSource) -> Result<ImportSourceInfo, echo_core::error::Error> {
        self.by_key(source)
            .ok_or_else(|| {
                echo_core::error::Error::validation(
                    echo_core::error::Subject::Other,
                    "source",
                    "unknown",
                )
            })?
            .describe(source)
    }
    fn open<'a>(
        &'a self,
        source: &ImportSource,
    ) -> Result<Box<dyn std::io::Read + 'a>, echo_core::error::Error> {
        self.by_key(source)
            .ok_or_else(|| {
                echo_core::error::Error::validation(
                    echo_core::error::Subject::Other,
                    "source",
                    "unknown",
                )
            })?
            .open(source)
    }
    fn sidecar(
        &self,
        source: &ImportSource,
    ) -> Result<Option<echo_core::application::ports::SidecarInfo>, echo_core::error::Error> {
        self.by_key(source)
            .ok_or_else(|| {
                echo_core::error::Error::validation(
                    echo_core::error::Subject::Other,
                    "source",
                    "unknown",
                )
            })?
            .sidecar(source)
    }
    fn open_sidecar<'a>(
        &'a self,
        source: &ImportSource,
    ) -> Result<Option<Box<dyn std::io::Read + 'a>>, echo_core::error::Error> {
        self.by_key(source)
            .ok_or_else(|| {
                echo_core::error::Error::validation(
                    echo_core::error::Subject::Other,
                    "source",
                    "unknown",
                )
            })?
            .open_sidecar(source)
    }
}

/// Assemble a ready `AppServices` over the given data dir + dialogs, running
/// boot recovery (the crash/restart path) and opening the readiness gate —
/// exactly the composition root wiring, minus the Tauri shell.
fn services(data_dir: &Path, dialogs: Arc<dyn SystemDialogs>) -> AppServices {
    let routed = assemble(&data_dir.join("echo.sqlite"), &data_dir.join("covers"))
        .expect("assemble real SQLite + root registry");
    let startup = StartupSupervisor::new();
    let scan_supervisor = echo_core::application::scan::ScanSupervisor::new();
    let trash = echo_desktop::platform::trash::DesktopTrash::default();
    startup
        .run_recovery(&routed.deps, &scan_supervisor, &trash)
        .expect("boot recovery resolves the gate");
    startup.on_ready();
    // The shell's frontend registers its file-open listener after the WebView
    // loads; this harness has no WebView, so it declares the gate open in the
    // same order the shell does (deliver-file-opens-after-frontend-ready).
    startup.mark_frontend_ready();
    AppServices::with_runtime(
        routed.deps.clone(),
        scan_supervisor,
        std::sync::Arc::new(startup),
        dialogs,
        routed.registry.clone(),
        routed.database,
        echo_core::application::root_switch::Blockers::new(),
    )
}

fn temp_library(root: &Path) {
    // Seed a small library with the real licensed audio fixtures so the scan's
    // media probe actually parses them (fake bytes would be classified as
    // corrupt/unsupported). Fixtures live at the workspace root.
    // A portable library discovers managed audio only below `media/`; keeping
    // fixtures in that tree also proves the control surface never becomes a
    // scan candidate.
    let media = root.join("media");
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

const SORT: SongSort = SongSort {
    field: SongSortField::AddedAt,
    direction: SortDirection::Asc,
};

/// The full native loop, then a restart against the same data dir.
/// Everything the restart half asserts against, established by launch 1.
struct NativeLoopOutcome {
    root: echo_core::domain::ids::LibraryRootId,
    /// Web-facing id of the song that was favorited, added to the playlist,
    /// deleted and then restored by undo.
    fav_song: String,
    playlist: String,
    imported_song: String,
}

/// Launch 1: drive scan to search to favorite to playlist to import to
/// delete/undo over `data_dir`, returning the relationships that a restart
/// must preserve.
///
/// `import_stay` must outlive this call: it holds the external fixture the
/// picker hands in, and the import copies out of it. A distinct format (ogg)
/// is genuinely new content, so BLAKE3 dedup does not collapse it into a
/// seeded duplicate.
fn first_launch_establishes_relationships(
    data_dir: &Path,
    root_dir: &Path,
    import_stay: &Path,
) -> NativeLoopOutcome {
    let import_src = {
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("workspace root");
        let p = import_stay.join("imported-song.ogg");
        std::fs::copy(repo.join("fixtures/audio/tone-short.ogg"), &p).expect("copy real fixture");
        p
    };
    let dialogs = Arc::new(ScriptedDialogs::with_root_and_import(
        root_dir.to_path_buf(),
        vec![import_src],
    ));
    let app = services(data_dir, dialogs);

    let status = app
        .choose_library_root()
        .expect("pick+activate root")
        .expect("configured");
    // A brand-new root activates read-only: write capability is established by
    // the first import, which exclusive-creates the owned staging dir (the
    // design's safe-write barrier). So we do NOT assert writable here.
    let root =
        echo_core::domain::ids::LibraryRootId::from_str(&status.active_root).expect("root id");

    let snapshot = app.start_scan(root).expect("scan converges once");
    assert!(snapshot.processed >= 2, "at least the seeded songs");
    let song_ids: Vec<String> = app
        .all_songs(SORT, None, 16)
        .expect("all songs")
        .items
        .into_iter()
        .map(|s| s.id)
        .collect();
    assert!(song_ids.len() >= 2, "songs listed after scan");

    // Search must find a seeded title.
    let search = app.search("tone", false, SORT, None, 16).expect("search");
    assert!(!search.items.is_empty(), "search hits the seeded title");

    // Favorite the first song.
    let fav_song = song_ids[0].clone();
    let fav_id = echo_core::domain::ids::SongId::from_str(&fav_song).expect("song id");
    let updated = app.set_favorite(fav_id, true).expect("favorite");
    assert!(updated.favorite, "favorite toggled on");

    // Playlist: create + add the favorited song.
    let playlist = app
        .create_playlist(root, "native-driver")
        .expect("create playlist");
    let pl_id = echo_core::domain::ids::PlaylistId::from_str(&playlist).expect("playlist id");
    app.add_to_playlists(fav_id, &[pl_id]).expect("add member");

    // Import the external file through the dialog boundary (like a real user:
    // the picker returns the batch, `choose_and_import_files` plans it).
    let batch = app
        .choose_and_import_files()
        .expect("import dialog")
        .expect("batch (not cancelled)");
    let imported_song = match &batch.results[0] {
        echo_desktop::ipc::dto::ImportResultDto::Imported { song_id, .. } => song_id.clone(),
        other => panic!("expected imported song, got {other:?}"),
    };

    // Delete the favorited song, then undo within the window.
    let op = app.delete_song(root, fav_id).expect("delete");
    let op_id = echo_core::domain::ids::OperationId::from_str(&op).expect("operation id");
    app.undo_delete(root, op_id).expect("undo");
    // After undo, the song is available again and still favorited.
    let faves = app.favorites(SORT, None, 16).expect("favorites");
    assert!(
        faves.items.iter().any(|s| s.id == fav_song),
        "favorited song restored by undo"
    );
    let members = app.playlist_members(pl_id).expect("playlist members");
    assert!(
        members.iter().any(|s| s.id == fav_song),
        "playlist membership survives undo"
    );

    NativeLoopOutcome {
        root,
        fav_song,
        playlist,
        imported_song,
    }
}

/// Launch 2: assemble a second `AppServices` over the SAME data dir (a proxy
/// for a process restart, so boot recovery re-runs over the same DB) and prove
/// the relationships from launch 1 are intact.
fn restart_preserves_relationships(data_dir: &Path, outcome: &NativeLoopOutcome) {
    let dialogs2 = Arc::new(ScriptedDialogs::default());
    let app2 = services(data_dir, dialogs2);

    // The active root, favorites, playlist and imported song all survive.
    let root2 = app2
        .bootstrap()
        .expect("bootstrap")
        .active_root
        .expect("active root still configured");
    assert_eq!(
        root2,
        outcome.root.to_string(),
        "root identity stable across restart"
    );

    // A restart creates a new process-local RootRegistry. Re-scan the same
    // directory before checking persisted relationships: this proves startup
    // restored the persisted root-id to path binding rather than merely reading
    // the catalog rows that happened to remain in SQLite.
    app2.start_scan(outcome.root)
        .expect("same active root resolves for scanning after restart");

    let faves2 = app2
        .favorites(SORT, None, 16)
        .expect("favorites after restart");
    assert!(
        faves2.items.iter().any(|s| s.id == outcome.fav_song),
        "favorite survives restart"
    );

    let playlists2 = app2.playlists().expect("playlists after restart");
    assert!(
        playlists2.iter().any(|p| p.id == outcome.playlist),
        "playlist survives restart"
    );

    let imported2 = app2
        .song_detail(
            echo_core::domain::ids::SongId::from_str(&outcome.imported_song).expect("imported id"),
        )
        .expect("imported song detail seen after restart");
    assert_eq!(
        imported2.availability, "available",
        "imported song available after restart"
    );

    // No duplicate UUID: the imported song appears exactly once across a fresh
    // scan of the same root.
    let all2 = app2.all_songs(SORT, None, 32).expect("all after restart");
    let occurrences = all2
        .items
        .iter()
        .filter(|s| s.id == outcome.imported_song)
        .count();
    assert_eq!(occurrences, 1, "no duplicate UUID after restart");
}

/// The full native loop, then a restart against the same data dir.
#[test]
fn native_loop_and_restart_preserve_relationships() {
    let data = tempfile::tempdir().expect("data dir");
    let root_dir = tempfile::tempdir().expect("root dir");
    temp_library(root_dir.path());

    // One external file (a real licensed fixture OUTSIDE the root) to bring in
    // via the picker. The temp dir must outlive the import, so it stays at test
    // scope rather than inside the phase helpers.
    let import_stay = tempfile::tempdir().expect("import src");

    let outcome =
        first_launch_establishes_relationships(data.path(), root_dir.path(), import_stay.path());
    restart_preserves_relationships(data.path(), &outcome);
}
