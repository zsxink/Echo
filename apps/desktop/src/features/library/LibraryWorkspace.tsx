/**
 * Library workspace: 工作区顶栏 + 资料库工具栏 + 歌曲列表 (task 10.4 / 10.5 / 10.6).
 *
 * Reproduces the prototype's `.workspace` (topbar → content → library-view):
 *   topbar      面包屑 · 搜索 · 导入
 *   library-head 当前视图标题 + 计数 + 排序方式（图标按钮 + 排序菜单）
 *   table-wrap   歌曲表格（`.track-table`）
 *
 * It owns the search text, this view's persisted sort, the row action menu,
 * and the import / add-to-playlist dialogs. Excluded on purpose (一期范围):
 * 同步、全选批量操作、歌曲信息编辑。
 */

import { useCallback, useEffect, useMemo, useState } from "react";

import { bridge } from "../../bridge";
import { usePlayerSnapshot } from "../../player/playerStore";
import type { ImportResultDto, SongView } from "../../ipc/ipc-types.generated";
import { LibraryViewKind } from "./types";
import { bumpLibraryCount, invalidateLibraryCounts } from "./libraryCounts";
import { SongList } from "./SongList";
import { useSongs } from "./useSongs";
import { publishSongUpdate, subscribeSongUpdates } from "./songUpdates";
import { SongMenu } from "./SongMenu";
import type { MenuAnchor } from "./SongMenu";
import { ImportFailureDialog, classifyImportResults } from "../import";
import { AddToPlaylistDialog } from "../playlists";
import { SongSortControl } from "./SongSortControl";
import { useStoredSongSort } from "./useStoredSongSort";
import { Icon } from "../../app/Icon";
import { notify } from "../../app/toast";
import { Topbar } from "../../app/shell";

interface LibraryWorkspaceProps {
  readonly view: LibraryViewKind;
  readonly title: string;
  readonly root: string;
  readonly readOnly: boolean;
  /** A mutation the sidebar also reflects (playlist membership, imports). */
  readonly onLibraryChanged?: () => void;
}

export function LibraryWorkspace({
  view,
  title,
  root,
  readOnly,
  onLibraryChanged,
}: LibraryWorkspaceProps) {
  const [search, setSearch] = useState("");
  const [sort, setSort] = useStoredSongSort(view);
  // The menu is anchored to the `.song-more` control that opened it, as the
  // prototype does — never to a fixed corner.
  const [menuFor, setMenuFor] = useState<{ song: SongView; anchor: MenuAnchor } | null>(null);
  const [importing, setImporting] = useState(false);
  const [importFailures, setImportFailures] = useState<readonly ImportResultDto[] | null>(null);
  const [addToPlaylistFor, setAddToPlaylistFor] = useState<SongView | null>(null);
  const snapshot = usePlayerSnapshot();
  const query = useMemo(
    () => ({
      view,
      search,
      sort,
      inFavorites: view === "favorites",
      root,
      readOnly,
    }),
    [view, search, sort, root, readOnly],
  );
  const { page, loading, error, loadMore, reset, retry, patchSong } = useSongs(query);

  const runImport = useCallback(async () => {
    if (importing) return;
    setImporting(true);
    try {
      const batch = await bridge.call("choose_and_import_files");
      if (batch === null) return;
      const feedback = classifyImportResults(batch.results);
      reset();
      invalidateLibraryCounts();
      onLibraryChanged?.();
      if (feedback.nonFailures.length > 0) notify(feedback.summary);
      if (feedback.failures.length > 0) setImportFailures(feedback.failures);
    } catch (error) {
      const message = error instanceof Error ? error.message : "导入命令失败";
      setImportFailures([
        {
          kind: "failed",
          code: "import_command",
          message,
        },
      ]);
    } finally {
      setImporting(false);
    }
  }, [importing, onLibraryChanged, reset]);

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
      bridge.fireAndForget("play_library_context", {
        view: view as "all" | "recent" | "favorites",
        query: search,
        sort: `${query.sort.field}:${query.sort.direction}`,
        selectedSong: song.id,
      });
    },
    [view, search, query.sort],
  );

  const onPlayNext = useCallback((song: SongView) => {
    // Insert the song right after the current one ("下一首播放", task 10.6).
    bridge.fireAndForget("queue_command", { command: "playNext", songId: song.id });
  }, []);

  const onEnqueue = useCallback((song: SongView) => {
    // Append the song to the end of the queue ("加入播放队列", task 10.6).
    void bridge
      .call("queue_command", { command: "enqueue", songId: song.id })
      .then(() => notify(`已将 ${song.title ?? "歌曲"} 加入播放队列`))
      .catch(() => notify({ message: "加入播放队列失败，请重试", error: true }));
  }, []);

  // 最近添加 is intentionally a fixed chronological view. 全部歌曲 and 喜欢的
  // 音乐 each own a saved menu selection rather than sharing one global sort.
  const showSort = view !== "recent";

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
            aria-busy={importing}
            disabled={importing}
            onClick={() => void runImport()}
            data-testid="import-button"
          >
            {importing ? "导入中…" : "导入"}
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
                <SongSortControl sort={sort} onChange={setSort} />
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
            onImport={() => void runImport()}
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

      {importFailures ? (
        <ImportFailureDialog results={importFailures} onClose={() => setImportFailures(null)} />
      ) : null}
    </>
  );
}
