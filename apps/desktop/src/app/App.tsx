/**
 * Application shell (task 10.3 / 10.4).
 *
 * Reproduces the prototype's frame (`docs/prototype/echo-desktop-player.html`)
 * element for element:
 *
 *   div.app
 *     aside.sidebar  品牌区 / 资料库导航 / 歌单导航 / 应用工具区
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
 * 应用工具区 holds 设置 only — the prototype's sidebar has no theme
 * dropdown (`.theme-control` exists in its stylesheet but no markup uses it);
 * theme switching lives in 设置, which is where the prototype puts it.
 *
 * There is deliberately **no** sync control in this shell. The prototype has no
 * `.sync-button` markup either (grep it), so reproducing one never was shell
 * fidelity — it was invention, and it violated task 10.4 / 13.8 and the
 * phase-one scope in `docs/ROADMAP.md` ("不包含：资料库同步与可操作的同步入口").
 * Sync arrives in phase two.
 *
 * All state and the sidebar's overlay/focus behaviour are orchestrated by
 * `useAppShell` (task 6.6); this file keeps only the layout.
 */

import { bridge } from "../bridge";
import { ChooseRootView } from "../features/workspace/ChooseRootView";
import { LibraryStatusView } from "../features/workspace/LibraryStatusView";
import type { LibraryCountView } from "../features/library/libraryCounts";
import { useLibraryCounts } from "../features/library/libraryCounts";
import type { LibraryViewKind } from "../features/library/types";
import { coverClass } from "../features/library/coverClass";
import { LibraryWorkspace } from "../features/library/LibraryWorkspace";
import { PlaylistCreateDialog } from "../features/playlists/PlaylistNameDialog";
import { PlaylistsView } from "../features/playlists/PlaylistsView";
import { SettingsView } from "../features/settings/SettingsView";
import { PlayerBar } from "../features/player/PlayerBar";
import { ImmersivePlayer } from "../features/player/ImmersivePlayer";
import { QueuePanel } from "../features/player/QueuePanel";
import { Icon } from "./Icon";
import { ShellNavProvider } from "./shell";
import { ToastView } from "./ToastView";
import { useAppShell } from "./useAppShell";

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

  const activePlaylist = playlists.find((playlist) => playlist.id === activePlaylistId) ?? null;
  const viewTitle =
    libraryView === "playlist"
      ? (activePlaylist?.name ?? "歌单")
      : libraryView === "recent"
        ? "最近添加"
        : libraryView === "favorites"
          ? "喜欢的音乐"
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

        {status.configured ? (
          <>
            <aside
              id="app-sidebar"
              className="sidebar"
              data-testid="sidebar"
              aria-label="资料库导航"
              ref={sidebarRef}
            >
              <div className="brand" aria-label="Echo，本地音乐播放器">
                <span className="brand-mark" aria-hidden="true">
                  <Icon name="note" />
                </span>
                <span>Echo</span>
              </div>

              <nav className="nav-group" aria-label="主导航">
                <span className="nav-label">资料库</span>
                {navItem("all", "全部歌曲", "library", "all")}
                {navItem("recent", "最近添加", "recent", "recent")}
                {navItem("favorites", "喜欢的音乐", "heart", "favorites")}
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

              <div className="side-foot">
                <button
                  type="button"
                  className="link-button settings-button"
                  onClick={() => setSettingsOpen(true)}
                  data-testid="settings-button"
                >
                  设置
                </button>
              </div>
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
            </section>
          </>
        ) : (
          <ChooseRootView onActivated={status.refresh} />
        )}

        <ImmersivePlayer />

        <QueuePanel />

        <PlayerBar />

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
 * 资料库导航计数. The prototype prints sample counts; Echo prints only a number
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
