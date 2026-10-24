/**
 * Application shell (task 10.3 / 10.4).
 *
 * Reproduces the prototype's frame (`docs/prototype/echo-desktop-player.html`)
 * element for element:
 *
 *   div.app
 *     aside.sidebar  品牌区 / 音乐库导航 / 歌单导航 / 应用工具区
 *     button.sidebar-scrim
 *     section.workspace        ← the routed view renders `header.topbar` +
 *                                `main.content` into this column
 *     设置 (`.settings-dialog`) is rendered here as a floating layer of the
 *     workspace, exactly where the prototype puts it. It is `position: fixed;
 *     inset: 0`, so it covers the whole window from that position.
 *     footer.playerbar
 *     section.now-playing-popover  (ImmersivePlayer)
 *     section.queue-popover        (QueuePanel)
 *     div.toast
 *
 * It never claims a working library it may not have: it reads a library-status
 * snapshot and renders the workspace / read-only / unavailable / first-launch
 * states accordingly.
 *
 * The Echo brand row owns compact Settings and Import actions. Theme switching
 * stays in 设置, which is where the prototype puts it.
 *
 * There is deliberately **no** sync control in this shell. The prototype has no
 * `.sync-button` markup either (grep it), so reproducing one never was shell
 * fidelity — it was invention, and it violated task 10.4 / 13.8 and the
 * phase-one scope in `docs/ROADMAP.md` ("不包含：资料库同步与可操作的同步入口").
 * Sync arrives in phase two.
 *
 * Navigation and overlay/focus behaviour are orchestrated by `useAppShell`
 * (task 6.6); this file also provides the brand-area portal slot for importing.
 */

import { useEffect, useRef } from "react";

import { bridge, reportBridgeFailure } from "../bridge";
import { useImport } from "../features/import";
import { ChooseRootView } from "../features/workspace/ChooseRootView";
import { LibraryStatusView } from "../features/workspace/LibraryStatusView";
import type { LibraryCountView } from "../features/library/libraryCounts";
import { useLibraryCounts } from "../features/library/libraryCounts";
import { invalidateLibrary } from "../features/library/libraryInvalidation";
import type { LibraryViewKind } from "../features/library/types";
import { coverClass } from "../features/library/coverClass";
import { LibraryWorkspace } from "../features/library/LibraryWorkspace";
import { CollectionDirectory } from "../features/library/CollectionDirectory";
import { PlaylistCreateDialog } from "../features/playlists/PlaylistNameDialog";
import { PlaylistsView } from "../features/playlists/PlaylistsView";
import { SettingsView } from "../features/settings/SettingsView";
import { PlayerBar } from "../features/player/PlayerBar";
import { ImmersivePlayer } from "../features/player/ImmersivePlayer";
import { QueuePanel } from "../features/player/QueuePanel";
import { Icon } from "./Icon";
import { ShellNavProvider } from "./shell";
import { WindowControls } from "./WindowControls";
import { TitlebarActions } from "./TitlebarActions";
import { EffectsPanel } from "../features/player";
import { ToastView } from "./ToastView";
import { useAppShell } from "./useAppShell";
import { invalidateCovers } from "./coverArt";
import { requestSearchFocus } from "./searchFocus";
import { error as logError } from "../logging/log";

/** The view kinds the shell's `selectView` accepts (kept in sync with
 * `features/library/types.ts`); menu `view:*` payloads are filtered by it. */
const LIBRARY_VIEW_KINDS: readonly string[] = [
  "all",
  "recent",
  "favorites",
  "artists",
  "albums",
  "playlist",
];

function isLibraryViewKind(view: string): view is LibraryViewKind {
  return LIBRARY_VIEW_KINDS.includes(view);
}

