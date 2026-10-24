//! macOS native application menu (issue #73).
//!
//! The Tauri default menu is English and carries no Echo capability. This
//! module replaces it with a Chinese menu covering 导入 / 选择资料库 /
//! 重新扫描 / 设置 / 播放控制 / 播放模式 / 视图切换 / 搜索 / 帮助, in the
//! same decision/execution split as `status_menu`: the semantic item ids and
//! the id → action mapping below are pure and unit-tested, while the shell
//! builds the real `Menu` and dispatches.
//!
//! Execution paths, one per action class:
//! - 播放类 share the tray/status-row authority chain (`StatusMenuSink` →
//!   `PlaybackCoordinator`), including the relative seek/volume items which
//!   are resolved to absolute `PlayerCommand`s from the coordinator snapshot.
//! - 导航/导入/搜索/设置 are emitted as `menu://action` payloads and handled
//!   by the frontend through the same `bridge.subscribe` channel as every
//!   other shell → UI event.
//! - 退出/帮助链接 are shell lifecycle concerns handled here directly.
//!
//! Windows/Linux compile the pure mapping (so its tests run everywhere) but
//! never build a menu — the tray keeps its existing structure untouched.

use echo_desktop::platform::status_menu;
use echo_desktop::player::port::{PlayMode, PlayerCommand};

/// Event carrying a menu-driven frontend action (`App.tsx` subscribes via the
/// bridge). Payload shape: `{ "action": "import" | "settings" | … }`.
pub const MENU_ACTION_EVENT: &str = "menu://action";

// --- 文件 ---------------------------------------------------------------------
/// Semantic id of the 导入音乐文件… item — same flow as the brand-area import.
pub const MENU_IMPORT: &str = "import";
/// Semantic id of the 选择资料库目录… item.
pub const MENU_CHOOSE_LIBRARY_ROOT: &str = "choose_library_root";
/// Semantic id of the 重新扫描资料库 item.
pub const MENU_RESCAN: &str = "rescan";

// --- 播放（播放/暂停、上一首、下一首、退出复用 status_menu 的语义 id）----------
/// Semantic id of the 快退 5 秒 item.
pub const MENU_SEEK_BACKWARD: &str = "seek_backward";
/// Semantic id of the 快进 5 秒 item.
pub const MENU_SEEK_FORWARD: &str = "seek_forward";
/// Semantic id of the 音量增 item.
pub const MENU_VOLUME_UP: &str = "volume_up";
/// Semantic id of the 音量减 item.
pub const MENU_VOLUME_DOWN: &str = "volume_down";
/// Semantic id of the 静音 item.
pub const MENU_MUTE: &str = "mute";
/// Semantic id of the 顺序播放 mode item.
pub const MENU_MODE_SEQUENTIAL: &str = "mode_sequential";
/// Semantic id of the 随机播放 mode item.
pub const MENU_MODE_SHUFFLE: &str = "mode_shuffle";
/// Semantic id of the 单曲循环 mode item.
pub const MENU_MODE_REPEAT_ONE: &str = "mode_repeat_one";

// --- 视图 ---------------------------------------------------------------------
/// Semantic id of the 全部歌曲 view item.
pub const MENU_VIEW_ALL: &str = "view_all";
/// Semantic id of the 最近添加 view item.
pub const MENU_VIEW_RECENT: &str = "view_recent";
/// Semantic id of the 喜欢的音乐 view item.
pub const MENU_VIEW_FAVORITES: &str = "view_favorites";
/// Semantic id of the 歌手 view item.
pub const MENU_VIEW_ARTISTS: &str = "view_artists";
/// Semantic id of the 专辑 view item.
pub const MENU_VIEW_ALBUMS: &str = "view_albums";
/// Semantic id of the 歌单 view item.
pub const MENU_VIEW_PLAYLISTS: &str = "view_playlists";
/// Semantic id of the 搜索 item — focuses the workspace search box.
pub const MENU_SEARCH: &str = "search";

// --- Echo / 帮助 ---------------------------------------------------------------
/// Semantic id of the 设置… item.
pub const MENU_SETTINGS: &str = "settings";
/// Semantic id of the GitHub 仓库 item.
pub const MENU_HELP_REPOSITORY: &str = "help_repository";
/// Semantic id of the 报告问题 item.
pub const MENU_HELP_ISSUES: &str = "help_issues";

/// Relative seek step in seconds — the same magnitude the workspace hotkeys
/// (←/→) use, so both entrances advance playback identically.
pub const SEEK_STEP_SECS: f64 = 5.0;
/// Relative volume step (0.0–1.0 scale), matching the ↑/↓ hotkeys.
pub const VOLUME_STEP: f64 = 0.05;

