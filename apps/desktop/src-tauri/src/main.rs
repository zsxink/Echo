//! Echo desktop application binary.
//!
//! The binary owns only the Tauri application boundary. Domain work remains in
//! `echo-core`; desktop runtime, player and platform implementation grow in
//! `echo-desktop` in their respective tasks.
//!
//! Windows: the executable runs as a GUI-subsystem program, so no console
//! (the black window) is attached at startup. `println!`/`eprintln!` output
//! then goes nowhere visible, which is why all diagnostics route through the
//! rolling-file logger + panic hook installed in `main` (task 7.8). The
//! attribute is a no-op on macOS/Linux.

#![windows_subsystem = "windows"]

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
use echo_core::application::ports::SystemTrashPort;
#[cfg(not(target_os = "macos"))]
use echo_core::domain::state::PlaybackState;
use echo_desktop::platform::local_state::{CloseBehavior, DesktopStateStore};
use echo_desktop::platform::status_menu::StatusMenuSink;
#[cfg(not(target_os = "macos"))]
use echo_desktop::platform::status_menu::{self, PlaySummary};
use echo_desktop::platform::trash::{TrashCrateBackend, TrashWithBackend};
use echo_desktop::player::coordinator::PlaybackCoordinator;
use echo_desktop::player::port::{PlayerCommand, PlayerError, PlayerPort};
use echo_desktop::runtime::app::assemble;
use echo_desktop::runtime::player;
use echo_desktop::runtime::services::AppServices;
use echo_desktop::runtime::{trash_worker::TrashFinalizationWorker, StartupSupervisor};
#[cfg(not(target_os = "macos"))]
use tauri::menu::{MenuBuilder, MenuItemBuilder};
#[cfg(not(target_os = "macos"))]
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager, RunEvent};

#[cfg(any(target_os = "macos", test))]
mod app_menu;
mod commands;
mod commands_audio_effects;
mod cover_protocol;
mod dialogs;
#[cfg(target_os = "macos")]
mod macos_now_playing;
#[cfg(target_os = "macos")]
mod macos_status_row;
#[cfg(any(target_os = "macos", test))]
mod macos_window_lifecycle;
mod open_targets;
mod restore_retry;
mod sync_commands;
mod sync_library_selection;
mod sync_provisioner;
mod sync_runtime;

const MAIN_WINDOW: &str = "main";

