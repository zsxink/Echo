/**
 * Application shell (task 10.3 / 10.4).
 *
 * Composes the frame — sidebar navigation, workspace, persistent player bar —
 * and routes the workspace between the library views (全部歌曲 / 最近添加 /
 * 喜欢的音乐 / 歌单) and the settings surface. It never claims a working
 * library it may not have: it reads a library-status snapshot and renders the
 * workspace / read-only / unavailable / first-launch states accordingly.
 *
 * Layout: `ui-layout` → `ui-sidebar` → `ui-workspace` → `ui-playerbar`. The
 * persistent player bar stays visible (task 11.1); the immersive player covers
 * the workspace (task 11.3) through the overlay manager.
 */

import { useCallback, useState } from "react";

import { ChooseRootView } from "../features/workspace/ChooseRootView";
import { useLibraryPlaylists } from "../features/playlists/useLibraryPlaylists";
import { LibraryStatusView } from "../features/workspace/LibraryStatusView";
import { useLibraryStatus } from "../features/workspace/useLibraryStatus";
import type { LibraryViewKind } from "../features/library/types";
import { LibraryWorkspace } from "../features/library/LibraryWorkspace";
import { PlaylistsView } from "../features/playlists/PlaylistsView";
import { SettingsView } from "../features/settings/SettingsView";
import { PlayerBar } from "../features/player/PlayerBar";
import { useTheme } from "../features/settings/useTheme";
import "./app.css";

type WorkspaceRoute = "library" | "settings";

export function App() {
  const { theme } = useTheme();
  const status = useLibraryStatus();
  const [route, setRoute] = useState<WorkspaceRoute>("library");
  const [libraryView, setLibraryView] = useState<LibraryViewKind>("all");
  const [activePlaylistId, setActivePlaylistId] = useState<string | null>(null);

  const selectView = useCallback((view: LibraryViewKind, playlistId?: string | null) => {
    setRoute("library");
    setLibraryView(view);
    setActivePlaylistId(playlistId ?? null);
  }, []);

  const openSettings = useCallback(() => setRoute("settings"), []);

  return (
    <div className="ui-layout" data-testid="echo-shell" data-echo-theme={theme}>
      {status.configured ? (
        <>
          <nav className="ui-sidebar" data-testid="sidebar" aria-label="资料库导航">
            <div className="ui-sidebar-brand">
              <span className="brand-note" aria-hidden="true">
                ♪
              </span>
              <span className="brand-name">Echo</span>
            </div>
            <ul className="ui-nav">
              <li>
                <button
                  type="button"
                  className={`ui-nav-item${route === "library" && libraryView === "all" ? " is-active" : ""}`}
                  onClick={() => selectView("all")}
                  aria-current={route === "library" && libraryView === "all" ? "page" : undefined}
                >
                  全部歌曲
                </button>
              </li>
              <li>
                <button
                  type="button"
                  className={`ui-nav-item${route === "library" && libraryView === "recent" ? " is-active" : ""}`}
                  onClick={() => selectView("recent")}
                  aria-current={
                    route === "library" && libraryView === "recent" ? "page" : undefined
                  }
                >
                  最近添加
                </button>
              </li>
              <li>
                <button
                  type="button"
                  className={`ui-nav-item${route === "library" && libraryView === "favorites" ? " is-active" : ""}`}
                  onClick={() => selectView("favorites")}
                  aria-current={
                    route === "library" && libraryView === "favorites" ? "page" : undefined
                  }
                >
                  喜欢的音乐
                </button>
              </li>
            </ul>
            <PlaylistsNav onSelect={selectView} activePlaylistId={activePlaylistId} />
            <div className="ui-nav-spacer" />
            <button
              type="button"
              className="ui-nav-item"
              onClick={openSettings}
              aria-current={route === "settings" ? "page" : undefined}
            >
              设置
            </button>
          </nav>

          <main className="ui-workspace" data-testid="workspace">
            {route === "settings" ? (
              <SettingsView />
            ) : (
              <>
                {!status.configured || status.unavailable ? (
                  <LibraryStatusView status={status} />
                ) : libraryView === "playlist" && activePlaylistId ? (
                  <PlaylistsView playlistId={activePlaylistId} />
                ) : (
                  <LibraryWorkspace
                    view={libraryView}
                    readOnly={status.readOnly}
                    root={status.activeRoot ?? ""}
                  />
                )}
              </>
            )}
          </main>
        </>
      ) : (
        <ChooseRootView />
      )}

      <PlayerBar />
    </div>
  );
}

/** Sidebar playlist navigation (list + counts). */
function PlaylistsNav(props: {
  onSelect: (view: LibraryViewKind, playlistId?: string | null) => void;
  activePlaylistId: string | null;
}) {
  // Playlists are loaded via the workspace store; a lightweight inline loader
  // keeps the sidebar's data local.
  const playlists = useLibraryPlaylists();
  return (
    <div className="ui-nav-group">
      <div className="ui-nav-group-title">歌单</div>
      {playlists.length === 0 ? (
        <p className="ui-nav-empty">暂无歌单</p>
      ) : (
        <ul>
          {playlists.map((playlist) => (
            <li key={playlist.id}>
              <button
                type="button"
                className={`ui-nav-item${props.activePlaylistId === playlist.id ? " is-active" : ""}`}
                onClick={() => props.onSelect("playlist", playlist.id)}
              >
                <span className="ui-nav-playlist-name">{playlist.name}</span>
                <span className="ui-nav-count">{playlist.memberCount}</span>
              </button>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