/// A workspace view the menu can select. Payloads are exactly the frontend
/// `LibraryViewKind` values, so the mapping cannot drift from the sidebar.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LibraryView {
    /// 全部歌曲.
    AllSongs,
    /// 最近添加.
    Recent,
    /// 喜欢的音乐.
    Favorites,
    /// 歌手.
    Artists,
    /// 专辑.
    Albums,
    /// 歌单（目录）.
    Playlists,
}

impl LibraryView {
    /// The payload the frontend `selectView` accepts.
    #[must_use]
    pub const fn payload(self) -> &'static str {
        match self {
            Self::AllSongs => "all",
            Self::Recent => "recent",
            Self::Favorites => "favorites",
            Self::Artists => "artists",
            Self::Albums => "albums",
            Self::Playlists => "playlist",
        }
    }

    /// Parse a frontend-reported view kind; unknown values are ignored.
    #[must_use]
    pub fn from_payload(payload: &str) -> Option<Self> {
        match payload {
            "all" => Some(Self::AllSongs),
            "recent" => Some(Self::Recent),
            "favorites" => Some(Self::Favorites),
            "artists" => Some(Self::Artists),
            "albums" => Some(Self::Albums),
            "playlist" => Some(Self::Playlists),
            _ => None,
        }
    }
}

/// A menu action the frontend executes via the `menu://action` event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrontendAction {
    /// 导入音乐文件… — the brand-area import flow.
    Import,
    /// 选择资料库目录… — the native directory picker + activation flow.
    ChooseLibraryRoot,
    /// 重新扫描资料库 — rescan the active root.
    Rescan,
    /// 设置… — open the settings layer.
    Settings,
    /// 搜索 — focus the workspace search box.
    Search,
    /// Switch the workspace view.
    View(LibraryView),
}

impl FrontendAction {
    /// The wire payload; `view:*` suffixes reuse [`LibraryView::payload`].
    #[must_use]
    pub fn payload(self) -> String {
        match self {
            Self::Import => "import".to_owned(),
            Self::ChooseLibraryRoot => "choose_library_root".to_owned(),
            Self::Rescan => "rescan".to_owned(),
            Self::Settings => "settings".to_owned(),
            Self::Search => "search".to_owned(),
            Self::View(view) => format!("view:{}", view.payload()),
        }
    }
}

/// A playback action. Seek/volume stay relative here because their absolute
/// resolution needs the live coordinator snapshot; the shell performs that
/// resolution before handing a concrete [`PlayerCommand`] to the sink.
#[derive(Clone, Debug, PartialEq)]
pub enum PlaybackAction {
    /// A directly sendable player command.
    Command(PlayerCommand),
    /// Seek by a relative delta in seconds.
    SeekBy(f64),
    /// Change volume by a relative delta on the 0.0–1.0 scale.
    VolumeBy(f64),
}

/// A shell-lifecycle action handled without the `WebView`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ShellAction {
    /// 退出 Echo — flush the playback session, then exit.
    Quit,
    /// Open the GitHub repository page.
    HelpRepository,
    /// Open the GitHub issue tracker.
    HelpIssues,
}

/// The three-way classification every menu item id maps into.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuAction {
    /// Player-authority action (tray/status row share the same channel).
    Playback(PlaybackAction),
    /// Frontend action delivered through `menu://action`.
    Frontend(FrontendAction),
    /// Shell-lifecycle action.
    Shell(ShellAction),
}

