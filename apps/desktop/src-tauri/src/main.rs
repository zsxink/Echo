//! Echo desktop application binary.
//!
//! The binary owns only the Tauri application boundary. Domain work remains in
//! `echo-core`; desktop runtime, player and platform implementation grow in
//! `echo-desktop` in their respective tasks.

use std::{
    env,
    fs::OpenOptions,
    io::Write,
    path::PathBuf,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

// Only the macOS Now Playing refresh reads cover bytes in the shell; the
// `cover://` protocol itself lives in cover_protocol.rs, which sources
// `CoverCache` from echo-desktop. Keep the import platform-scoped so
// `-D warnings` clippy stays green on Windows/Linux.
#[cfg(target_os = "macos")]
use echo_core::application::ports::CoverCache;
#[cfg(not(target_os = "macos"))]
use echo_core::domain::state::PlaybackState;
use echo_desktop::platform::local_state::{CloseBehavior, DesktopStateStore};
use echo_desktop::platform::status_menu::StatusMenuSink;
#[cfg(not(target_os = "macos"))]
use echo_desktop::platform::status_menu::{self, PlaySummary};
use echo_desktop::player::coordinator::PlaybackCoordinator;
use echo_desktop::player::port::{PlayerCommand, PlayerError, PlayerPort};
use echo_desktop::runtime::app::assemble;
use echo_desktop::runtime::player;
use echo_desktop::runtime::services::AppServices;
use echo_desktop::runtime::StartupSupervisor;
#[cfg(not(target_os = "macos"))]
use tauri::menu::{MenuBuilder, MenuItemBuilder};
#[cfg(not(target_os = "macos"))]
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager, RunEvent};

mod commands;
mod cover_protocol;
mod dialogs;
#[cfg(target_os = "macos")]
mod macos_now_playing;
#[cfg(target_os = "macos")]
mod macos_status_row;
mod open_targets;

const MAIN_WINDOW: &str = "main";

type SharedPlaybackCoordinator = Arc<Mutex<PlaybackCoordinator<Arc<dyn PlayerPort>>>>;

/// The tray is created before playback composition. This small adapter keeps a
/// stable managed instance for its whole lifetime, then receives the real
/// coordinator once the player is ready instead of leaving menu controls on a
/// startup-only no-op sink.
#[derive(Default)]
struct RuntimeStatusMenuSink {
    coordinator: Mutex<Option<SharedPlaybackCoordinator>>,
}

impl RuntimeStatusMenuSink {
    fn install(&self, coordinator: SharedPlaybackCoordinator) {
        if let Ok(mut slot) = self.coordinator.lock() {
            *slot = Some(coordinator);
        }
    }
}

impl StatusMenuSink for RuntimeStatusMenuSink {
    fn on_command(&self, command: PlayerCommand) -> Result<(), PlayerError> {
        let coordinator = {
            let slot = self.coordinator.lock().map_err(|_| PlayerError::Backend {
                message: "playback coordinator lock poisoned".to_owned(),
            })?;
            slot.as_ref()
                .ok_or_else(|| PlayerError::Backend {
                    message: "playback coordinator is not installed".to_owned(),
                })?
                .clone()
        };
        let mut coordinator = coordinator.lock().map_err(|_| PlayerError::Backend {
            message: "playback coordinator lock poisoned".to_owned(),
        })?;
        match command {
            PlayerCommand::Play | PlayerCommand::Pause | PlayerCommand::TogglePlayPause => {
                coordinator.player().send(command)
            }
            PlayerCommand::Previous => {
                if coordinator.current().is_none() {
                    return Err(PlayerError::Backend {
                        message: "no current track".to_owned(),
                    });
                }
                coordinator.previous();
                Ok(())
            }
            PlayerCommand::Next => {
                coordinator
                    .advance_to_next()
                    .map(|_| ())
                    .ok_or_else(|| PlayerError::Backend {
                        message: "no next track".to_owned(),
                    })
            }
            _ => Err(PlayerError::Backend {
                message: "unsupported system playback command".to_owned(),
            }),
        }
    }
}

/// The event name carrying a `UiPlayerSnapshot` to the frontend (`playerStore`
/// subscribes via the bridge). Matches the `ipc::events` `player://snapshot`
/// design and the `BridgeCommandMap` consumer.
const PLAYER_SNAPSHOT_EVENT: &str = "player://snapshot";

