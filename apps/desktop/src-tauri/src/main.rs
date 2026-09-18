//! Echo desktop application binary.
//!
//! The binary owns only the Tauri application boundary. Domain work remains in
//! `echo-core`; desktop runtime, player and platform implementation grow in
//! `echo-desktop` in their respective tasks.

use std::{
    env,
    fs::OpenOptions,
    io::Write,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use echo_core::application::ports::CoverCache;
#[cfg(not(target_os = "macos"))]
use echo_core::domain::state::PlaybackState;
use echo_desktop::platform::local_state::{CloseBehavior, DesktopStateStore};
use echo_desktop::platform::security::{CoverError, CoverProtocol};
use echo_desktop::platform::status_menu::StatusMenuSink;
#[cfg(not(target_os = "macos"))]
use echo_desktop::platform::status_menu::{self, PlaySummary};
use echo_desktop::player::coordinator::PlaybackCoordinator;
use echo_desktop::player::port::{PlayerCommand, PlayerPort};
use echo_desktop::runtime::app::assemble;
use echo_desktop::runtime::player;
use echo_desktop::runtime::services::AppServices;
use echo_desktop::runtime::StartupSupervisor;
#[cfg(not(target_os = "macos"))]
use tauri::menu::{MenuBuilder, MenuItemBuilder};
#[cfg(not(target_os = "macos"))]
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{
    http::{Request as HttpRequest, Response as HttpResponse},
    Emitter, Manager, RunEvent,
};

mod commands;
mod dialogs;
#[cfg(target_os = "macos")]
mod macos_status_row;

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
    fn on_command(&self, command: PlayerCommand) {
        let Ok(slot) = self.coordinator.lock() else {
            return;
        };
        let Some(coordinator) = slot.as_ref() else {
            return;
        };
        let Ok(mut coordinator) = coordinator.lock() else {
            return;
        };
        match command {
            PlayerCommand::TogglePlayPause => {
                let _ = coordinator.player().send(PlayerCommand::TogglePlayPause);
            }
            PlayerCommand::Previous => coordinator.previous(),
            PlayerCommand::Next => {
                let _ = coordinator.advance_to_next();
            }
            _ => {}
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
fn bundled_libmpv(_app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    #[cfg(target_os = "macos")]
    {
        // `current_exe` is accurate in both layouts we ship/stage. `Frameworks`
        // sits two levels up from the executable in each — the rpath instrument
        // is `@executable_path/../Frameworks` (one `..` from the exe's *parent*
        // directory, matching `parent().parent()` here):
        //   • dev:  target/debug/echo  → ../..  → target/Frameworks (staged set)
        //   • .app: .../Contents/MacOS/echo → ../.. → .../Contents/Frameworks
        let exe = std::env::current_exe().ok()?;
        let frameworks = exe.parent()?.parent()?.join("Frameworks");
        let candidate = frameworks.join("libmpv.dylib");
        candidate.exists().then_some(candidate)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = _app;
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

/// Wire the composition root (task 7.3 / 10.2): assemble the real `ScanDeps`,
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
fn wire_composition(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
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
    let supervisor = StartupSupervisor::new();
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
    player::spawn_forwarder(
        controller.port.clone(),
        queue_provider,
        queue_metadata,
        emit,
    );
    // Auto-advance: a track reaching EOF (or failing to load) must drive the
    // coordinator to the next entry. Without this the queue stalls on `ended`
    // until the user presses next manually.
    player::spawn_auto_advance(controller.port.clone(), controller.coordinator.clone());
    // Playback statistics (task 8.10): feed the real snapshot stream into the
    // accumulator so a qualified listen reaches Core's idempotent
    // `record_playback` (play_count / 最近播放). Without this the accumulator
    // had no production caller and play counts never moved.
    let stats_sink = Arc::new(player::CorePlaybackRecorder::new(routed.database.clone()));
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
fn record_gate_open(paths: &[String]) {
    let Ok(log_path) = env::var("ECHO_GATE_OPEN_LOG") else {
        return;
    };
    let Ok(mut log) = OpenOptions::new().create(true).append(true).open(log_path) else {
        return;
    };
    for path in paths {
        let _ = writeln!(log, "{path}");
    }
}

/// The shell→frontend event carrying one accepted file-open path (task 9.1).
/// The frontend bridge subscribes to this; the payload is a single absolute
/// path that was either delivered immediately or drained from the pre-ready
/// FIFO. The event name also appears in `echo-desktop::ipc::events`.
const FILE_OPEN_REQUEST: &str = "app://file-open-request";

/// Route one OS file-open path through the startup FIFO (task 9.1).
///
/// While the runtime is still initializing — the supervisor is not yet
/// `Ready` — the path is stashed in the bounded FIFO and later drained by
/// [`drain_pending_opens`], never silently dropped. Once ready, the path is
/// delivered to the frontend immediately. A racing second launch before the
/// supervisor is registered focuses the window and records for the Gate rather
/// than panicking.
fn deliver_file_open(app: &tauri::AppHandle, path: String) {
    let Some(supervisor) = app.try_state::<StartupSupervisor>() else {
        focus_main_window(app);
        record_gate_open(&[path]);
        return;
    };
    // `receive_file_open` returns `Some` only when the runtime is already
    // ready; a `None` means the path was queued for the `on_ready()` drain.
    if let Some(ready_path) = supervisor.receive_file_open(path) {
        record_gate_open(std::slice::from_ref(&ready_path));
        let _ = app.emit(FILE_OPEN_REQUEST, vec![ready_path]);
    }
}

/// Signal that the shell finished initializing and drain every file-open that
/// arrived during startup, delivering each to the frontend (task 9.1). Paths
/// received before ready are never dropped — they are all emitted here.
fn drain_pending_opens(app: &tauri::AppHandle) {
    let Some(supervisor) = app.try_state::<StartupSupervisor>() else {
        return;
    };
    let drained = supervisor.on_ready();
    if !drained.is_empty() {
        record_gate_open(&drained);
        let _ = app.emit(FILE_OPEN_REQUEST, drained);
    }
}

/// The `cover://` asset protocol (design §16): serves cover art bytes from the
/// [`CoverCache`] by opaque asset key only.
///
/// Every security decision lives in [`CoverProtocol`] (echo-desktop):
///   - the URI must be `cover://` and its key must pass the shape whitelist,
///     so a request can never be turned into a filesystem path;
///   - the bytes come exclusively from `cache.get(key)` — never from a
///     client-supplied path.
///
/// When no [`CoverCache`] is registered yet (the runtime has not wired the
/// cover store), the protocol answers [`http::StatusCode::NOT_FOUND`] rather
/// than erroring — the shell stays thin and the protocol is inert until the
/// runtime injects the store.
/// `context`/`request` are consumed by the framework signature (the trait bound
/// fixes them by value); the handler only borrows them, hence the allow.
#[allow(clippy::needless_pass_by_value)]
fn cover_protocol_handler<R: tauri::Runtime>(
    context: tauri::UriSchemeContext<'_, R>,
    request: HttpRequest<Vec<u8>>,
) -> HttpResponse<Vec<u8>> {
    let key = match CoverProtocol::parse(request.uri().to_string().as_str()) {
        Ok(key) => key,
        Err(CoverError::KeyTooLong) => {
            return HttpResponse::builder()
                .status(400)
                .body(vec![])
                .unwrap_or_default();
        }
        Err(_) => {
            // Wrong scheme, missing or malformed key — the request is not for
            // us, answer 404 (never a redirect or a fallthrough read).
            return HttpResponse::builder()
                .status(404)
                .body(vec![])
                .unwrap_or_default();
        }
    };

    let Some(cache) = context.app_handle().try_state::<Arc<dyn CoverCache>>() else {
        return HttpResponse::builder()
            .status(404)
            .body(vec![])
            .unwrap_or_default();
    };

    match cache.get(&key) {
        Ok(Some(bytes)) => {
            let media_type = cover_media_type(&bytes);
            let mut response = HttpResponse::builder()
                .status(200)
                .header("Content-Type", media_type)
                .header("Vary", "Origin");
            // Canvas extraction needs an origin-clean image. Grant pixel reads
            // only to the bundled renderer (and, in a dev build, the local dev
            // server — whatever port it picked).
            if let Some(origin) = request
                .headers()
                .get("Origin")
                .and_then(|v| v.to_str().ok())
            {
                if cover_canvas_origin_allowed(origin) {
                    response = response.header("Access-Control-Allow-Origin", origin);
                } else if cfg!(debug_assertions) {
                    // A refused pixel read is otherwise completely silent: the
                    // probe's `crossOrigin` request fails, the palette resolves
                    // to `null`, and the immersive surface falls back to the
                    // theme colour for *every* song while the DOM cover still
                    // renders. This line is the only place the refusal is
                    // observable, so make it observable on purpose.
                    eprintln!("cover: refused pixel access to origin {origin}");
                }
            }
            response.body(bytes).unwrap_or_default()
        }
        // Unknown/malformed key for the store, or a transient storage miss —
        // same 404 as any missing asset.
        Ok(None) | Err(_) => HttpResponse::builder()
            .status(404)
            .body(vec![])
            .unwrap_or_default(),
    }
}

/// The origins allowed to read cover **pixels** (an origin-clean `<img>` plus a
/// canvas read-back).
///
/// The bundled renderer has one fixed origin per platform — see the three
/// literals below. A dev build does not: with no `devUrl` configured, `tauri
/// dev` serves `frontendDist` from its own static server, which picks its port
/// at runtime (1430, or the next free one). Naming a single dev port here made
/// the whitelist stop matching the moment the CLI chose another one — and that
/// failure is invisible end to end: the probe's `crossOrigin` request gets no
/// `Access-Control-Allow-Origin`, the palette resolves to `null`, and the
/// immersive surface falls back to the theme colour for *every* song, while the
/// DOM cover (which needs no CORS) keeps rendering normally.
///
/// A dev build therefore accepts any loopback origin rather than a guessed
/// port. Release builds keep the fixed, enumerated set.
fn cover_canvas_origin_allowed(origin: &str) -> bool {
    if matches!(
        origin,
        "tauri://localhost" | "http://tauri.localhost" | "https://tauri.localhost"
    ) {
        return true;
    }
    cfg!(debug_assertions) && is_loopback_http_origin(origin)
}

/// `http://<loopback>:<port>` — the shape of a local dev server's origin.
///
/// The port is required, and the host must be loopback: `Origin: null` (a
/// sandboxed or `data:` document), `http://localhost` with no port, and
/// `http://localhost.example.com` all parse as URLs but none of them is the
/// renderer.
fn is_loopback_http_origin(origin: &str) -> bool {
    tauri::Url::parse(origin).is_ok_and(|url| {
        url.scheme() == "http"
            && url.port().is_some()
            && matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
    })
}

/// The media type of an embedded cover, derived from the bytes themselves.
///
/// The cache stores the original `mime.txt` next to the bytes, but the read port
/// resolves bytes only — and a wildcard `image/*` is not a concrete media type,
/// so `WebKit` will not decode the response into an `<img>`. The protocol
/// therefore sniffs the container magic of the bytes it is about to serve. An
/// unrecognised container is still served (as `application/octet-stream`) rather
/// than hidden: a candidate that fails to decode is a local-library fact, not
/// something the shell should pretend is missing.
fn cover_media_type(bytes: &[u8]) -> &'static str {
    match bytes {
        [0xFF, 0xD8, 0xFF, ..] => "image/jpeg",
        [0x89, b'P', b'N', b'G', ..] => "image/png",
        [b'G', b'I', b'F', ..] => "image/gif",
        [b'B', b'M', ..] => "image/bmp",
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => "image/webp",
        _ => "application/octet-stream",
    }
}

#[allow(clippy::too_many_lines)]
fn main() {
    let app = match tauri::Builder::default()
        // The single-instance plugin is registered first, as required by the
        // plugin, so a later platform adapter cannot create a second process.
        .register_uri_scheme_protocol("cover", cover_protocol_handler)
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            // A second launch (or a file-open) on a running instance: wake the
            // main window, then forward each path exactly once through the
            // startup FIFO (task 9.1). `skip(1)` drops the argv program name;
            // macOS/Windows/Linux argv unification is task 9.2.
            focus_main_window(app);
            for path in args.into_iter().skip(1) {
                deliver_file_open(app, path);
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
            commands::player_control,
            commands::queue_command,
            commands::set_volume,
            commands::toggle_mute,
            commands::seek,
        ])
        .setup(|app| {
            // Register the startup supervisor first so any OS file-open that
            // arrives while the shell is still initializing is queued in the
            // FIFO rather than dropped (task 9.1).
            app.manage(StartupSupervisor::new());

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
            app.manage(status_sink.clone());

            #[cfg(target_os = "macos")]
            {
                let show_handle = app.handle().clone();
                let quit_handle = app.handle().clone();
                macos_status_row::install(
                    status_sink,
                    Arc::new(move || focus_main_window(&show_handle)),
                    Arc::new(move || {
                        if let Some(player) = quit_handle.try_state::<commands::PlayerHandle>() {
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
                let previous =
                    MenuItemBuilder::with_id(status_menu::MENU_PREVIOUS, "上一首").build(app)?;
                let next = MenuItemBuilder::with_id(status_menu::MENU_NEXT, "下一首").build(app)?;
                let show =
                    MenuItemBuilder::with_id(status_menu::MENU_SHOW, "显示 Echo").build(app)?;
                let quit = MenuItemBuilder::with_id(status_menu::MENU_QUIT, "退出").build(app)?;
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
                                if let Some(player) = app.try_state::<commands::PlayerHandle>() {
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
                                        sink.on_command(command);
                                    }
                                }
                            }
                        }
                    })
                    .build(app)?;
            }

            // -----------------------------------------------------------------
            wire_composition(app)?;

            schedule_gate_exit(app.handle().clone());

            // The shell is fully initialized: mark the runtime ready and drain
            // any file-opens received during setup, delivered in arrival order
            // and never silently dropped (task 9.1).
            drain_pending_opens(app.handle());
            Ok(())
        })
        .build(tauri::generate_context!())
    {
        Ok(app) => app,
        Err(error) => {
            eprintln!("failed to initialize Echo: {error}");
            std::process::exit(1);
        }
    };

    app.run(|app, event| match event {
        // macOS delivers file-association opens through `RunEvent::Opened`; they
        // go through the same FIFO as the single-instance argv path (task 9.1).
        RunEvent::Opened { urls } => {
            focus_main_window(app);
            for url in urls {
                deliver_file_open(app, url.to_string());
            }
        }
        // Clicking the macOS Dock icon emits `Reopen`, rather than a tray-icon
        // event. Restore the hidden player window only when no window is visible.
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
    });
}

// Kept last: `clippy::items_after_test_module` requires a `#[cfg(test)]` module
// to be the final item in the file, and `cover_media_type`/`main` below read
// better next to the protocol handler they belong to.
#[cfg(test)]
mod cover_canvas_tests {
    #[test]
    fn only_echo_renderer_origins_can_read_cover_pixels() {
        for origin in [
            "tauri://localhost",
            "http://tauri.localhost",
            "https://tauri.localhost",
        ] {
            assert!(super::cover_canvas_origin_allowed(origin));
        }
        for origin in [
            "null",
            "https://example.com",
            "http://tauri.localhost.evil.com",
            "http://localhost.evil.com:1430",
            "http://192.168.1.10:1430",
            "file:///tmp/cover.html",
        ] {
            assert!(!super::cover_canvas_origin_allowed(origin), "{origin}");
        }
    }

    /// The port of the dev server is chosen by the CLI at runtime, so the
    /// renderer's dev origin is *not* a constant. Pinning one port — this suite
    /// used to pin 1420, while `tauri dev` served 1430 — is precisely how the
    /// immersive palette came to fall back to the theme for every song: the
    /// whitelist stopped matching and nothing anywhere said so.
    #[test]
    fn a_dev_build_accepts_whatever_loopback_port_the_renderer_is_served_from() {
        for origin in [
            "http://localhost:1420",
            "http://localhost:1430",
            "http://localhost:1431",
            "http://127.0.0.1:1430",
        ] {
            assert!(super::is_loopback_http_origin(origin), "{origin}");
            assert_eq!(
                super::cover_canvas_origin_allowed(origin),
                cfg!(debug_assertions),
                "{origin}"
            );
        }
        // Loopback-shaped but not a renderer: no port, not http, not loopback.
        for origin in [
            "http://localhost",
            "http://127.0.0.1",
            "https://localhost:1430",
            "ftp://localhost:1430",
            "localhost:1430",
        ] {
            assert!(!super::is_loopback_http_origin(origin), "{origin}");
        }
    }

    #[test]
    fn main_window_activation_restores_one_existing_window_without_creating_another() {
        use tauri::Manager;

        let app = tauri::test::mock_app();
        let main = tauri::WebviewWindowBuilder::new(&app, super::MAIN_WINDOW, Default::default())
            .build()
            .expect("mock main window");

        main.hide().expect("hide mock window");
        super::focus_main_window(&app.handle());
        assert!(main.is_visible().expect("read visible state"));

        main.minimize().expect("minimize mock window");
        super::focus_main_window(&app.handle());
        assert!(!main.is_minimized().expect("read minimized state"));

        for _ in 0..4 {
            super::focus_main_window(&app.handle());
        }
        assert_eq!(app.webview_windows().len(), 1);
        assert!(app.get_webview_window(super::MAIN_WINDOW).is_some());
    }
}