/// Map a clicked item's semantic id to its action. Predefined system items
/// (About/Services/编辑剪贴板/最小化/缩放/关闭窗口/全屏/隐藏) never route
/// through here — the OS handles them — and unknown ids are `None`.
#[must_use]
pub fn menu_action(id: &str) -> Option<MenuAction> {
    let action = match id {
        // 播放：与托盘/状态项共用语义 id 与同一条 sink 通道。
        status_menu::MENU_PLAY_PAUSE => {
            MenuAction::Playback(PlaybackAction::Command(PlayerCommand::TogglePlayPause))
        }
        status_menu::MENU_PREVIOUS => {
            MenuAction::Playback(PlaybackAction::Command(PlayerCommand::Previous))
        }
        status_menu::MENU_NEXT => {
            MenuAction::Playback(PlaybackAction::Command(PlayerCommand::Next))
        }
        MENU_MUTE => MenuAction::Playback(PlaybackAction::Command(PlayerCommand::ToggleMute)),
        MENU_MODE_SEQUENTIAL => MenuAction::Playback(PlaybackAction::Command(
            PlayerCommand::SetMode(PlayMode::Sequential),
        )),
        MENU_MODE_SHUFFLE => MenuAction::Playback(PlaybackAction::Command(PlayerCommand::SetMode(
            PlayMode::Shuffle,
        ))),
        MENU_MODE_REPEAT_ONE => MenuAction::Playback(PlaybackAction::Command(
            PlayerCommand::SetMode(PlayMode::RepeatOne),
        )),
        MENU_SEEK_BACKWARD => MenuAction::Playback(PlaybackAction::SeekBy(-SEEK_STEP_SECS)),
        MENU_SEEK_FORWARD => MenuAction::Playback(PlaybackAction::SeekBy(SEEK_STEP_SECS)),
        MENU_VOLUME_UP => MenuAction::Playback(PlaybackAction::VolumeBy(VOLUME_STEP)),
        MENU_VOLUME_DOWN => MenuAction::Playback(PlaybackAction::VolumeBy(-VOLUME_STEP)),
        // 前端动作。
        MENU_IMPORT => MenuAction::Frontend(FrontendAction::Import),
        MENU_CHOOSE_LIBRARY_ROOT => MenuAction::Frontend(FrontendAction::ChooseLibraryRoot),
        MENU_RESCAN => MenuAction::Frontend(FrontendAction::Rescan),
        MENU_SETTINGS => MenuAction::Frontend(FrontendAction::Settings),
        MENU_SEARCH => MenuAction::Frontend(FrontendAction::Search),
        MENU_VIEW_ALL => MenuAction::Frontend(FrontendAction::View(LibraryView::AllSongs)),
        MENU_VIEW_RECENT => MenuAction::Frontend(FrontendAction::View(LibraryView::Recent)),
        MENU_VIEW_FAVORITES => MenuAction::Frontend(FrontendAction::View(LibraryView::Favorites)),
        MENU_VIEW_ARTISTS => MenuAction::Frontend(FrontendAction::View(LibraryView::Artists)),
        MENU_VIEW_ALBUMS => MenuAction::Frontend(FrontendAction::View(LibraryView::Albums)),
        MENU_VIEW_PLAYLISTS => MenuAction::Frontend(FrontendAction::View(LibraryView::Playlists)),
        // 壳层动作。
        status_menu::MENU_QUIT => MenuAction::Shell(ShellAction::Quit),
        MENU_HELP_REPOSITORY => MenuAction::Shell(ShellAction::HelpRepository),
        MENU_HELP_ISSUES => MenuAction::Shell(ShellAction::HelpIssues),
        _ => return None,
    };
    Some(action)
}

/// Resolve a relative seek against the current position: `max(0, position +
/// delta)`, with no loaded media treated as position 0 (the actor ignores a
/// seek while stopped, so the value is harmless). Mirrors the workspace
/// ←/→ hotkeys so both entrances seek identically.
#[must_use]
pub fn seek_target(position: Option<f64>, delta: f64) -> f64 {
    (position.unwrap_or(0.0) + delta).max(0.0)
}

/// Resolve a relative volume step against the current volume, clamped to the
/// 0.0–1.0 actor range — the same clamp the `set_volume` command applies.
#[must_use]
pub fn volume_step(volume: f64, delta: f64) -> f64 {
    (volume + delta).clamp(0.0, 1.0)
}

// -----------------------------------------------------------------------------
// macOS installation, dispatch and live state. Everything below is gated: the
// pure mapping above compiles (and is tested) on every platform.
// -----------------------------------------------------------------------------
#[cfg(target_os = "macos")]
mod platform {
    use echo_core::domain::state::PlaybackState;
    use echo_desktop::player::port::{PlayMode, PlayerCommand};

    use super::{
        menu_action, seek_target, volume_step, LibraryView, MenuAction, PlaybackAction,
        ShellAction, MENU_ACTION_EVENT, MENU_CHOOSE_LIBRARY_ROOT, MENU_HELP_ISSUES,
        MENU_HELP_REPOSITORY, MENU_IMPORT, MENU_MODE_REPEAT_ONE, MENU_MODE_SEQUENTIAL,
        MENU_MODE_SHUFFLE, MENU_MUTE, MENU_RESCAN, MENU_SEARCH, MENU_SEEK_BACKWARD,
        MENU_SEEK_FORWARD, MENU_SETTINGS, MENU_VIEW_ALBUMS, MENU_VIEW_ALL, MENU_VIEW_ARTISTS,
        MENU_VIEW_FAVORITES, MENU_VIEW_PLAYLISTS, MENU_VIEW_RECENT, MENU_VOLUME_DOWN,
        MENU_VOLUME_UP,
    };
    use crate::{commands, RuntimeStatusMenuSink};
    use echo_desktop::platform::status_menu::{self, StatusMenuSink};
    use std::sync::{Arc, OnceLock};
    use tauri::menu::{
        CheckMenuItem, CheckMenuItemBuilder, Menu, MenuBuilder, MenuItem, MenuItemBuilder,
        PredefinedMenuItem, SubmenuBuilder, HELP_SUBMENU_ID, WINDOW_SUBMENU_ID,
    };
    use tauri::{AppHandle, Emitter, Manager, Wry};

