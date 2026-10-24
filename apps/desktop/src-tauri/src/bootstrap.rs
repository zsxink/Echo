//! `fn main()` — application bootstrap and Tauri wiring.

#[allow(clippy::wildcard_imports, unused_imports)]
use super::*;

use super::file_open_gate::{deliver_file_open, schedule_gate_exit};

/// Crate entry: `main.rs` delegates here.
#[allow(clippy::too_many_lines)]
#[allow(clippy::redundant_pub_crate)]
pub(super) fn bootstrap_main() {
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

    let builder = tauri::Builder::default()
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
            sync_commands::sync_status,
            sync_commands::sync_storage_space,
            sync_commands::sync_conflicts,
            sync_commands::resolve_sync_conflict,
            sync_commands::recover_restored_host,
            sync_commands::sync_authorized_peers,
            sync_commands::local_lan_addresses,
            sync_commands::host_start,
            sync_commands::host_stop,
            sync_commands::sync_leave,
            sync_commands::sync_peer_start,
            sync_commands::sync_pause,
            sync_commands::sync_resume,
            sync_commands::sync_now,
            sync_commands::sync_restore_retry,
            sync_commands::revoke_device,
            sync_commands::discover_hosts,
            sync_commands::create_sync_pairing_code,
            sync_commands::request_sync_pairing,
            commands_audio_effects::get_audio_effects_snapshot,
            commands_audio_effects::select_audio_effects_preset,
            commands_audio_effects::edit_audio_equalizer,
            commands_audio_effects::set_audio_effects_enabled,
            commands_audio_effects::reset_audio_effects,
            commands_audio_effects::save_audio_effects_preset,
            commands_audio_effects::rename_audio_effects_preset,
            commands_audio_effects::delete_audio_effects_preset,
            commands_audio_effects::retry_audio_effects,
            commands::get_bootstrap_state,
            commands::library_status,
            commands::all_songs,
            commands::search,
            commands::favorites,
            commands::recent,
            commands::catalog_collections,
            commands::catalog_collection_songs,
            commands::set_artist_cover,
            commands::library_counts,
            commands::playlists,
            commands::playlist_members,
            commands::playlists_for_song,
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
            sync_library_selection::choose_library_root,
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
            commands::report_library_view,
        ]);

    // The Chinese application menu replaces the Tauri default *before* the
    // first window appears, so no English default menu ever flashes. macOS
    // only — Windows/Linux keep their tray-only entry unchanged.
    #[cfg(target_os = "macos")]
    let builder = builder
        .menu(app_menu::build_app_menu)
        .on_menu_event(|app, event| {
            app_menu::handle_menu_event(app, &event);
        });

    let app = match builder
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
                    let menu_handle = app.handle().clone();
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
                        // Right-click 导入/设置 reuse the app-menu decision
                        // layer, so the context menu and the menu bar can never
                        // diverge into two code paths.
                        Arc::new(move |action| app_menu::dispatch_by_id(&menu_handle, action)),
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

                    // Embed the tray-sized PNG directly rather than depending on
                    // Windows' default window icon/ICO resource selection.
                    let tray_icon = tauri::include_image!("icons/32x32.png");
                    TrayIconBuilder::with_id("echo-gate")
                        .icon(tray_icon)
                        .tooltip("Echo")
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

    #[cfg(target_os = "macos")]
    let pending_fullscreen_hide = Arc::new(PendingFullscreenHide::default());
    #[cfg(target_os = "macos")]
    app.manage(Arc::clone(&pending_fullscreen_hide));

    app.run({
        // Only the macOS `RunEvent::Opened` arm routes through the startup
        // supervisor; the closed-over `startup` (line 683) is enough for other
        // platforms, where a shadow would otherwise be an unused variable.
        #[cfg(target_os = "macos")]
        let startup = Arc::clone(&startup);
        #[cfg(target_os = "macos")]
        let pending_fullscreen_hide = Arc::clone(&pending_fullscreen_hide);
        move |app, event| match event {
            #[cfg(target_os = "macos")]
            RunEvent::Ready => {
                // Tauri creates configured windows during setup(), which runs
                // immediately before Ready. The window does not exist at build().
                if let Err(error) = macos_window_lifecycle::install(
                    app,
                    Arc::clone(&pending_fullscreen_hide),
                    MAIN_WINDOW,
                ) {
                    tracing::error!(%error, "failed to observe native fullscreen exit");
                    app.exit(1);
                }
            }
            RunEvent::ExitRequested { .. } => {
                if let Some(sync) = app.try_state::<Arc<sync_runtime::SyncRuntime>>() {
                    let _ = sync.stop();
                }
                commands_audio_effects::flush(app);
                #[cfg(target_os = "macos")]
                macos_now_playing::clear();
                #[cfg(target_os = "macos")]
                macos_window_lifecycle::unregister();
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
                pending_fullscreen_hide.cancel();
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
                        #[cfg(target_os = "macos")]
                        match window.is_fullscreen() {
                            Ok(true) => {
                                tracing::debug!("background close requested while main window is fullscreen");
                                pending_fullscreen_hide.begin();
                                if let Err(error) = window.set_fullscreen(false) {
                                    pending_fullscreen_hide.abort_exit();
                                    tracing::warn!(%error, "failed to exit fullscreen before hiding the main window");
                                }
                            }
                            Ok(false) => {
                                tracing::debug!(native_exit_in_progress = pending_fullscreen_hide.is_exiting(), pending = pending_fullscreen_hide.is_pending(), "background close requested while Tao reports windowed");
                                if pending_fullscreen_hide.is_exiting() {
                                    pending_fullscreen_hide.begin();
                                } else if let Err(error) = window.hide() {
                                    tracing::warn!(%error, "failed to hide the main window after close");
                                } else {
                                    pending_fullscreen_hide.cancel();
                                    tracing::debug!("hid windowed main window after background close");
                                }
                            }
                            Err(error) => {
                                tracing::warn!(%error, "failed to read fullscreen state before hiding the main window");
                            }
                        }
                        #[cfg(not(target_os = "macos"))]
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
