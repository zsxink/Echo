/**
 * App shell data + interaction orchestration (task 6.6).
 *
 * The shell's single job is to compose the routed view, the sidebar/workspace
 * and the floating layers. All of the state it used to hold inline — theme,
 * library status, the active view/playlist, the playlist list, the count-sync
 * subscription, the global hotkeys and the single Escape-stack `Sidebar` layer
 * with its focus trap — is the "多个 hook 编排" the brief calls out. Pulling it
 * here keeps `App.tsx` to the layout alone and makes the wiring independently
 * readable/testable (CODE_STANDARDS §6).
 */

import { useCallback, useRef, useState, type RefObject } from "react";

import { useLibraryPlaylists } from "../features/playlists/useLibraryPlaylists";
import { useLibraryStatus } from "../features/workspace/useLibraryStatus";
import type { LibraryViewKind } from "../features/library/types";
import { useLibraryCountSync } from "../features/library/libraryCounts";
import { useTheme } from "../features/settings/useTheme";
import { useGlobalPlayerHotkeys } from "../player/useGlobalPlayerHotkeys";
import { OverlayTier, useFocusTrap, useOverlay } from "./overlays";
import { type ShellNav } from "./shell";

type LibraryStatus = ReturnType<typeof useLibraryStatus>;
type PlaylistList = ReturnType<typeof useLibraryPlaylists>["playlists"];

export interface AppShell {
  readonly theme: string;
  readonly status: LibraryStatus;
  readonly libraryView: LibraryViewKind;
  readonly activePlaylistId: string | null;
  readonly playlists: PlaylistList;
  readonly reloadPlaylists: () => void;
  readonly selectView: (view: LibraryViewKind, playlistId?: string | null) => void;
  readonly nav: ShellNav;
  readonly sidebarRef: RefObject<HTMLElement>;
  readonly sidebarOpen: boolean;
  readonly closeSidebar: () => void;
  readonly playlistNameOpen: boolean;
  readonly setPlaylistNameOpen: (open: boolean) => void;
  readonly settingsOpen: boolean;
  readonly setSettingsOpen: (open: boolean) => void;
}

export function useAppShell(): AppShell {
  const { theme } = useTheme();
  const status = useLibraryStatus();
  const [libraryView, setLibraryView] = useState<LibraryViewKind>("all");
  const [activePlaylistId, setActivePlaylistId] = useState<string | null>(null);
  const [playlistNameOpen, setPlaylistNameOpen] = useState(false);
  const [settingsOpen, setSettingsOpen] = useState(false);
  // Narrow-screen sidebar toggle (task 12.3): the sidebar collapses behind the
  // topbar button; while open a scrim covers the workspace.
  const [sidebarOpen, setSidebarOpen] = useState(false);
  const sidebarRef = useRef<HTMLElement>(null);
  const { playlists, reload: reloadPlaylists } = useLibraryPlaylists(
    status.configured,
    status.activeRoot,
  );

  // 资料库导航计数 (tasks.md 4.2): fetched up front and re-fetched whenever
  // anything could have changed a total, so "喜欢的音乐" shows its size before
  // the user ever opens it.
  useLibraryCountSync(status.configured && !status.unavailable);

  // Global playback shortcuts (Space toggle, arrows step/volume, M mute,
  // ,/. prev/next) — task 11.8. Ignored while focus is in an input/overlay.
  useGlobalPlayerHotkeys();

  const selectView = useCallback((view: LibraryViewKind, playlistId?: string | null) => {
    setLibraryView(view);
    setActivePlaylistId(playlistId ?? null);
    // Selecting a view closes the narrow-screen sidebar (task 12.3).
    setSidebarOpen(false);
  }, []);

  // The open narrow-screen sidebar is the `Sidebar`-tier layer: it closes last
  // on the single Escape stack, and focus returns to the topbar button.
  useOverlay({
    tier: OverlayTier.Sidebar,
    onClose: () => setSidebarOpen(false),
    containerRef: sidebarRef,
    enabled: sidebarOpen,
  });
  useFocusTrap(sidebarRef, sidebarOpen);

  const nav: ShellNav = {
    sidebarOpen,
    onToggleSidebar: () => setSidebarOpen((open) => !open),
  };

  return {
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
    closeSidebar: () => setSidebarOpen(false),
    playlistNameOpen,
    setPlaylistNameOpen,
    settingsOpen,
    setSettingsOpen,
  };
}