    /// GitHub repository / issue tracker opened by the 帮助 menu.
    const REPOSITORY_URL: &str = "https://github.com/zsxink/Echo";
    const ISSUES_URL: &str = "https://github.com/zsxink/Echo/issues";

    /// Live handles for the stateful menu items. Tauri menu types are
    /// `Send + Sync` under a main-thread-only mutation contract, which is
    /// exactly how they are used here (every mutator runs inside
    /// `run_on_main_thread`).
    struct MenuHandles {
        play_pause: MenuItem<Wry>,
        mode_sequential: CheckMenuItem<Wry>,
        mode_shuffle: CheckMenuItem<Wry>,
        mode_repeat_one: CheckMenuItem<Wry>,
        views: Vec<(LibraryView, CheckMenuItem<Wry>)>,
    }

    static HANDLES: OnceLock<MenuHandles> = OnceLock::new();

    /// The 播放 submenu plus the live handles it owns.
    struct PlaybackMenu {
        submenu: tauri::menu::Submenu<Wry>,
        play_pause: MenuItem<Wry>,
        mode_sequential: CheckMenuItem<Wry>,
        mode_shuffle: CheckMenuItem<Wry>,
        mode_repeat_one: CheckMenuItem<Wry>,
    }

    /// The 视图 submenu plus its six check items.
    struct ViewMenu {
        submenu: tauri::menu::Submenu<Wry>,
        views: Vec<(LibraryView, CheckMenuItem<Wry>)>,
    }

    /// Build the full application menu. Passed to `tauri::Builder::menu` so it
    /// is installed before the first window appears — no English default menu
    /// ever flashes at startup.
    pub fn build_app_menu(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
        let playback = build_playback_menu(app)?;
        let view = build_view_menu(app)?;
        let menu = MenuBuilder::new(app)
            .item(&build_app_submenu(app)?)
            .item(&build_file_submenu(app)?)
            .item(&build_edit_submenu(app)?)
            .item(&playback.submenu)
            .item(&view.submenu)
            .item(&build_window_submenu(app)?)
            .item(&build_help_submenu(app)?)
            .build()?;

        let _ = HANDLES.set(MenuHandles {
            play_pause: playback.play_pause,
            mode_sequential: playback.mode_sequential,
            mode_shuffle: playback.mode_shuffle,
            mode_repeat_one: playback.mode_repeat_one,
            views: view.views,
        });
        Ok(menu)
    }

    /// Echo（应用菜单）。第一项子菜单即应用菜单；标准项（关于/服务/隐藏）
    /// 保持在系统期望的位置，文案显式传中文——muda 不传 text 时是英文默认。
    fn build_app_submenu(app: &AppHandle) -> tauri::Result<tauri::menu::Submenu<Wry>> {
        SubmenuBuilder::new(app, "Echo")
            .item(&PredefinedMenuItem::about(app, Some("关于 Echo"), None)?)
            .item(&text_item(app, MENU_SETTINGS, "设置…", Some("Cmd+,"))?)
            .separator()
            .item(&PredefinedMenuItem::services(app, Some("服务"))?)
            .separator()
            .item(&PredefinedMenuItem::hide(app, Some("隐藏 Echo"))?)
            .item(&PredefinedMenuItem::hide_others(app, Some("隐藏其他"))?)
            .item(&PredefinedMenuItem::show_all(app, Some("显示全部"))?)
            .separator()
            .item(&text_item(
                app,
                status_menu::MENU_QUIT,
                "退出 Echo",
                Some("Cmd+Q"),
            )?)
            .build()
    }

    fn build_file_submenu(app: &AppHandle) -> tauri::Result<tauri::menu::Submenu<Wry>> {
        SubmenuBuilder::new(app, "文件")
            .item(&text_item(
                app,
                MENU_IMPORT,
                "导入音乐文件…",
                Some("Cmd+O"),
            )?)
            .item(&text_item(
                app,
                MENU_CHOOSE_LIBRARY_ROOT,
                "选择资料库目录…",
                None,
            )?)
            .item(&text_item(app, MENU_RESCAN, "重新扫描资料库", None)?)
            .separator()
            // performClose passes through the shell's CloseRequested handler,
            // so the「保持在后台运行」preference keeps working.
            .item(&PredefinedMenuItem::close_window(app, Some("关闭窗口"))?)
            .build()
    }