export function App() {
  const {
    theme,
    status,
    libraryView,
    activePlaylistId,
    playlists,
    reloadPlaylists,
    selectView,
    nav,
    sidebarRef,
    sidebarOpen,
    closeSidebar,
    playlistNameOpen,
    setPlaylistNameOpen,
    settingsOpen,
    setSettingsOpen,
  } = useAppShell();
  // 菜单动作处理器只需要这三个状态切片（原始值 + 稳定的 refresh），避免
  // 整个 status 对象身份变化导致订阅反复重建。
  const { activeRoot, scanning, refresh: statusRefresh } = status;
  // The import entry is shell-owned (playlist-search-locate-import 5.2): it is
  // rendered into the sidebar brand area, so the action stays available under
  // every library view — 歌单, 歌手/专辑目录 and their details included.
  const { importing, runImport, renderImportDialog } = useImport({
    onImportCommitted: reloadPlaylists,
  });
  // The queue popover and its trigger are siblings, so the panel cannot reach the
  // button through its own container. Sharing one ref lets the panel count the
  // button as part of its interior — a second press closes it instead of
  // closing and immediately re-opening it.
  const queueTriggerRef = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    const preventDefaultContextMenu = (event: Event) => event.preventDefault();
    document.addEventListener("contextmenu", preventDefaultContextMenu);
    return () => document.removeEventListener("contextmenu", preventDefaultContextMenu);
  }, []);

  // Finder / file-manager opens are a desktop-owned playback request, never an
  // import. The player coordinator atomically replaces its context for each
  // accepted file; registering here also covers paths drained after cold start.
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    const signalReady = () =>
      // The listener is now in place, so the shell may drain every file-open
      // that arrived before it (deliver-file-opens-after-frontend-ready): Tauri
      // drops an emit with no registered JS listener, so paths from a cold-start
      // double-click wait in the startup FIFO until this command opens the gate.
      // Idempotent on the backend — the second call drains an empty queue — so
      // React StrictMode's double-invoked effect is harmless.
      bridge.fireAndForget("file_open_frontend_ready");
    void bridge
      .subscribe<readonly string[]>("app://file-open-request", (paths) => {
        // Each element is a DECODED absolute filesystem path: the shell's
        // `open_targets` already converted the OS `file://` URL and decoded
        // percent-encoding exactly once (normalize-os-file-open-paths). Do NOT
        // URL-decode here — a filename may legitimately contain a literal
        // `%20`, and a second decode would corrupt it. The display name is the
        // final path segment, taken verbatim.
        for (const path of paths) {
          const displayName = path.split(/[\\/]/).filter(Boolean).at(-1) ?? "音频文件";
          bridge.fireAndForget("play_temporary_file", { path, displayName });
        }
      })
      .then((dispose) => {
        unlisten = dispose;
        signalReady();
      })
      .catch((error: unknown) => {
        // The listener could not be registered. Fire the ready signal anyway so
        // the shell still drains (the emit is dropped server-side, but nothing
        // silently in a stuck pre-ready queue) and surface the failure (D2).
        signalReady();
        reportBridgeFailure("file_open_frontend_ready", error);
      });
    return () => unlisten?.();
  }, []);

  // macOS 应用菜单（issue #73）通过 `menu://action` 下发的前端动作。播放类
  // 菜单命令由壳层直接进入播放权威通道（与托盘/状态项同一条），不经这里；
  // 这里只承接需要 React 状态或既有流程的动作，全部转调既有入口。
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    // 选择资料库目录…：与设置页同一"已配置后切换目录"语义，另加封面失效——
    // 新根可能把相同 SongId 指向不同文件，缓存封面不得跨根存活。
    const runChooseRoot = async () => {
      try {
        const result = await bridge.call("choose_library_root");
        if (result === null) return; // 取消选择器是无操作，不解释为成功
        invalidateCovers();
        await statusRefresh();
      } catch (error: unknown) {
        reportBridgeFailure("choose_library_root", error);
      }
    };
    // 重新扫描资料库：与设置页「重新扫描」同一序列。
    const runRescan = async () => {
      if (!activeRoot || scanning) return;
      try {
        await bridge.call("start_scan", { root: activeRoot });
        invalidateCovers();
        invalidateLibrary();
        await statusRefresh();
      } catch (error: unknown) {
        reportBridgeFailure("start_scan", error);
      }
    };
    void bridge
      .subscribe<{ action: string }>("menu://action", ({ action }) => {
        if (action.startsWith("view:")) {
          const view = action.slice("view:".length);
          if (isLibraryViewKind(view)) selectView(view);
          return;
        }
        switch (action) {
          case "import":
            void runImport();
            break;
          case "settings":
            setSettingsOpen(true);
            break;
          case "search":
            // 设置浮层是模态焦点陷阱；打开时不把焦点抢回其背后的搜索框。
            if (!settingsOpen) requestSearchFocus();
            break;
          case "choose_library_root":
            void runChooseRoot();
            break;
          case "rescan":
            void runRescan();
            break;
          default:
            break;
        }
      })
      .then((dispose) => {
        unlisten = dispose;
      })
      .catch(() => {
        // 事件通道名不是命令名，无法走 `reportBridgeFailure`；按渲染端日志
        // 约定保底记录（不含载荷）。
        logError("菜单事件订阅失败", "menu-action");
      });
    return () => unlisten?.();
  }, [selectView, runImport, setSettingsOpen, settingsOpen, activeRoot, scanning, statusRefresh]);

  const activePlaylist = playlists.find((playlist) => playlist.id === activePlaylistId) ?? null;
  const viewTitle =
    libraryView === "playlist"
      ? (activePlaylist?.name ?? "歌单")
      : libraryView === "recent"
        ? "最近添加"
        : libraryView === "favorites"
          ? "喜欢的音乐"
          : libraryView === "artists"
            ? "歌手"
            : libraryView === "albums"
              ? "专辑"
              : "全部歌曲";

  const navItem = (
    view: LibraryViewKind,
    label: string,
    icon: string,
    countView?: LibraryCountView,
  ) => (
    <button
      type="button"
      className={`nav-item${libraryView === view && activePlaylistId === null ? " active" : ""}`}
      aria-current={libraryView === view && activePlaylistId === null ? "page" : undefined}
      onClick={() => selectView(view)}
    >
      <Icon name={icon} />
      {label}
      {countView ? <LibraryNavCount view={countView} /> : null}
    </button>
  );

  return (
    <ShellNavProvider value={nav}>
      <div
        className={`app${status.configured ? "" : " app-initial"}`}
        id="app-shell"
        data-testid="echo-shell"
        data-echo-theme={theme}
      >
        {/* macOS overlay titlebar drag handle — see `.titlebar-drag` in
            app-extras.css. `tauri.conf.json` sets `titleBarStyle: "Overlay"` +
            `hiddenTitle: true`, so no native titlebar drags the window and the
            top `--titlebar-inset` strip (28px in the WebView, 0px in a plain
            browser preview) must declare itself draggable instead. */}
        <div className="titlebar-drag" data-tauri-drag-region aria-hidden="true" />

        {/* Windows caption buttons (issue #76): `decorations: false` leaves no
            native — □ ×, so the shell draws them in the top-right safe-area
            strip. Renders nothing on macOS/Linux/web. */}
        <WindowControls />

        {/* 标题栏导入/设置：Windows 在窗口控件左侧，macOS 在右上角安全区条。
            未配置资料库前入口无意义（设置浮层只在工作区渲染），随侧边栏一起
            出现；导入在只读库同样隐藏。 */}
        {status.configured ? (
          <TitlebarActions
            importing={importing}
            showImport={!status.readOnly}
            onImport={() => void runImport()}
            onSettings={() => setSettingsOpen(true)}
          />
        ) : null}

        {status.configured ? (
          <>
            <aside
              id="app-sidebar"
              className="sidebar"
              data-testid="sidebar"
              aria-label="音乐库导航"
              ref={sidebarRef}
            >
              <div className="brand-area" data-testid="brand-area">
                <div className="brand" aria-label="Echo，本地音乐播放器">
                  <span className="brand-mark" aria-hidden="true">
                    <Icon name="note" />
                  </span>
                  <span className="brand-name">Echo</span>
                </div>
              </div>

              <nav className="nav-group" aria-label="主导航">
                <span className="nav-label">音乐库</span>
                {navItem("all", "全部歌曲", "library", "all")}
                {navItem("recent", "最近添加", "recent", "recent")}
                {navItem("favorites", "喜欢的音乐", "heart", "favorites")}
                {navItem("artists", "歌手", "artist", "artists")}
                {navItem("albums", "专辑", "album", "albums")}
              </nav>

              <nav className="nav-group playlist-navigation" aria-label="歌单">
                <div className="nav-section-head">
                  <span className="nav-label">歌单</span>
                  <button
                    type="button"
                    className="playlist-create"
                    aria-label="添加歌单"
                    title="添加歌单"
                    disabled={status.readOnly || !status.activeRoot || status.unavailable}
                    onClick={() => setPlaylistNameOpen(true)}
                    data-testid="create-playlist"
                  >
                    <Icon name="plus" />
                  </button>
                </div>
                <div className="playlist-list" role="list">
                  {playlists.map((playlist) => (
                    <button
                      key={playlist.id}
                      type="button"
                      role="listitem"
                      className={`nav-item playlist-item${
                        libraryView === "playlist" && activePlaylistId === playlist.id
                          ? " active"
                          : ""
                      }`}
                      aria-current={
                        libraryView === "playlist" && activePlaylistId === playlist.id
                          ? "page"
                          : undefined
                      }
                      onClick={() => selectView("playlist", playlist.id)}
                    >
                      <span
                        className={`cover playlist-cover ${coverClass(playlist.id)}${playlist.coverKey ? " has-image" : ""}`}
                        aria-hidden="true"
                      >
                        {playlist.coverKey ? (
                          <img src={bridge.assetUrl(playlist.coverKey)} alt="" />
                        ) : null}
                      </span>
                      <span className="playlist-name">{playlist.name}</span>
                      {/* `memberCount` is authoritative from the backend — no
                          need to open the playlist to know its size. */}
                      <span className="nav-count" data-testid={`playlist-count-${playlist.id}`}>
                        {playlist.memberCount}
                      </span>
                    </button>
                  ))}
                </div>
              </nav>
            </aside>

            <button
              type="button"
              className="sidebar-scrim"
              aria-label="关闭侧边栏"
              hidden={!sidebarOpen}
              onClick={() => closeSidebar()}
              data-testid="sidebar-mask"
            />

            <section className="workspace" data-testid="workspace">
              {status.unavailable ? (
                <LibraryStatusView status={status} />
              ) : libraryView === "playlist" && activePlaylistId ? (
                <PlaylistsView
                  playlistId={activePlaylistId}
                  title={viewTitle}
                  coverKey={
                    playlists.find((playlist) => playlist.id === activePlaylistId)?.coverKey
                  }
                  automaticCoverKey={
                    playlists.find((playlist) => playlist.id === activePlaylistId)
                      ?.automaticCoverKey
                  }
                  hasCustomCover={
                    playlists.find((playlist) => playlist.id === activePlaylistId)?.hasCustomCover
                  }
                  root={status.activeRoot ?? ""}
                  existingNames={playlists.map((playlist) => playlist.name)}
                  readOnly={status.readOnly}
                  onDeleted={() => {
                    selectView("all");
                    reloadPlaylists();
                  }}
                  onLibraryChanged={reloadPlaylists}
                />
              ) : libraryView === "artists" || libraryView === "albums" ? (
                <CollectionDirectory
                  kind={libraryView === "artists" ? "artist" : "album"}
                  root={status.activeRoot ?? ""}
                  readOnly={status.readOnly}
                  onLibraryChanged={reloadPlaylists}
                />
              ) : (
                <LibraryWorkspace
                  view={libraryView}
                  title={viewTitle}
                  root={status.activeRoot ?? ""}
                  readOnly={status.readOnly}
                  onLibraryChanged={reloadPlaylists}
                />
              )}

              {settingsOpen ? <SettingsView onClose={() => setSettingsOpen(false)} /> : null}
              {renderImportDialog()}
            </section>
          </>
        ) : (
          <ChooseRootView onActivated={status.refresh} />
        )}

        <ImmersivePlayer />

        <QueuePanel triggerRef={queueTriggerRef} />
        <EffectsPanel />

        <PlayerBar queueTriggerRef={queueTriggerRef} />

        <ToastView />

        {playlistNameOpen ? (
          <PlaylistCreateDialog
            existingNames={playlists.map((playlist) => playlist.name)}
            root={status.activeRoot}
            onClose={() => setPlaylistNameOpen(false)}
            onCreated={(_name, createdId) => {
              setPlaylistNameOpen(false);
              reloadPlaylists();
              if (createdId) selectView("playlist", createdId);
            }}
          />
        ) : null}
      </div>
    </ShellNavProvider>
  );
}

/**
 * 音乐库导航计数. The prototype prints sample counts; Echo prints only a number
 * the backend has actually counted, and never blanks a known number while a
 * re-count is in flight (`docs/interface-terminology.md`: 原型模拟业务状态不作为实现).
 */
function LibraryNavCount({ view }: { view: LibraryCountView }) {
  const counts = useLibraryCounts();
  const count = counts[view];
  return count === null ? null : (
    <span className="nav-count" data-testid={`nav-count-${view}`}>
      {count}
    </span>
  );
}