/// Resolve the bundled libmpv dylib for the current platform. On macOS the
/// release app has it in `Echo.app/Contents/Frameworks`; `build.rs` stages the
/// same dependency set in `target/Frameworks` for `tauri dev`. Both are reached
/// through the *running executable's* own directory — not through Tauri's
/// `executable_dir()`, which on macOS is unsupported (`dirs::executable_dir`
/// returns `None`), so it can never be used to locate a sibling `Frameworks/`.
// The parameter exists only to keep a uniform call signature across platforms;
// the non-macOS branch cannot return a libmpv path, so the handle is unused.
// `missing_const_for_fn` is allowed because the macOS branch calls
// `std::env::current_exe()` (not const), which gates the whole fn — yet on a
// non-macOS build the body would otherwise satisfy the lint.
#[allow(clippy::missing_const_for_fn)]
fn bundled_libmpv(_: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    #[cfg(target_os = "macos")]
    {
        // `current_exe` is accurate in both layouts we ship/stage. `Frameworks`
        // sits two levels up from the executable in each — the rpath instrument
        // is `@executable_path/../Frameworks` (one `..` from the exe's *parent*
        // directory, matching `parent().parent()` here):
        //   • dev:  target/debug/Echo  → ../..  → target/Frameworks (staged set)
        //   • .app: .../Contents/MacOS/Echo → ../.. → .../Contents/Frameworks
        let exe = std::env::current_exe().ok()?;
        let frameworks = exe.parent()?.parent()?.join("Frameworks");
        let candidate = frameworks.join("libmpv.dylib");
        candidate.exists().then_some(candidate)
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

fn focus_main_window<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    // Resolve the stable window label only. Status-item activation never
    // constructs a WebView, so rapid/repeated activation cannot produce a
    // second main window or a second playback composition.
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Feed the macOS status row from the player port's authoritative snapshot
/// stream. `AppKit` is main-thread-only, so each state update is scheduled onto
/// Tauri's main thread; a closed player is normal during process shutdown.
#[cfg(target_os = "macos")]
fn spawn_macos_status_row_snapshot_refresh(app: tauri::AppHandle, port: &Arc<dyn PlayerPort>) {
    let initial = port.snapshot().state;
    let initial_app = app.clone();
    let _ = initial_app.run_on_main_thread(move || {
        macos_status_row::refresh_play_pause(initial);
    });
    let snapshots = port.subscribe_snapshots();
    thread::spawn(move || {
        while let Ok(snapshot) = snapshots.recv() {
            let state = snapshot.state;
            let _ = app.run_on_main_thread(move || {
                macos_status_row::refresh_play_pause(state);
            });
        }
    });
}

#[cfg(target_os = "macos")]
fn spawn_macos_now_playing_refresh(
    app: tauri::AppHandle,
    port: &Arc<dyn PlayerPort>,
    coordinator: SharedPlaybackCoordinator,
    metadata: Arc<player::QueueMetadataResolver>,
    cover_cache: Arc<dyn CoverCache>,
) {
    let snapshots = port.subscribe_snapshots();
    thread::spawn(move || {
        while let Ok(snapshot) = snapshots.recv() {
            let current = {
                let coord = coordinator.lock().expect("player coordinator lock");
                coord.current().cloned()
            };
            let track = current.as_ref().and_then(|entry| {
                let (title, artist, album, duration, cover_key) = match &entry.item {
                    echo_desktop::player::queue::QueueItem::Temporary(item) => (
                        Some(item.display_name.clone()),
                        None,
                        None,
                        item.duration,
                        None,
                    ),
                    echo_desktop::player::queue::QueueItem::Library(song_id) => {
                        let resolved = metadata.resolve(std::slice::from_ref(entry));
                        let item = resolved.get(song_id)?;
                        (
                            item.title.clone(),
                            item.artist.clone(),
                            item.album.clone(),
                            // Media durations are integer seconds far below 2^53,
                            // so the u64→f64 cast is exact for every real track.
                            #[allow(clippy::cast_precision_loss)]
                            item.duration_s.map(|s| s as f64),
                            item.cover_key.clone(),
                        )
                    }
                };
                Some(macos_now_playing::NowPlayingTrack {
                    entry_id: entry.id.to_string(),
                    title: title?,
                    artist,
                    album,
                    duration,
                    cover_key,
                })
            });
            let mut projection = macos_now_playing::project(&snapshot, track.as_ref());
            if let (Some(projection), Some(cover_key)) = (
                projection.as_mut(),
                track.as_ref().and_then(|track| track.cover_key.as_ref()),
            ) {
                projection.artwork = cover_cache.get(cover_key).ok().flatten();
            }
            let _ = app.run_on_main_thread(move || match projection {
                Some(projection) => macos_now_playing::publish(&projection),
                None => macos_now_playing::clear(),
            });
        }
        let _ = app.run_on_main_thread(macos_now_playing::clear);
    });
}

/// build `AppServices`, and register the playback coordinator + actor + theme
/// store as managed state, plus the snapshot→frontend forwarder.
///
/// Failure here is fatal (the app cannot serve commands without its runtime).
/// Uses `AppServices::with_runtime` with the real system dialogs (`TauriDialogs`)
/// and the SQLite-backed runtime-state store, so directory/file selection and
/// reveal come from the OS rather than a cancelling test double.
///
/// # Errors
///
/// `Io`/`Storage` when the database or cover cache cannot open, or `String` on
/// assembly / recovery failure; propagated as a fatal startup error.
#[allow(clippy::too_many_lines)]
fn wire_composition(
    app: &tauri::App,
    startup: &Arc<StartupSupervisor>,
) -> Result<(), Box<dyn std::error::Error>> {
    // The Gate overrides the data dir so a native run targets a hermetic temp
    // library and can be killed/restarted safely. Production never sets it.
    let app_data = match env::var("ECHO_GATE_DATA_DIR") {
        Ok(dir) => std::path::PathBuf::from(dir),
        Err(_) => app.path().app_data_dir()?,
    };
    let db_path = app_data.join("echo.sqlite");
    let cover_dir = app_data.join("covers");
    let routed =
        assemble(&db_path, &cover_dir).map_err(|error| format!("assemble runtime: {error}"))?;

    // Run crash recovery (a fresh DB has no pending operations) and open the
    // readiness gate BEFORE constructing AppServices, so the gate is resolved
    // when the command surface becomes callable.
    //
    // This is the process-wide supervisor created at the top of `main`, not a
    // new one: the OS file-open paths consult it directly, so recovery, the
    // frontend-ready gate and `AppServices::startup()` must all be the same
    // object (deliver-file-opens-after-frontend-ready, D3). A second instance
    // would let the shell's delivery gate and the command surface's readiness
    // gate diverge with nothing to notice it.
    let supervisor = Arc::clone(startup);
    let scan_supervisor = echo_core::application::scan::ScanSupervisor::new();
    let trash = echo_desktop::platform::trash::DesktopTrash::default();
    supervisor
        .run_recovery(&routed.deps, &scan_supervisor, &trash)
        .map_err(|error| format!("runtime recovery failed: {error}"))?;
    supervisor.on_ready();

    // A scripted Gate run (native E2E) replaces the OS pickers with env-driven
    // dialogs; everything else is identical to production.
    let gate_dialogs: Arc<dyn echo_desktop::platform::dialogs::SystemDialogs> =
        if env::var_os("ECHO_GATE_ROOT").is_some() || env::var_os("ECHO_GATE_IMPORT").is_some() {
            Arc::new(dialogs::GateDialogs)
        } else {
            Arc::new(dialogs::TauriDialogs::new(app.handle().clone()))
        };
    let services = AppServices::with_runtime(
        routed.deps.clone(),
        scan_supervisor,
        supervisor,
        gate_dialogs,
        routed.registry.clone(),
        routed.database.clone(),
        echo_core::application::root_switch::Blockers::new(),
    );
    app.manage(services);
    app.manage(routed.registry.clone());
    app.manage(routed.deps.clone());
    // The `cover://` protocol resolves bytes by opaque asset key (design §16).
    // It looks the store up as the *dynamic* `CoverCache` type, so the cache has
    // to be registered under exactly that type — the same instance the scan
    // persists embedded artwork into (design §115 内置优先). Without this the
    // protocol answers 404 for every request and no artwork ever renders.
    app.manage(routed.deps.cover_cache.clone());

    // Theme / close-behavior persistence (task 7.6).
    let platform_default = if cfg!(target_os = "macos") {
        echo_desktop::platform::local_state::PlatformCloseDefault::Macos
    } else {
        echo_desktop::platform::local_state::PlatformCloseDefault::Other
    };
    let local_state = Arc::new(DesktopStateStore::new(
        app_data.join("desktop-state.json"),
        platform_default,
    ));
    app.manage(local_state.clone());

    // Playback: spawn the real libmpv actor and keep the coordinator handle
    // the command layer drives.  Do not substitute FakePlayer here: it makes
    // the UI report a successful play transition while producing no sound.
    let resolver = player::PlayerController::resolver(&routed.deps);
    let libmpv = bundled_libmpv(app.handle()).ok_or_else(|| {
        "bundled libmpv is missing; playback cannot start (run the macOS build staging)".to_owned()
    })?;
    let controller = player::PlayerController::spawn_mpv(&libmpv, resolver)
        .map_err(|error| format!("start libmpv player: {error}"))?;
    let player_source: Arc<std::sync::Mutex<Option<String>>> =
        Arc::new(std::sync::Mutex::new(None));
    // Session persistence (task 8.9 落盘接线): the saver thread throttles
    // durable playback-session writes onto the atomic desktop-state store.
    let saver_persistence: Arc<dyn echo_desktop::player::session::SessionPersistence> = Arc::new(
        echo_desktop::player::session::StateStoreSession::new(local_state),
    );
    let player_handle = commands::PlayerHandle {
        coordinator: controller.coordinator.clone(),
        source: player_source.clone(),
        persistence: saver_persistence.clone(),
    };
    app.manage(player_handle);
    if let Some(sink) = app.try_state::<Arc<RuntimeStatusMenuSink>>() {
        sink.install(controller.coordinator.clone());
    }

    #[cfg(target_os = "macos")]
    spawn_macos_status_row_snapshot_refresh(app.handle().clone(), &controller.port);

    player::spawn_session_saver(
        controller.port.clone(),
        controller.coordinator.clone(),
        saver_persistence,
        player_source,
        std::time::Duration::from_secs(3),
    );

    // Forward actor snapshots to the frontend as typed UI snapshots. The actor
    // contributes transport state; the coordinator supplies everything
    // queue-derived (entries + per-round failed set + current entry + mode), so
    // the UI snapshot reflects one consistent view of what is playing (11.2).
    let handle = app.handle().clone();
    let coordinator = controller.coordinator.clone();
    let queue_provider: Arc<dyn Fn() -> player::CoordinatorView + Send + Sync> =
        Arc::new(move || {
            let coord = coordinator.lock().expect("player coordinator lock");
            player::CoordinatorView {
                entries: coord.queue_view(),
                failed_round: coord.failed_round().collect(),
                blocked: coord
                    .queue_view()
                    .iter()
                    .filter(|entry| coord.queue().is_blocked(entry.id))
                    .map(|entry| entry.id)
                    .collect(),
                current: coord.current().cloned(),
                mode: coord.mode(),
            }
        });
    let emit: player::SnapshotEmitter = Box::new(move |ui| {
        let _ = handle.emit(PLAYER_SNAPSHOT_EVENT, ui);
    });
    // The queue-panel metadata resolver: maps library song ids to title /
    // artist / duration / cover-asset-key, batch-cached per song so the 10 Hz
    // position stream never triggers per-row metadata queries (task 2.2).
    let queue_metadata = Arc::new(player::QueueMetadataResolver::new(
        routed.deps.songs.clone(),
        routed.deps.covers.clone(),
    ));
    // macOS hands the resolver on to the Now Playing refresh below, so it needs
    // a second handle for the forwarder; other targets own it solely in the
    // forwarder thread and can move it outright.
    #[cfg(target_os = "macos")]
    let forwarder_metadata = queue_metadata.clone();
    #[cfg(not(target_os = "macos"))]
    let forwarder_metadata = queue_metadata;
    player::spawn_forwarder(
        controller.port.clone(),
        queue_provider,
        forwarder_metadata,
        emit,
    );
    #[cfg(target_os = "macos")]
    spawn_macos_now_playing_refresh(
        app.handle().clone(),
        &controller.port,
        controller.coordinator.clone(),
        queue_metadata,
        routed.deps.cover_cache.clone(),
    );
    // Auto-advance: a track reaching EOF (or failing to load) must drive the
    // coordinator to the next entry. Without this the queue stalls on `ended`
    // until the user presses next manually.
    player::spawn_auto_advance(controller.port.clone(), controller.coordinator.clone());
    // Playback statistics (task 8.10): feed the real snapshot stream into the
    // accumulator so a qualified listen reaches Core's idempotent
    // `record_playback` (play_count / 最近播放). Without this the accumulator
    // had no production caller and play counts never moved.
    let stats_sink = Arc::new(player::CorePlaybackRecorder::new(
        routed.database.clone(),
        routed.deps.control.clone(),
        routed.deps.device_id.clone(),
        routed.deps.roots.clone(),
    ));
    player::spawn_stats_recorder(
        controller.port.clone(),
        controller.coordinator.clone(),
        stats_sink,
    );
    Ok(())
}

/// The Gate sets this only for its short-lived subprocesses. It lets the
/// native check prove an explicit application exit without adding a public
/// command or privileged frontend capability.
fn schedule_gate_exit(app: tauri::AppHandle) {
    let Ok(milliseconds) = env::var("ECHO_GATE_QUIT_AFTER_MS") else {
        return;
    };
    let Ok(milliseconds) = milliseconds.parse::<u64>() else {
        return;
    };

    thread::spawn(move || {
        thread::sleep(Duration::from_millis(milliseconds));
        app.exit(0);
    });
}

/// The Gate uses this opt-in log to assert that a hot second launch delivered
/// its file path to the already-running instance. Production never sets it.
fn record_gate_open(paths: &[PathBuf]) {
    let Ok(log_path) = env::var("ECHO_GATE_OPEN_LOG") else {
        return;
    };
    let Ok(mut log) = OpenOptions::new().create(true).append(true).open(log_path) else {
        return;
    };
    for path in paths {
        let _ = writeln!(log, "{}", path.to_string_lossy());
    }
}

/// The shell→frontend event carrying one accepted file-open path (task 9.1).
/// The frontend bridge subscribes to this; the payload is a single absolute
/// path that was either delivered immediately or drained from the startup FIFO.
/// The event name also appears in `echo-desktop::ipc::events`.
const FILE_OPEN_REQUEST: &str = "app://file-open-request";

/// Deliver a batch of accepted file-open paths to the frontend.
///
/// The single place that emits [`FILE_OPEN_REQUEST`], so a path — delivered
/// immediately or drained later — is recorded for the Gate exactly once.
fn emit_file_opens(app: &tauri::AppHandle, paths: Vec<PathBuf>) {
    if paths.is_empty() {
        return;
    }
    record_gate_open(&paths);
    let _ = app.emit(FILE_OPEN_REQUEST, paths);
}

/// Route one OS file-open path through the startup supervisor (task 9.1).
///
/// While the runtime is still initializing — or the frontend has not yet
/// registered its file-open listener — the path is stashed in the bounded FIFO
/// and later drained by [`drain_pending_opens`], never silently dropped.
/// `receive_file_open` returns `Some` only once **both** gates are open, and
/// only then is an emit safe: Tauri drops an event with no registered JS
/// listener, which is what happened to every cold-start double-click before
/// `deliver-file-opens-after-frontend-ready`.
///
/// `startup` is passed in rather than looked up through `try_state` on purpose.
/// macOS delivers a cold-start `odoc` **before `.setup()` runs**, so a managed
/// state lookup is `None` at exactly the moment the first — and only — open
/// request arrives; that lookup is what dropped the path and made a
/// double-click merely launch the app. The caller owns the process-wide
/// instance created at the top of [`main`], so there is no window in which a
/// path can arrive with nowhere to go.
fn deliver_file_open(app: &tauri::AppHandle, startup: &StartupSupervisor, path: PathBuf) {
    if let Some(ready_path) = startup.receive_file_open(path) {
        emit_file_opens(app, vec![ready_path]);
    }
}

/// The frontend registered its `app://file-open-request` listener: open the
/// delivery gate and hand back every path queued before that
/// (deliver-file-opens-after-frontend-ready).
///
/// This is the **only** delivery point for queued paths. The shell cannot drain
/// them when `.setup()` finishes — the `WebView` has not loaded yet, so
/// `js_event_listeners` is still empty and Tauri would drop the emit. Paths
/// therefore wait here instead of being lost.
///
/// Idempotent (see [`StartupSupervisor::mark_frontend_ready`]): a repeat call
/// drains an empty queue, so the frontend effect may run twice.
fn drain_pending_opens(app: &tauri::AppHandle, startup: &StartupSupervisor) {
    let drained = startup.mark_frontend_ready();
    emit_file_opens(app, drained);
}

#[allow(clippy::too_many_lines)]
fn main() {
    // Created here — before the Tauri application exists — on purpose. macOS
    // delivers a cold-start `odoc` (a double-clicked file) while
    // `Builder::build()` is still running, i.e. **before `.setup()`**: a
    // supervisor registered only inside `.setup()` is therefore still missing
    // when the first and only open request of that launch arrives, `try_state`
    // returns `None`, and the path is dropped — "Echo 启动了但没播放". One
    // process-wide instance is shared by the managed state, the single-instance
    // argv path and the `RunEvent::Opened` path, so there is no instant at which
    // an open request has nowhere to be queued.
    let startup = Arc::new(StartupSupervisor::new());

    let app = match tauri::Builder::default()
        // The single-instance plugin is registered first, as required by the
        // plugin, so a later platform adapter cannot create a second process.
        .register_uri_scheme_protocol("cover", cover_protocol::cover_protocol_handler)
        .plugin(tauri_plugin_single_instance::init({
            let startup = Arc::clone(&startup);
            move |app, args, _cwd| {
                // A second launch (or a file-open) on a running instance: wake the
                // main window, then forward each path exactly once through the
                // startup FIFO (task 9.1). `skip(1)` drops the argv program name;
                // macOS/Windows/Linux argv unification is task 9.2.
                focus_main_window(app);
                for path in args.into_iter().skip(1) {
                    deliver_file_open(app, &startup, PathBuf::from(path));
                }
            }
        }))
        // Native directory/file pickers in the Rust side (task 7.5 real wiring)
        // and reveal-in-folder: `echo-desktop` stays Tauri-free, so the
        // adapters live in this shell crate. Rust-side `DialogExt` calls do not
        // go through the WebView capability system, so the main window keeps
        // its minimal permission set (no dialog/fs permissions granted).
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            commands::get_bootstrap_state,
            commands::library_status,
            commands::all_songs,
            commands::search,
            commands::favorites,
            commands::recent,
            commands::library_counts,
            commands::playlists,
            commands::playlist_members,
            commands::song_detail,
            commands::get_lyrics,
            commands::song_cover_keys,
            commands::set_favorite,
            commands::create_playlist,
            commands::rename_playlist,
            commands::set_playlist_cover,
            commands::delete_playlist,
            commands::add_to_playlists,
            commands::remove_playlist_song,
            commands::delete_song,
            commands::undo_delete,
            commands::choose_library_root,
            commands::choose_and_import_files,
            commands::reveal_song,
            commands::start_scan,
            commands::cancel_scan,
            commands::set_theme,
            commands::set_close_behavior,
            commands::get_close_behavior,
            commands::play_playlist_context,
            commands::play_library_context,
            commands::restore_playback_session,
            commands::play_temporary_file,
            commands::import_current_temporary_file,
            commands::file_open_frontend_ready,
            commands::player_control,
            commands::queue_command,
            commands::set_volume,
            commands::toggle_mute,
            commands::seek,
        ])
        .setup({
            let startup = Arc::clone(&startup);
            move |app| {
                // Register the process-wide supervisor as managed state so the
                // command layer — which cannot capture a closure and must reach
                // it through `try_state` — sees the *same* object the shell's
                // open paths use (deliver-file-opens-after-frontend-ready, D3).
                // Every command runs only after the frontend has loaded, so this
                // state is always registered by then; it is the open paths that
                // can fire earlier, which is why they take the captured `Arc`
                // instead of a state lookup.
                app.manage(Arc::clone(&startup));

                // Structured tracing + panic hook (task 7.8, design §17): install
                // the rolling-file logger and the local crash diagnostic before
                // anything else so all startup work is observable. The diagnostics
                // directory is `app_data_dir/logs`; logs stay local, never uploaded.
                let app_data = app.path().app_data_dir()?;
                let diagnostics = echo_desktop::platform::diagnostics::Diagnostics::new(app_data);
                let _installed = diagnostics.install_logger();
                diagnostics.install_panic_hook();

                // Productionized platform entry (task 9.3): a current-play summary
                // plus 播放/暂停、上一首、下一首、显示 Echo、退出. The summary and
                // the transport→command mapping are decision-logic in
                // `status_menu` (unit-tested); the shell only builds the widgets and
                // dispatches. Until the composition root connects a sink, transport
                // clicks fall through to `NoopSink`.
                let status_sink = Arc::new(RuntimeStatusMenuSink::default());
                // macOS keeps a handle for the Now Playing / status row installs
                // below; other platforms hand the sink wholly to the managed state.
                #[cfg(target_os = "macos")]
                let managed_status_sink = status_sink.clone();
                #[cfg(not(target_os = "macos"))]
                let managed_status_sink = status_sink;
                app.manage(managed_status_sink);

                #[cfg(target_os = "macos")]
                {
                    macos_now_playing::install(status_sink.clone())?;
                }

                #[cfg(target_os = "macos")]
                {
                    let show_handle = app.handle().clone();
                    let quit_handle = app.handle().clone();
                    macos_status_row::install(
                        status_sink,
                        Arc::new(move || focus_main_window(&show_handle)),
                        Arc::new(move || {
                            if let Some(player) = quit_handle.try_state::<commands::PlayerHandle>()
                            {
                                commands::flush_player_session(&player);
                            }
                            quit_handle.exit(0);
                        }),
                    )?;
                }

                #[cfg(not(target_os = "macos"))]
                {
                    let summary_text = {
                        let playback = PlaySummary {
                            state: PlaybackState::Stopped,
                            ..PlaySummary::default()
                        };
                        format!("{} — {}", playback.title_line(), playback.status_line())
                    };
                    let summary = MenuItemBuilder::with_id("echo-summary", summary_text.as_str())
                        .enabled(false)
                        .build(app)?;
                    let play_pause = MenuItemBuilder::with_id(
                        status_menu::MENU_PLAY_PAUSE,
                        status_menu::play_pause_label(PlaybackState::Stopped),
                    )
                    .build(app)?;
                    let previous = MenuItemBuilder::with_id(status_menu::MENU_PREVIOUS, "上一首")
                        .build(app)?;
                    let next =
                        MenuItemBuilder::with_id(status_menu::MENU_NEXT, "下一首").build(app)?;
                    let show =
                        MenuItemBuilder::with_id(status_menu::MENU_SHOW, "显示 Echo").build(app)?;
                    let quit =
                        MenuItemBuilder::with_id(status_menu::MENU_QUIT, "退出").build(app)?;
                    let menu = MenuBuilder::new(app)
                        .item(&summary)
                        .separator()
                        .item(&play_pause)
                        .item(&previous)
                        .item(&next)
                        .separator()
                        .item(&show)
                        .separator()
                        .item(&quit)
                        .build()?;

                    TrayIconBuilder::with_id("echo-gate")
                        .menu(&menu)
                        // A hidden main window needs a direct, discoverable way back:
                        // clicking the platform status item is equivalent to choosing
                        // “显示 Echo” from its menu. Restrict this to left-button release
                        // so right-click continues to open the platform menu normally.
                        .on_tray_icon_event(|tray, event| {
                            if matches!(
                                event,
                                TrayIconEvent::Click {
                                    button: MouseButton::Left,
                                    button_state: MouseButtonState::Up,
                                    ..
                                }
                            ) {
                                focus_main_window(tray.app_handle());
                            }
                        })
                        .on_menu_event(|app, event| {
                            match event.id().as_ref() {
                                status_menu::MENU_SHOW => focus_main_window(app),
                                status_menu::MENU_QUIT => {
                                    if let Some(player) = app.try_state::<commands::PlayerHandle>()
                                    {
                                        commands::flush_player_session(&player);
                                    }
                                    app.exit(0);
                                }
                                id => {
                                    // Transport item: forward the coarse player command
                                    // to the playback sink. The composition root
                                    // installs the coordinator forwarder; `NoopSink`
                                    // remains until then.
                                    if let Some(command) = status_menu::transport_command(id) {
                                        if let Some(sink) =
                                            app.try_state::<Arc<RuntimeStatusMenuSink>>()
                                        {
                                            let _ = sink.on_command(command);
                                        }
                                    }
                                }
                            }
                        })
                        .build(app)?;
                }

                // -----------------------------------------------------------------
                wire_composition(app, &startup)?;

                schedule_gate_exit(app.handle().clone());

                // The shell is fully initialized — but that is *not* the moment to
                // deliver file-opens. The WebView has not loaded, so no JS listener
                // is registered and Tauri would drop the emit; the paths stay in the
                // queue until the frontend calls `file_open_frontend_ready`
                // (deliver-file-opens-after-frontend-ready).
                Ok(())
            }
        })
        .build(tauri::generate_context!())
    {
        Ok(app) => app,
        Err(error) => {
            eprintln!("failed to initialize Echo: {error}");
            std::process::exit(1);
        }
    };

    app.run({
        // Only the macOS `RunEvent::Opened` arm routes through the startup
        // supervisor; the closed-over `startup` (line 683) is enough for other
        // platforms, where a shadow would otherwise be an unused variable.
        #[cfg(target_os = "macos")]
        let startup = Arc::clone(&startup);
        move |app, event| match event {
            RunEvent::ExitRequested { .. } => {
                #[cfg(target_os = "macos")]
                macos_now_playing::clear();
            }
            // macOS delivers file-association opens through `RunEvent::Opened`; they
            // go through the same FIFO as the single-instance argv path (task 9.1).
            // The variant is macOS-only, so it can never match on other targets.
            #[cfg(target_os = "macos")]
            RunEvent::Opened { urls } => {
                focus_main_window(app);
                for path in open_targets::open_targets(&urls) {
                    deliver_file_open(app, &startup, path);
                }
            }
            // Clicking the macOS Dock icon emits `Reopen`, rather than a tray-icon
            // event. Restore the hidden player window only when no window is visible.
            #[cfg(target_os = "macos")]
            RunEvent::Reopen {
                has_visible_windows,
                ..
            } => {
                if !has_visible_windows {
                    focus_main_window(app);
                }
            }
            RunEvent::WindowEvent {
                label,
                event: tauri::WindowEvent::CloseRequested { api, .. },
                ..
            } => {
                let background = app
                    .try_state::<Arc<DesktopStateStore>>()
                    .and_then(|store| store.close_behavior().ok())
                    == Some(CloseBehavior::Background);
                if label == MAIN_WINDOW && background {
                    if let Some(player) = app.try_state::<commands::PlayerHandle>() {
                        commands::flush_player_session(&player);
                    }
                    api.prevent_close();
                    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
                        let _ = window.hide();
                    }
                }
            }
            _ => {}
        }
    });
}

// Kept last: `clippy::items_after_test_module` requires a `#[cfg(test)]` module
// to be the final item in the file, and `cover_media_type`/`main` below read
#[cfg(test)]
mod cover_canvas_tests {
    #[test]
    fn main_window_activation_restores_one_existing_window_without_creating_another() {
        use tauri::Manager;

        let app = tauri::test::mock_app();
        let main = tauri::WebviewWindowBuilder::new(
            &app,
            super::MAIN_WINDOW,
            tauri::WebviewUrl::default(),
        )
        .build()
        .expect("mock main window");

        main.hide().expect("hide mock window");
        super::focus_main_window(app.handle());
        assert!(main.is_visible().expect("read visible state"));

        main.minimize().expect("minimize mock window");
        super::focus_main_window(app.handle());
        assert!(!main.is_minimized().expect("read minimized state"));

        for _ in 0..4 {
            super::focus_main_window(app.handle());
        }
        assert_eq!(app.webview_windows().len(), 1);
        assert!(app.get_webview_window(super::MAIN_WINDOW).is_some());
    }
}