    fn build_edit_submenu(app: &AppHandle) -> tauri::Result<tauri::menu::Submenu<Wry>> {
        SubmenuBuilder::new(app, "编辑")
            .item(&PredefinedMenuItem::undo(app, Some("撤销"))?)
            .item(&PredefinedMenuItem::redo(app, Some("重做"))?)
            .separator()
            .item(&PredefinedMenuItem::cut(app, Some("剪切"))?)
            .item(&PredefinedMenuItem::copy(app, Some("复制"))?)
            .item(&PredefinedMenuItem::paste(app, Some("粘贴"))?)
            .item(&PredefinedMenuItem::select_all(app, Some("全选"))?)
            .build()
    }

    fn build_playback_menu(app: &AppHandle) -> tauri::Result<PlaybackMenu> {
        let play_pause =
            MenuItemBuilder::with_id(status_menu::MENU_PLAY_PAUSE, "播放").build(app)?;
        let previous = MenuItemBuilder::with_id(status_menu::MENU_PREVIOUS, "上一首").build(app)?;
        let next = MenuItemBuilder::with_id(status_menu::MENU_NEXT, "下一首").build(app)?;
        let seek_backward = MenuItemBuilder::with_id(MENU_SEEK_BACKWARD, "快退 5 秒").build(app)?;
        let seek_forward = MenuItemBuilder::with_id(MENU_SEEK_FORWARD, "快进 5 秒").build(app)?;
        let volume_up = MenuItemBuilder::with_id(MENU_VOLUME_UP, "音量增").build(app)?;
        let volume_down = MenuItemBuilder::with_id(MENU_VOLUME_DOWN, "音量减").build(app)?;
        let mute = MenuItemBuilder::with_id(MENU_MUTE, "静音").build(app)?;
        // 播放模式是命名入口：直接选中目标模式（非循环切换），勾选态由快照流驱动。
        let mode_sequential =
            CheckMenuItemBuilder::with_id(MENU_MODE_SEQUENTIAL, "顺序播放").build(app)?;
        let mode_shuffle =
            CheckMenuItemBuilder::with_id(MENU_MODE_SHUFFLE, "随机播放").build(app)?;
        let mode_repeat_one =
            CheckMenuItemBuilder::with_id(MENU_MODE_REPEAT_ONE, "单曲循环").build(app)?;
        let mode_submenu = SubmenuBuilder::new(app, "播放模式")
            .item(&mode_sequential)
            .item(&mode_shuffle)
            .item(&mode_repeat_one)
            .build()?;
        let submenu = SubmenuBuilder::new(app, "播放")
            .item(&play_pause)
            .item(&previous)
            .item(&next)
            .item(&seek_backward)
            .item(&seek_forward)
            .separator()
            .item(&volume_up)
            .item(&volume_down)
            .item(&mute)
            .separator()
            .item(&mode_submenu)
            .build()?;
        Ok(PlaybackMenu {
            submenu,
            play_pause,
            mode_sequential,
            mode_shuffle,
            mode_repeat_one,
        })
    }

    fn build_view_menu(app: &AppHandle) -> tauri::Result<ViewMenu> {
        let view_all = CheckMenuItemBuilder::with_id(MENU_VIEW_ALL, "全部歌曲")
            .accelerator("Cmd+1")
            .build(app)?;
        let view_recent = CheckMenuItemBuilder::with_id(MENU_VIEW_RECENT, "最近添加")
            .accelerator("Cmd+2")
            .build(app)?;
        let view_favorites = CheckMenuItemBuilder::with_id(MENU_VIEW_FAVORITES, "喜欢的音乐")
            .accelerator("Cmd+3")
            .build(app)?;
        let view_artists = CheckMenuItemBuilder::with_id(MENU_VIEW_ARTISTS, "歌手")
            .accelerator("Cmd+4")
            .build(app)?;
        let view_albums = CheckMenuItemBuilder::with_id(MENU_VIEW_ALBUMS, "专辑")
            .accelerator("Cmd+5")
            .build(app)?;
        let view_playlists = CheckMenuItemBuilder::with_id(MENU_VIEW_PLAYLISTS, "歌单")
            .accelerator("Cmd+6")
            .build(app)?;
        let search = MenuItemBuilder::with_id(MENU_SEARCH, "搜索")
            .accelerator("Cmd+F")
            .build(app)?;
        let submenu = SubmenuBuilder::new(app, "视图")
            .item(&view_all)
            .item(&view_recent)
            .item(&view_favorites)
            .item(&view_artists)
            .item(&view_albums)
            .item(&view_playlists)
            .separator()
            .item(&search)
            .separator()
            .item(&PredefinedMenuItem::fullscreen(app, Some("全屏"))?)
            .build()?;
        Ok(ViewMenu {
            submenu,
            views: vec![
                (LibraryView::AllSongs, view_all),
                (LibraryView::Recent, view_recent),
                (LibraryView::Favorites, view_favorites),
                (LibraryView::Artists, view_artists),
                (LibraryView::Albums, view_albums),
                (LibraryView::Playlists, view_playlists),
            ],
        })
    }

