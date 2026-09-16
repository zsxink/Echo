/**
 * Library workspace: 工作区顶栏 + 资料库工具栏 + 歌曲列表 (task 10.4 / 10.5 / 10.6).
 *
 * Reproduces the prototype's `.workspace` (topbar → content → library-view):
 *   topbar      面包屑 · 搜索 · 导入
 *   library-head 当前视图标题 + 计数 + 排序方式（图标按钮 + 排序菜单）
 *   table-wrap   歌曲表格（`.track-table`）
 *
 * It owns the search text, the four sort fields with direction, the row action
 * menu, and the import / add-to-playlist dialogs. Excluded on purpose (一期范围):
 * 同步、全选批量操作、歌单内手动排序、歌曲信息编辑。
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { bridge } from "../../bridge";
import { usePlayerSnapshot } from "../../player/playerStore";
import type { ImportBatchDto, SongView } from "../../ipc/ipc-types.generated";
import { LibraryViewKind, SongSortField } from "./types";
import { bumpLibraryCount, invalidateLibraryCounts } from "./coverPalette";
import { SongList } from "./SongList";
import { useSongs } from "./useSongs";
import { publishSongUpdate, subscribeSongUpdates } from "./songUpdates";
import { SongMenu } from "./SongMenu";
import type { MenuAnchor } from "./SongMenu";
import { AddToPlaylistDialog } from "../playlists/AddToPlaylistDialog";
import { ImportBatchDialog } from "../import/ImportBatchDialog";
import { Icon } from "../../app/Icon";
import { notify } from "../../app/toast";
import { OverlayTier, useFocusTrap, useOverlay } from "../../app/overlays";
import { Topbar } from "../../app/shell";

interface LibraryWorkspaceProps {
  readonly view: LibraryViewKind;
  readonly title: string;
  readonly root: string;
  readonly readOnly: boolean;
  /** A mutation the sidebar also reflects (playlist membership, imports). */
  readonly onLibraryChanged?: () => void;
}

/** The prototype's sort menu: four fields, then a separator and the direction. */
const SORT_FIELDS: readonly { readonly key: SongSortField; readonly label: string }[] = [
  { key: "addedAt", label: "最近添加" },
  { key: "title", label: "歌曲名称" },
  { key: "artist", label: "艺人" },
  { key: "playCount", label: "播放次数" },
];

