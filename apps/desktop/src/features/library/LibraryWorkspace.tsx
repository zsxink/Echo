/**
 * Library workspace: 工作区顶栏 + 资料库工具栏 + 歌曲列表 (task 10.4 / 10.5 / 10.6).
 *
 * Reproduces the prototype's `.workspace` (topbar → content → library-view):
 *   topbar      面包屑 · 搜索 · 导入
 *   library-head 当前视图标题 + 计数 + 排序方式（图标按钮 + 排序菜单）
 *   table-wrap   歌曲表格（`.track-table`）
 *
 * It owns the search text, this view's persisted sort, selection, row/batch
 * action menus, and the import / add-to-playlist dialogs.
 */

import { useCallback, useEffect, useMemo, useState } from "react";

import { bridge } from "../../bridge";
import { usePlayerSnapshot } from "../../player/playerStore";
import type { ImportResultDto, SongView } from "../../ipc/ipc-types.generated";
import { LibraryViewKind } from "./types";
import { bumpLibraryCount, invalidateLibraryCounts } from "./libraryCounts";
import { invalidateLibrary } from "./libraryInvalidation";
import { SongList } from "./SongList";
import { useSongs } from "./useSongs";
import { publishSongUpdate, subscribeSongUpdates } from "./songUpdates";
import { SongMenu } from "./SongMenu";
import type { MenuAnchor } from "./SongMenu";
import { BatchSongMenu, SelectionModeButton } from "./BatchSongActions";
import {
  formatBatchFailureDetails,
  formatBatchResult,
  runDeleteBatch,
  runFavoriteBatch,
  runQueueBatch,
  undoDeleteBatch,
} from "./batchOperations";
import type { BatchResult } from "./batchOperations";
import { useSongSelection } from "./useSongSelection";
import { ConfirmationDialog } from "./ConfirmationDialog";
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
  const [addToPlaylistFor, setAddToPlaylistFor] = useState<readonly SongView[] | null>(null);
  const [batchMenuFor, setBatchMenuFor] = useState<{
    readonly song: SongView;
    readonly anchor: MenuAnchor;
  } | null>(null);
  const [batchDeleteFor, setBatchDeleteFor] = useState<readonly SongView[] | null>(null);
  const [batchBusy, setBatchBusy] = useState(false);
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
  const selectionKey = useMemo(
    () => [view, search.trim().toLowerCase(), sort.field, sort.direction, root].join("|"),
    [view, search, sort, root],
  );
  const selection = useSongSelection(selectionKey);
  const clearSelection = selection.clear;
  const [selectionMode, setSelectionMode] = useState(false);
  const exitSelectionMode = useCallback(() => {
    selection.clear();
    setBatchMenuFor(null);
    setBatchDeleteFor(null);
    setSelectionMode(false);
  }, [selection]);
  useEffect(() => {
    // Navigation between library views must not carry a prior view's bulk
    // mode or selection into the newly opened list.
    clearSelection();
    setBatchMenuFor(null);
    setBatchDeleteFor(null);
    setSelectionMode(false);
  }, [clearSelection, view]);
  const selectedSongs = useMemo(
    () => page.songs.filter((song) => selection.selectedIds.has(song.id)),
    [page.songs, selection.selectedIds],
  );
  const allLoadedSelected =
    page.songs.length > 0 && page.songs.every((song) => selection.selectedIds.has(song.id));

  const runImport = useCallback(async () => {
    if (importing) return;
    setImporting(true);
    try {
      const batch = await bridge.call("choose_and_import_files");
      if (batch === null) return;
      const feedback = classifyImportResults(batch.results);
      if (feedback.nonFailures.length > 0) invalidateLibrary();
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
  }, [importing, onLibraryChanged]);

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

  const notifyBatch = useCallback((result: BatchResult<SongView>) => {
    const details = formatBatchFailureDetails(result, (song) => song.title ?? "未命名歌曲");
    notify({
      message: `批量操作：${formatBatchResult(result)}${details ? `。${details}` : ""}`,
      error: result.failed > 0,
    });
  }, []);

  const applyBatchFavorite = useCallback(
    async (favorite: boolean) => {
      const targets = selectedSongs.filter(
        (song) => song.availability === "available" && song.favorite !== favorite,
      );
      if (
        targets.length === 0 &&
        selectedSongs.every((song) => song.availability === "available")
      ) {
        notify("没有需要变更的歌曲");
        return;
      }
      setBatchBusy(true);
      try {
        bumpLibraryCount("favorites", favorite ? targets.length : -targets.length);
        const result = await runFavoriteBatch(selectedSongs, favorite);
        for (const item of result.items) {
          if (item.status === "success" && item.value) publishSongUpdate(item.value);
        }
        if (result.failed > 0)
          bumpLibraryCount("favorites", favorite ? -result.failed : result.failed);
        reset();
        invalidateLibraryCounts();
        notifyBatch(result);
        exitSelectionMode();
      } catch {
        notify({ message: "批量操作失败，请重试", error: true });
      } finally {
        setBatchBusy(false);
      }
    },
    [exitSelectionMode, notifyBatch, reset, selectedSongs],
  );

  const applyBatchQueue = useCallback(
    async (command: "enqueue" | "playNext") => {
      setBatchBusy(true);
      try {
        const result = await runQueueBatch(selectedSongs, command);
        notifyBatch(result);
        exitSelectionMode();
      } catch {
        notify({ message: "批量操作失败，请重试", error: true });
      } finally {
        setBatchBusy(false);
      }
    },
    [exitSelectionMode, notifyBatch, selectedSongs],
  );

  const openBatchMenu = useCallback(
    (song: SongView, anchor: MenuAnchor) => {
      if (!selection.isSelected(song.id)) selection.replace(song.id);
      setBatchMenuFor({ song, anchor });
    },
    [selection],
  );

  const batchMenuSongs = useMemo(() => {
    if (!batchMenuFor) return [];
    if (selectedSongs.some((song) => song.id === batchMenuFor.song.id)) return selectedSongs;
    return [batchMenuFor.song];
  }, [batchMenuFor, selectedSongs]);

  const batchHandlers = useMemo(
    () => ({
      onFavorite: (favorite: boolean) => {
        setBatchMenuFor(null);
        void applyBatchFavorite(favorite);
      },
      onAddToPlaylist: () => {
        setBatchMenuFor(null);
        setAddToPlaylistFor(batchMenuSongs);
      },
      onPlayNext: () => {
        setBatchMenuFor(null);
        void applyBatchQueue("playNext");
      },
      onEnqueue: () => {
        setBatchMenuFor(null);
        void applyBatchQueue("enqueue");
      },
      onDelete: () => {
        setBatchMenuFor(null);
        setBatchDeleteFor(batchMenuSongs);
      },
    }),
    [applyBatchFavorite, applyBatchQueue, batchMenuSongs],
  );

  const confirmBatchDelete = useCallback(async () => {
    if (!batchDeleteFor) return;
    const songs = batchDeleteFor;
    setBatchDeleteFor(null);
    setBatchBusy(true);
    const result = await runDeleteBatch(root, songs);
    reset();
    invalidateLibraryCounts();
    exitSelectionMode();
    setBatchBusy(false);
    const operations = result.items.flatMap((item) =>
      item.status === "success" && item.value ? [item.value] : [],
    );
    const details = result.items
      .filter((item) => item.status !== "success")
      .map((item) => `${item.item.title ?? "未命名歌曲"}：${item.message ?? "未完成"}`)
      .join("；");
    const baseMessage = `批量删除：${formatBatchResult(result)}`;
    if (operations.length === 0) {
      notify({ message: details ? `${baseMessage}。${details}` : baseMessage, error: true });
      return;
    }
    notify({
      message: `${baseMessage}，10 秒内可撤销${details ? `。${details}` : ""}`,
      actionLabel: "撤销",
      autoDismissMs: 10_000,
      onAction: () => {
        void undoDeleteBatch(root, operations).then((undo) => {
          reset();
          invalidateLibraryCounts();
          notify({
            message: `撤回删除：成功 ${undo.succeeded}，失败 ${undo.failed}`,
            error: undo.failed > 0,
          });
        });
      },
    });
  }, [batchDeleteFor, exitSelectionMode, reset, root]);

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
            <div className="library-tools">
              {showSort ? <SongSortControl sort={sort} onChange={setSort} /> : null}
              <SelectionModeButton
                active={selectionMode}
                onToggle={() => {
                  if (selectionMode) exitSelectionMode();
                  else setSelectionMode(true);
                }}
              />
            </div>
          </div>

          <SongList
            songs={page.songs}
            search={search}
            loading={loading}
            isLast={page.isLast}
            readOnly={readOnly}
            currentSongId={snapshot.currentSongId}
            playing={snapshot.state === "playing"}
            selectionMode={selectionMode}
            selectedIds={selection.selectedIds}
            allLoadedSelected={allLoadedSelected}
            onToggleSelection={(song) => selection.toggle(song.id)}
            onToggleSelectAll={() => selection.toggleAllLoaded(page.songs.map((song) => song.id))}
            onContextMenu={selectionMode ? openBatchMenu : undefined}
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
            setAddToPlaylistFor([menuFor.song]);
            setMenuFor(null);
          }}
          onRefresh={reset}
        />
      ) : null}

      {selectionMode && batchMenuFor && batchMenuSongs.length > 0 ? (
        <BatchSongMenu
          anchor={batchMenuFor.anchor}
          songs={batchMenuSongs}
          readOnly={readOnly || batchBusy}
          inPlaylist={false}
          handlers={batchHandlers}
          onClose={() => setBatchMenuFor(null)}
        />
      ) : null}

      {addToPlaylistFor ? (
        <AddToPlaylistDialog
          songIds={addToPlaylistFor.map((song) => song.id)}
          songs={addToPlaylistFor}
          songTitle={addToPlaylistFor.length === 1 ? addToPlaylistFor[0]?.title : undefined}
          root={root}
          readOnly={readOnly || batchBusy}
          onClose={() => setAddToPlaylistFor(null)}
          onDone={() => {
            reset();
            invalidateLibraryCounts();
            onLibraryChanged?.();
            exitSelectionMode();
          }}
        />
      ) : null}

      {batchDeleteFor ? (
        <ConfirmationDialog
          title={`删除已选 ${batchDeleteFor.length} 首歌曲？`}
          description="可删除歌曲会移至回收站；不可删除项会在结果中明确列出。整批成功项可在 10 秒内撤回。"
          confirmLabel="批量移至回收站"
          onConfirm={() => void confirmBatchDelete()}
          onCancel={() => setBatchDeleteFor(null)}
        />
      ) : null}

      {importFailures ? (
        <ImportFailureDialog results={importFailures} onClose={() => setImportFailures(null)} />
      ) : null}
    </>
  );
}
