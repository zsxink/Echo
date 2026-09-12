//! Echo desktop application binary.
//!
//! The binary owns only the Tauri application boundary. Domain work remains in
//! `echo-core`; desktop runtime, player and platform implementation grow in
//! `echo-desktop` in their respective tasks.

use std::{env, fs::OpenOptions, io::Write, sync::Arc, thread, time::Duration};

use echo_core::application::ports::CoverCache;
use echo_core::domain::state::PlaybackState;
use echo_desktop::platform::local_state::DesktopStateStore;
use echo_desktop::platform::security::{CoverError, CoverProtocol};
use echo_desktop::platform::status_menu::{self, NoopSink, PlaySummary, StatusMenuSink};
use echo_desktop::runtime::app::assemble;
use echo_desktop::runtime::player;
use echo_desktop::runtime::services::AppServices;
use echo_desktop::runtime::StartupSupervisor;
use tauri::{
    http::{Request as HttpRequest, Response as HttpResponse},
    menu::{MenuBuilder, MenuItemBuilder},
    tray::TrayIconBuilder,
    Emitter, Manager, RunEvent,
};

mod commands;

const MAIN_WINDOW: &str = "main";

/// The event name carrying a `UiPlayerSnapshot` to the frontend (`playerStore`
/// subscribes via the bridge). Matches the `ipc::events` `player://snapshot`
/// design and the `BridgeCommandMap` consumer.
const PLAYER_SNAPSHOT_EVENT: &str = "player://snapshot";

/// Resolve the bundled libmpv dylib for the current platform. On macOS it is
/// vendored into the app bundle's `Frameworks/` (see `build.rs` rpath +
/// `tauri.conf.json`). Other platforms have not been vendored yet (deferred to
/// their platform Gate); a missing dylib leaves the actor degraded (`Stopped`).
fn bundled_libmpv(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    #[cfg(target_os = "macos")]
    {
        // …/Echo.app/Contents/MacOS → …/Echo.app/Contents/Frameworks/libmpv.dylib
        let executable_dir = app.path().executable_dir().ok()?;
        let frameworks = executable_dir.parent()?.join("Frameworks");
        let candidate = frameworks.join("libmpv.dylib");
        candidate.exists().then_some(candidate)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        None
    }
}

fn focus_main_window(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Wire the composition root (task 7.3 / 10.2): assemble the real `ScanDeps`,
/// build `AppServices`, and register the playback coordinator + actor + theme
/// store as managed state, plus the snapshot→frontend forwarder.
///
/// Failure here is fatal (the app cannot serve commands without its runtime).
/// Uses `AppServices::new` (cancelling dialogs, empty runtime state) — the real
/// dialog/trash ports are shell-owned and wired in a later platform task.
///
/// # Errors
///
/// `Io`/`Storage` when the database or cover cache cannot open, or `String` on
/// assembly / recovery failure; propagated as a fatal startup error.
#[allow(clippy::too_many_lines)]
fn wire_composition(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let app_data = app.path().app_data_dir()?;
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

    let services = AppServices::new(routed.deps.clone(), scan_supervisor, supervisor);
    app.manage(services);
    app.manage(routed.registry.clone());
    app.manage(routed.deps.clone());

    // Theme / close-behavior persistence (task 7.6).
    let platform_default = if cfg!(target_os = "macos") {
        echo_desktop::platform::local_state::PlatformCloseDefault::Macos
    } else {
        echo_desktop::platform::local_state::PlatformCloseDefault::Other
    };
    let local_state = DesktopStateStore::new(app_data.join("desktop-state.json"), platform_default);
    app.manage(local_state);

    // Playback: spawn the libmpv actor, build the coordinator, and keep a
    // handle the command layer drives. `bundled_libmpv` is `None` on platforms
    // without vendored libmpv — the actor starts degraded.
    let resolver = player::PlayerController::resolver(&routed.deps);
    let controller = bundled_libmpv(app.handle()).map_or_else(
        || player::PlayerController::over_fake(echo_desktop::player::fake::FakePlayer::new()),
        |libmpv| {
            player::PlayerController::spawn_mpv(&libmpv, resolver).unwrap_or_else(|_| {
                eprintln!("libmpv unavailable; player degraded");
                player::PlayerController::over_fake(echo_desktop::player::fake::FakePlayer::new())
            })
        },
    );
    let player_handle = commands::PlayerHandle {
        coordinator: controller.coordinator.clone(),
    };
    app.manage(player_handle);

    // Forward actor snapshots to the frontend as typed UI snapshots. The queue
    // provider surfaces the coordinator's full queue (entries + per-round failed
    // set + current entry) so the UI snapshot drives the queue panel (11.2).
    let handle = app.handle().clone();
    let coordinator = controller.coordinator.clone();
    let queue_provider: Arc<dyn Fn() -> player::QueueView + Send + Sync> = Arc::new(move || {
        let coord = coordinator.lock().expect("player coordinator lock");
        (
            coord.queue().entries().to_vec(),
            coord.failed_round().collect(),
            coord.current().cloned(),
        )
    });
    let emit: player::SnapshotEmitter = Box::new(move |ui| {
        let _ = handle.emit(PLAYER_SNAPSHOT_EVENT, ui);
    });
    player::spawn_forwarder(controller.port.clone(), queue_provider, emit);
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
        Ok(Some(bytes)) => HttpResponse::builder()
            .status(200)
            .header("Content-Type", "image/*")
            .body(bytes)
            .unwrap_or_default(),
        // Unknown/malformed key for the store, or a transient storage miss —
        // same 404 as any missing asset.
        Ok(None) | Err(_) => HttpResponse::builder()
            .status(404)
            .body(vec![])
            .unwrap_or_default(),
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
        .invoke_handler(tauri::generate_handler![
            commands::get_bootstrap_state,
            commands::library_status,
            commands::all_songs,
            commands::search,
            commands::favorites,
            commands::recent,
            commands::playlists,
            commands::playlist_members,
            commands::song_detail,
            commands::get_lyrics,
            commands::set_favorite,
            commands::create_playlist,
            commands::rename_playlist,
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
            commands::play_context,
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
            app.manage::<Arc<dyn StatusMenuSink>>(Arc::new(NoopSink) as Arc<dyn StatusMenuSink>);

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
            let show = MenuItemBuilder::with_id(status_menu::MENU_SHOW, "显示 Echo").build(app)?;
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
                .on_menu_event(|app, event| {
                    match event.id().as_ref() {
                        status_menu::MENU_SHOW => focus_main_window(app),
                        status_menu::MENU_QUIT => app.exit(0),
                        id => {
                            // Transport item: forward the coarse player command
                            // to the playback sink. The composition root
                            // installs the coordinator forwarder; `NoopSink`
                            // remains until then.
                            if let Some(command) = status_menu::transport_command(id) {
                                if let Some(sink) = app.try_state::<Arc<dyn StatusMenuSink>>() {
                                    sink.on_command(command);
                                }
                            }
                        }
                    }
                })
                .build(app)?;

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

    app.run(|app, event| {
        // macOS delivers file-association opens through `RunEvent::Opened`; they
        // go through the same FIFO as the single-instance argv path (task 9.1).
        if let RunEvent::Opened { urls } = event {
            focus_main_window(app);
            for url in urls {
                deliver_file_open(app, url.to_string());
            }
        }
    });
}