    /// The system reserved id lets Tauri register this submenu as `NSApp`'s
    /// window menu (`init_app_menu`), keeping standard integration.
    fn build_window_submenu(app: &AppHandle) -> tauri::Result<tauri::menu::Submenu<Wry>> {
        SubmenuBuilder::with_id(app, WINDOW_SUBMENU_ID, "窗口")
            .item(&PredefinedMenuItem::minimize(app, Some("最小化"))?)
            .item(&PredefinedMenuItem::maximize(app, Some("缩放"))?)
            .item(&PredefinedMenuItem::close_window(app, Some("关闭"))?)
            .build()
    }

    /// Same as the window menu: the reserved id registers this submenu as
    /// `NSApp`'s help menu.
    fn build_help_submenu(app: &AppHandle) -> tauri::Result<tauri::menu::Submenu<Wry>> {
        SubmenuBuilder::with_id(app, HELP_SUBMENU_ID, "帮助")
            .item(&text_item(app, MENU_HELP_REPOSITORY, "GitHub 仓库", None)?)
            .item(&text_item(app, MENU_HELP_ISSUES, "报告问题", None)?)
            .build()
    }

    fn text_item(
        app: &AppHandle,
        id: &str,
        text: &str,
        accelerator: Option<&str>,
    ) -> tauri::Result<MenuItem<Wry>> {
        let mut builder = MenuItemBuilder::with_id(id, text);
        if let Some(accelerator) = accelerator {
            builder = builder.accelerator(accelerator);
        }
        builder.build(app)
    }

    /// The `menu://action` payload wire format.
    #[derive(Clone, serde::Serialize)]
    struct MenuActionPayload {
        action: String,
    }

    /// `tauri::Builder::on_menu_event` entry point (the builder adapter in
    /// `bootstrap.rs` converts the by-value callback event into a borrow).
    pub fn handle_menu_event(app: &AppHandle, event: &tauri::menu::MenuEvent) {
        dispatch(app, event.id().as_ref());
    }

    /// Dispatch a semantic menu id exactly as if the corresponding app-menu
    /// item had been clicked. The status-row context menu forwards through
    /// here so both entrances share one decision layer.
    pub fn dispatch_by_id(app: &AppHandle, id: &str) {
        dispatch(app, id);
    }

    fn dispatch(app: &AppHandle, id: &str) {
        match menu_action(id) {
            Some(MenuAction::Playback(action)) => dispatch_playback(app, action),
            Some(MenuAction::Frontend(action)) => {
                let _ = app.emit(
                    MENU_ACTION_EVENT,
                    MenuActionPayload {
                        action: action.payload(),
                    },
                );
            }
            Some(MenuAction::Shell(action)) => dispatch_shell(app, action),
            None => {}
        }
    }

    fn dispatch_playback(app: &AppHandle, action: PlaybackAction) {
        let Some(sink) = app.try_state::<Arc<RuntimeStatusMenuSink>>() else {
            return;
        };
        let command = match action {
            PlaybackAction::Command(command) => command,
            PlaybackAction::SeekBy(delta) => {
                PlayerCommand::Seek(seek_target(current_position(app), delta))
            }
            PlaybackAction::VolumeBy(delta) => {
                PlayerCommand::SetVolume(volume_step(current_volume(app), delta))
            }
        };
        let _ = sink.on_command(command);
    }

    fn current_position(app: &AppHandle) -> Option<f64> {
        let handle = app.try_state::<commands::PlayerHandle>()?;
        let coordinator = handle.coordinator.lock().ok()?;
        coordinator.snapshot().position
    }

    fn current_volume(app: &AppHandle) -> f64 {
        app.try_state::<commands::PlayerHandle>()
            .and_then(|handle| {
                handle
                    .coordinator
                    .lock()
                    .ok()
                    .map(|coordinator| coordinator.snapshot().volume)
            })
            .unwrap_or(0.0)
    }

    fn dispatch_shell(app: &AppHandle, action: ShellAction) {
        match action {
            ShellAction::Quit => {
                // Same contract as the tray/status-row quit: persist the
                // playback session, then terminate the process.
                if let Some(player) = app.try_state::<commands::PlayerHandle>() {
                    commands::flush_player_session(&player);
                }
                app.exit(0);
            }
            ShellAction::HelpRepository => open_external(app, REPOSITORY_URL),
            ShellAction::HelpIssues => open_external(app, ISSUES_URL),
        }
    }