export function LibraryWorkspace({
  view,
  title,
  root,
  readOnly,
  onLibraryChanged,
}: LibraryWorkspaceProps) {
  const [search, setSearch] = useState("");
  const [sortField, setSortField] = useState<SongSortField>("addedAt");
  const [sortDir, setSortDir] = useState<"asc" | "desc">("desc");
  const [sortOpen, setSortOpen] = useState(false);
  // The menu is anchored to the `.song-more` control that opened it, as the
  // prototype does — never to a fixed corner.
  const [menuFor, setMenuFor] = useState<{ song: SongView; anchor: MenuAnchor } | null>(null);
  const [importOpen, setImportOpen] = useState(false);
  const [addToPlaylistFor, setAddToPlaylistFor] = useState<SongView | null>(null);
  const snapshot = usePlayerSnapshot();
  const sortWrapRef = useRef<HTMLDivElement>(null);

  useOverlay({
    tier: OverlayTier.Menu,
    onClose: () => setSortOpen(false),
    containerRef: sortWrapRef,
    enabled: sortOpen,
  });
  useFocusTrap(sortWrapRef, sortOpen);

  const query = useMemo(
    () => ({
      view,
      search,
      sort: { field: sortField, direction: sortDir },
      inFavorites: view === "favorites",
      root,
      readOnly,
    }),
    [view, search, sortField, sortDir, root, readOnly],
  );
  const { page, loading, error, loadMore, reset, retry, patchSong } = useSongs(query);

  const onFavorite = useCallback(
    (song: SongView, favorite: boolean) => {
      // Optimistic: the sidebar's 喜欢的音乐 count moves now, not one
      // round-trip later. The authoritative re-count that follows corrects it.
      bumpLibraryCount("favorites", favorite ? 1 : -1);
      void bridge
        .call("set_favorite", { songId: song.id, favorite })
        // The command returns the committed authoritative SongView — broadcast
        // it so every surface (this table, the player bar's heart) adopts it
        // immediately instead of waiting for the next full re-query.
        .then((committed) => publishSongUpdate(committed as SongView))
        .catch(() => {
          // A failed mutation leaves the state authoritative — re-render from
          // the server (task 10.7: 过期/失败不覆盖新状态).
          bumpLibraryCount("favorites", favorite ? -1 : 1);
          reset();
        });
    },
    [reset],
  );

  // Adopt song updates published by any surface (this view, the player bar, a
  // playlist) so a toggle made elsewhere is reflected in the loaded rows at
  // once — the broadcast carries the committed view, never an optimistic guess.
  useEffect(() => subscribeSongUpdates(patchSong), [patchSong]);

  const onPlay = useCallback(
    (song: SongView) => {
      // The desktop resolves every page of the declared active-root view. A
      // rendered page is never treated as the playback-context boundary.
      void bridge.call("play_library_context", {
        view: view as "all" | "recent" | "favorites",
        query: search,
        sort: `${sortField}:${sortDir}`,
        selectedSong: song.id,
      });
    },
    [view, search, sortField, sortDir],
  );

  const onPlayNext = useCallback((song: SongView) => {
    // Insert the song right after the current one ("下一首播放", task 10.6).
    void bridge.call("queue_command", { command: "playNext", songId: song.id });
  }, []);

  const onEnqueue = useCallback((song: SongView) => {
    // Append the song to the end of the queue ("加入播放队列", task 10.6).
    void bridge
      .call("queue_command", { command: "enqueue", songId: song.id })
      .then(() => notify(`已将 ${song.title ?? "歌曲"} 加入播放队列`))
      .catch(() => notify({ message: "加入播放队列失败，请重试", error: true }));
  }, []);

  // Only 全部歌曲 accepts user-selected global sorting. Recent and favorites
  // have their own deterministic definitions.
  const showSort = view === "all";

  return (
    <>
      <Topbar title={title}>
        <label className="search" data-testid="search-field">
          <Icon name="search" />
          <input
            type="search"
            placeholder="搜索歌曲、艺人或专辑"
            aria-label="搜索歌曲"
            value={search}
            onChange={(event) => setSearch(event.target.value)}
            data-testid="search-input"
          />
        </label>
        {!readOnly ? (
          <button
            type="button"
            className="btn btn-primary"
            id="import-button"
            aria-busy={importOpen}
            onClick={() => setImportOpen(true)}
            data-testid="import-button"
          >
            导入
          </button>
        ) : null}
      </Topbar>

      <main className="content" data-testid="library-workspace">
        <div className="library-view">
          <div className="library-head">
            <div className="list-context">
              <h1 id="view-title" data-testid="view-title">
                {title}
              </h1>
              <span className="library-total">{page.songs.length} 首</span>
            </div>
            {showSort ? (
              <div className="library-tools">
                <div className="sort-wrap" ref={sortWrapRef}>
                  <button
                    type="button"
                    className="tool-button tool-icon"
                    aria-label="排序方式"
                    title="排序方式"
                    aria-expanded={sortOpen}
                    onClick={() => setSortOpen((open) => !open)}
                    data-testid="sort-button"
                  >
                    <Icon name="sort" />
                  </button>
                  <div
                    className="sort-popover"
                    role="menu"
                    aria-label="排序选项"
                    hidden={!sortOpen}
                  >
                    {SORT_FIELDS.map((field) => (
                      <button
                        key={field.key}
                        type="button"
                        role="menuitemradio"
                        aria-checked={field.key === sortField}
                        className={`sort-option${field.key === sortField ? " active" : ""}`}
                        onClick={() => {
                          setSortField(field.key);
                          setSortOpen(false);
                        }}
                      >
                        <Icon className="sort-check" name="check" />
                        <span>{field.label}</span>
                      </button>
                    ))}
                    <div className="sort-divider" role="separator" />
                    {(["asc", "desc"] as const).map((direction) => (
                      <button
                        key={direction}
                        type="button"
                        role="menuitemradio"
                        aria-checked={direction === sortDir}
                        className={`sort-option${direction === sortDir ? " active" : ""}`}
                        data-sort-direction={direction}
                        onClick={() => {
                          setSortDir(direction);
                          setSortOpen(false);
                        }}
                      >
                        <Icon className="sort-check" name="check" />
                        <span>{direction === "asc" ? "升序" : "降序"}</span>
                      </button>
                    ))}
                  </div>
                </div>
              </div>
            ) : null}
          </div>

          <SongList
            songs={page.songs}
            search={search}
            loading={loading}
            isLast={page.isLast}
            readOnly={readOnly}
            currentSongId={snapshot.currentSongId}
            playing={snapshot.state === "playing"}
            error={error}
            onRetry={retry}
            onImport={() => setImportOpen(true)}
            onLoadMore={loadMore}
            onClearSearch={() => setSearch("")}
            onPlay={onPlay}
            onFavorite={onFavorite}
            onEnqueue={onEnqueue}
            onOpenMenu={(song, anchor) => setMenuFor({ song, anchor })}
          />
        </div>
      </main>

      {menuFor ? (
        <SongMenu
          song={menuFor.song}
          root={root}
          readOnly={readOnly}
          anchor={menuFor.anchor}
          onClose={() => setMenuFor(null)}
          onPlay={() => {
            onPlay(menuFor.song);
            setMenuFor(null);
          }}
          onPlayNext={() => {
            onPlayNext(menuFor.song);
            setMenuFor(null);
          }}
          onEnqueue={() => {
            onEnqueue(menuFor.song);
            setMenuFor(null);
          }}
          onFavorite={(favorite) => {
            onFavorite(menuFor.song, favorite);
            setMenuFor(null);
          }}
          onAddToPlaylist={() => {
            setAddToPlaylistFor(menuFor.song);
            setMenuFor(null);
          }}
          onRefresh={reset}
        />
      ) : null}

      {addToPlaylistFor ? (
        <AddToPlaylistDialog
          songId={addToPlaylistFor.id}
          songTitle={addToPlaylistFor.title ?? "歌曲"}
          root={root}
          onClose={() => setAddToPlaylistFor(null)}
          onDone={() => {
            reset();
            invalidateLibraryCounts();
            onLibraryChanged?.();
          }}
        />
      ) : null}

      {importOpen ? (
        <ImportBatchDialog
          onClose={() => setImportOpen(false)}
          onDone={(batch: ImportBatchDto) => {
            reset();
            invalidateLibraryCounts();
            onLibraryChanged?.();
            notify(
              `导入完成：成功 ${batch.results.filter((r) => r.kind === "imported").length} 首`,
            );
          }}
        />
      ) : null}
    </>
  );
}