#[cfg(any(target_os = "macos", test))]
use macos_window_lifecycle::PendingFullscreenHide;

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
    // Mutations span the whole match, so the coordinator guard is held to the
    // end of the command body on purpose — the same intent `commands.rs`
    // documents for its `significant_drop_tightening` allow.
    #[allow(clippy::significant_drop_tightening)]
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
            // App-menu additions (issue #73): the same coordinator semantics
            // the IPC command layer applies for seek/volume/mute/mode, so the
            // menu, the tray and the playback bar cannot drift apart.
            PlayerCommand::Seek(position) => {
                coordinator.seek(position.max(0.0));
                Ok(())
            }
            PlayerCommand::SetVolume(volume) => {
                coordinator.set_volume(volume.clamp(0.0, 1.0));
                Ok(())
            }
            PlayerCommand::ToggleMute => {
                coordinator.toggle_mute();
                Ok(())
            }
            PlayerCommand::SetMode(mode) => {
                coordinator.set_mode(mode);
                Ok(())
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

/// Resolve the bundled libmpv library for the current platform.
///
/// Every platform ships libmpv in the *running executable's* own directory —
/// not through Tauri's `executable_dir()`, which on macOS is unsupported
/// (`dirs::executable_dir` returns `None`), so it can never be used to locate
/// a sibling `Frameworks/`.
///
///   • macOS: release app has it in `Echo.app/Contents/Frameworks`; `build.rs`
///     stages the same dependency set in `target/Frameworks` for `tauri dev`.
///     Two levels up from the executable in each:
///       - dev:  target/debug/Echo  → ../..  → target/Frameworks
///       - .app: .../Contents/MacOS/Echo → ../.. → .../Contents/Frameworks
///   • Windows: NSIS installs the DLL set in the install root (`extraResources`,
///     task 3.4), same directory as the executable. The packaged name is
///     `libmpv-2.dll` (shinchiro ABI 2.5), not `libmpv-1.dll`.
///   • Linux: deb/AppImage place `libmpv.so` in the same directory as the
///     executable (`bundle.linux`, task 3.4); the real SONAME file is
///     `libmpv.so.<ver>`, but Echo loads the unversioned `libmpv.so` the
///     vendored tree offers as a symlink next to it.
#[allow(clippy::missing_const_for_fn)]
fn bundled_libmpv(_: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    #[cfg(target_os = "macos")]
    {
        let exe = std::env::current_exe().ok()?;
        let frameworks = exe.parent()?.parent()?.join("Frameworks");
        let candidate = frameworks.join("libmpv.dylib");
        candidate.exists().then_some(candidate)
    }
    #[cfg(target_os = "windows")]
    {
        let exe = std::env::current_exe().ok()?;
        let candidate = exe.parent()?.join("libmpv-2.dll");
        candidate.exists().then_some(candidate)
    }
    #[cfg(all(not(target_os = "macos"), not(target_os = "windows")))]
    {
        let exe = std::env::current_exe().ok()?;
        let candidate = exe.parent()?.join("libmpv.so");
        candidate.exists().then_some(candidate)
    }
}

fn focus_main_window<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    // Resolve the stable window label only. Status-item activation never
    // constructs a WebView, so rapid/repeated activation cannot produce a
    // second main window or a second playback composition.
    #[cfg(target_os = "macos")]
    if let Some(pending) = app.try_state::<Arc<PendingFullscreenHide>>() {
        pending.cancel();
    }
    if let Some(window) = app.get_webview_window(MAIN_WINDOW) {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}

/// Feed the macOS status row **and the application menu** from the player
/// port's authoritative snapshot stream — one stream, two surfaces, so the
/// compact row icons and the 播放/暂停 label + 播放模式 check states can never
/// disagree. `AppKit` and the menu are main-thread-only, so each update is
/// scheduled onto Tauri's main thread; a closed player is normal during
/// process shutdown. The playback mode lives on the coordinator (queue-owned
/// state), hence the second handle.
#[cfg(target_os = "macos")]
fn spawn_macos_status_row_snapshot_refresh(
    app: tauri::AppHandle,
    port: &Arc<dyn PlayerPort>,
    coordinator: SharedPlaybackCoordinator,
) {
    let initial = port.snapshot().state;
    let initial_mode = coordinator
        .lock()
        .map_or(echo_desktop::player::port::PlayMode::Sequential, |coord| {
            coord.mode()
        });
    let initial_app = app.clone();
    let _ = initial_app.run_on_main_thread(move || {
        macos_status_row::refresh_play_pause(initial);
        app_menu::refresh_playback_state(initial, initial_mode);
    });
    let snapshots = port.subscribe_snapshots();
    thread::spawn(move || {
        while let Ok(snapshot) = snapshots.recv() {
            let state = snapshot.state;
            let mode = coordinator
                .lock()
                .map_or(echo_desktop::player::port::PlayMode::Sequential, |coord| {
                    coord.mode()
                });
            let _ = app.run_on_main_thread(move || {
                macos_status_row::refresh_play_pause(state);
                app_menu::refresh_playback_state(state, mode);
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

fn retry_saved_sync_restore(runtime: &Arc<sync_runtime::SyncRuntime>) {
    if let Err(error) = runtime.start_restore_retry() {
        tracing::warn!(
            error = error.message_key(),
            "failed to start saved LAN sync retry worker"
        );
    }
}

/// Build `AppServices`, and register the playback coordinator + actor + theme
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
    echo_desktop::platform::sync_security::configure_local_vault(app_data.join("lan-sync-vault"))?;
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
    let trash: Arc<dyn SystemTrashPort> = Arc::new(TrashWithBackend::new(TrashCrateBackend::new(
        routed.registry.clone(),
    )));
    supervisor
        .run_recovery(&routed.deps, &scan_supervisor, trash.as_ref())
        .map_err(|error| format!("runtime recovery failed: {error}"))?;
    supervisor.on_ready();
    let trash_worker = TrashFinalizationWorker::start(routed.deps.clone(), Arc::clone(&trash))
        .map_err(|error| format!("trash finalization worker failed: {error}"))?;

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
        scan_supervisor.clone(),
        supervisor,
        gate_dialogs,
        routed.registry.clone(),
        routed.database.clone(),
        echo_core::application::root_switch::Blockers::new(),
    );
    app.manage(services);
    app.manage(routed.database.clone());
    app.manage(trash_worker);
    app.manage(routed.registry.clone());
    app.manage(routed.deps.clone());
    // Constructing this boundary performs no discovery, listening or requests.
    // A previously saved host/peer role is restored below after the status
    // notifier is installed, so startup can report progress to the UI.
    let native_sync_provisioner = Arc::new(sync_provisioner::NativeSyncProvisioner::new(
        routed.database.clone(),
        app_data.join("lan-resources"),
        echo_core::application::remote_resources::RemoteResourcePublishDeps {
            fs: routed.deps.fs.clone(),
            hasher: routed.deps.hasher.clone(),
            journal: routed.database.clone(),
        },
        routed.deps.clone(),
        scan_supervisor,
    ));
    let provisioner: Arc<dyn sync_runtime::SyncProvisioner> = native_sync_provisioner.clone();
    let cycle_port: Arc<dyn sync_runtime::SyncCyclePort> = native_sync_provisioner;
    let sync_runtime = Arc::new(sync_runtime::SyncRuntime::new(
        routed.database.clone(),
        Some(provisioner),
        Some(cycle_port),
    ));
    let sync_app = app.handle().clone();
    sync_runtime.set_status_notifier(Arc::new(move |status| {
        let _ = sync_app.emit("sync://status", status);
    }));
    if let Err(error) = sync_runtime.restore_saved_session() {
        tracing::warn!(
            error = error.message_key(),
            "failed to restore saved LAN sync session"
        );
        let status = sync_runtime.report_error(error);
        let _ = app.emit("sync://status", &status);
        retry_saved_sync_restore(&sync_runtime);
    }
    app.manage(Arc::clone(&sync_runtime));
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
        "bundled libmpv is missing; playback cannot start (run the platform build staging for this OS)".to_owned()
    })?;
    let controller = player::PlayerController::spawn_mpv(&libmpv, resolver)
        .map_err(|error| format!("start libmpv player: {error}"))?;
    let player_source: Arc<std::sync::Mutex<Option<String>>> =
        Arc::new(std::sync::Mutex::new(None));
    // Session persistence (task 8.9 落盘接线): the saver thread throttles
    // durable playback-session writes onto the atomic desktop-state store.
    let saver_persistence: Arc<dyn echo_desktop::player::session::SessionPersistence> = Arc::new(
        echo_desktop::player::session::StateStoreSession::new(local_state.clone()),
    );
    let effects = Arc::new(echo_desktop::effects_service::EffectsService::restore(
        local_state,
        Arc::new(
            echo_desktop::effects_service::player_adapter::PlayerEffectsAdapter::new(
                controller.port.clone(),
            ),
        ),
    ));
    app.manage(effects.clone());
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
    spawn_macos_status_row_snapshot_refresh(
        app.handle().clone(),
        &controller.port,
        controller.coordinator.clone(),
    );

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
    let emit: player::SnapshotEmitter = Box::new(move |mut ui| {
        ui.effects = Some(effects.snapshot().snapshot().into());
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
mod bootstrap;
mod file_open_gate;

#[allow(unused_imports)]
pub(crate) use file_open_gate::{deliver_file_open, drain_pending_opens};

fn main() {
    bootstrap::bootstrap_main();
}

#[cfg(test)]
#[path = "main_tests.rs"]
mod cover_canvas_tests;