    fn open_external(app: &AppHandle, url: &str) {
        use tauri_plugin_opener::OpenerExt;
        if let Err(error) = app.opener().open_url(url, None::<&str>) {
            tracing::warn!(target: "echo_desktop", %error, %url, "failed to open menu help link");
        }
    }

    /// Refresh the stateful playback items. Driven by the same snapshot
    /// stream as the status-row icons (`spawn_macos_status_row_snapshot_refresh`)
    /// so the menu and the compact row can never disagree.
    pub fn refresh_playback_state(state: PlaybackState, mode: PlayMode) {
        let Some(handles) = HANDLES.get() else {
            return;
        };
        let _ = handles
            .play_pause
            .set_text(status_menu::play_pause_label(state));
        let _ = handles
            .mode_sequential
            .set_checked(mode == PlayMode::Sequential);
        let _ = handles.mode_shuffle.set_checked(mode == PlayMode::Shuffle);
        let _ = handles
            .mode_repeat_one
            .set_checked(mode == PlayMode::RepeatOne);
    }

    /// Apply a frontend-reported view to the 视图 check items. Must run on the
    /// main thread (menu mutation contract), so this schedules itself there.
    pub fn set_current_view(app: &AppHandle, view: &str) {
        let Some(target) = LibraryView::from_payload(view) else {
            if !view.is_empty() {
                tracing::debug!(target: "echo_desktop", %view, "ignoring unknown library view report");
            }
            return;
        };
        let _ = app.run_on_main_thread(move || {
            let Some(handles) = HANDLES.get() else {
                return;
            };
            for (kind, item) in &handles.views {
                let _ = item.set_checked(*kind == target);
            }
        });
    }
}

#[cfg(target_os = "macos")]
pub use platform::{
    build_app_menu, dispatch_by_id, handle_menu_event, refresh_playback_state, set_current_view,
};

#[cfg(test)]
mod tests {
    use super::*;

    fn playback(id: &str) -> Option<PlaybackAction> {
        match menu_action(id)? {
            MenuAction::Playback(action) => Some(action),
            _ => None,
        }
    }

    fn frontend(id: &str) -> Option<FrontendAction> {
        match menu_action(id)? {
            MenuAction::Frontend(action) => Some(action),
            _ => None,
        }
    }

    fn shell(id: &str) -> Option<ShellAction> {
        match menu_action(id)? {
            MenuAction::Shell(action) => Some(action),
            _ => None,
        }
    }

    #[test]
    fn transport_items_reuse_the_status_menu_authority_channel() {
        assert_eq!(
            playback(status_menu::MENU_PLAY_PAUSE),
            Some(PlaybackAction::Command(PlayerCommand::TogglePlayPause))
        );
        assert_eq!(
            playback(status_menu::MENU_PREVIOUS),
            Some(PlaybackAction::Command(PlayerCommand::Previous))
        );
        assert_eq!(
            playback(status_menu::MENU_NEXT),
            Some(PlaybackAction::Command(PlayerCommand::Next))
        );
        assert_eq!(
            playback(MENU_MUTE),
            Some(PlaybackAction::Command(PlayerCommand::ToggleMute))
        );
    }

    #[test]
    fn playback_modes_select_the_target_directly() {
        assert_eq!(
            playback(MENU_MODE_SEQUENTIAL),
            Some(PlaybackAction::Command(PlayerCommand::SetMode(
                PlayMode::Sequential
            )))
        );
        assert_eq!(
            playback(MENU_MODE_SHUFFLE),
            Some(PlaybackAction::Command(PlayerCommand::SetMode(
                PlayMode::Shuffle
            )))
        );
        assert_eq!(
            playback(MENU_MODE_REPEAT_ONE),
            Some(PlaybackAction::Command(PlayerCommand::SetMode(
                PlayMode::RepeatOne
            )))
        );
    }

    #[test]
    fn seek_and_volume_items_stay_relative_until_dispatch() {
        assert_eq!(
            playback(MENU_SEEK_BACKWARD),
            Some(PlaybackAction::SeekBy(-SEEK_STEP_SECS))
        );
        assert_eq!(
            playback(MENU_SEEK_FORWARD),
            Some(PlaybackAction::SeekBy(SEEK_STEP_SECS))
        );
        assert_eq!(
            playback(MENU_VOLUME_UP),
            Some(PlaybackAction::VolumeBy(VOLUME_STEP))
        );
        assert_eq!(
            playback(MENU_VOLUME_DOWN),
            Some(PlaybackAction::VolumeBy(-VOLUME_STEP))
        );
    }

    #[test]
    fn frontend_actions_use_stable_payloads() {
        assert_eq!(frontend(MENU_IMPORT), Some(FrontendAction::Import));
        assert_eq!(
            frontend(MENU_CHOOSE_LIBRARY_ROOT),
            Some(FrontendAction::ChooseLibraryRoot)
        );
        assert_eq!(frontend(MENU_RESCAN), Some(FrontendAction::Rescan));
        assert_eq!(frontend(MENU_SETTINGS), Some(FrontendAction::Settings));
        assert_eq!(frontend(MENU_SEARCH), Some(FrontendAction::Search));
        assert_eq!(
            frontend(MENU_IMPORT).map(FrontendAction::payload),
            Some("import".to_owned())
        );
    }

    #[test]
    fn view_items_map_to_frontend_library_kinds() {
        let cases = [
            (MENU_VIEW_ALL, LibraryView::AllSongs, "all"),
            (MENU_VIEW_RECENT, LibraryView::Recent, "recent"),
            (MENU_VIEW_FAVORITES, LibraryView::Favorites, "favorites"),
            (MENU_VIEW_ARTISTS, LibraryView::Artists, "artists"),
            (MENU_VIEW_ALBUMS, LibraryView::Albums, "albums"),
            (MENU_VIEW_PLAYLISTS, LibraryView::Playlists, "playlist"),
        ];
        let mut payloads = Vec::new();
        for (id, view, kind) in cases {
            assert_eq!(frontend(id), Some(FrontendAction::View(view)));
            assert_eq!(
                frontend(id).map(FrontendAction::payload),
                Some(format!("view:{kind}"))
            );
            assert_eq!(LibraryView::from_payload(kind), Some(view));
            payloads.push(kind);
        }
        // Every wire payload is distinct — no view can shadow another.
        let mut sorted = payloads;
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 6);
    }

    #[test]
    fn unknown_or_predefined_ids_produce_no_action() {
        assert_eq!(menu_action("unknown"), None);
        assert_eq!(menu_action(""), None);
        // Predefined system items never carry a semantic id through the
        // handler; separators/summary ids from the tray are not app-menu ids.
        assert!(shell("separator").is_none());
        assert!(playback("echo-summary").is_none());
    }

    #[test]
    fn shell_actions_are_quit_and_help_links() {
        assert_eq!(shell(status_menu::MENU_QUIT), Some(ShellAction::Quit));
        assert_eq!(
            shell(MENU_HELP_REPOSITORY),
            Some(ShellAction::HelpRepository)
        );
        assert_eq!(shell(MENU_HELP_ISSUES), Some(ShellAction::HelpIssues));
    }

    /// Float results are asserted with a tolerance, not `==`: the same
    /// loudness makes a round trip through mpv's 0–100 scale and can come back
    /// a few ULPs off (see `VOLUME_EPSILON` in `player::port`).
    fn assert_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < 1e-9,
            "expected ~{expected}, got {actual}"
        );
    }

    #[test]
    fn seek_resolution_mirrors_the_workspace_hotkeys() {
        assert_close(seek_target(Some(12.0), -5.0), 7.0);
        assert_close(seek_target(Some(3.0), -5.0), 0.0);
        assert_close(seek_target(None, 5.0), 5.0);
        assert_close(seek_target(Some(0.0), -5.0), 0.0);
    }

    #[test]
    fn volume_resolution_clamps_to_the_actor_range() {
        assert_close(volume_step(0.5, 0.05), 0.55);
        assert_close(volume_step(0.98, 0.05), 1.0);
        assert_close(volume_step(0.02, -0.05), 0.0);
        assert_close(volume_step(0.0, -0.05), 0.0);
    }

    #[test]
    fn view_reports_parse_only_frontend_kinds() {
        assert_eq!(
            LibraryView::from_payload("playlist"),
            Some(LibraryView::Playlists)
        );
        assert_eq!(LibraryView::from_payload("Playlists"), None);
        assert_eq!(LibraryView::from_payload("view:playlist"), None);
        assert_eq!(LibraryView::from_payload(""), None);
    }

    #[test]
    fn frontend_event_name_is_pinned_on_every_platform() {
        // `App.tsx` subscribes to this exact channel string, so the name is a
        // cross-boundary contract, not a macOS-internal detail. Pinning it here
        // also keeps the constant live on the non-macOS test profile, where the
        // macOS-only emitter is cfg'd out and clippy would otherwise flag it as
        // dead code under `--all-targets`.
        assert_eq!(MENU_ACTION_EVENT, "menu://action");
    }
}
