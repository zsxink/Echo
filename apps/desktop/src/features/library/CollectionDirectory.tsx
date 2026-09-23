/**
 * Artist and album directory for the local library.
 *
 * Core owns grouping and stable identities; this component only retains the
 * current directory/detail request so a late result cannot replace a newer
 * view, search, or active-root selection.
 *
 * The opened artist/album song list reuses the 全部歌曲 `SongList` table (same
 * columns, windowing, bulk-selection mode, per-row ⋯ menu and batch menus), so
 * every capability of the library view — 加入歌单 / 下一首播放 / 加入播放队列 /
 * 收藏 / 删除 — behaves identically here.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { bridge } from "../../bridge";
import { Icon } from "../../app/Icon";
import { Topbar } from "../../app/shell";
import { notify } from "../../app/toast";
import { usePlayerSnapshot } from "../../player/playerStore";
import type { CatalogCollectionView, SongView } from "../../ipc/ipc-types.generated";
import { coverClass } from "./coverClass";
import { bumpLibraryCount, invalidateLibraryCounts } from "./libraryCounts";
import { subscribeLibraryInvalidations } from "./libraryInvalidation";
import { publishSongUpdate, subscribeSongUpdates } from "./songUpdates";
import { SongList } from "./SongList";
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
import { AddToPlaylistDialog } from "../playlists";
import { SortMenu } from "./SortMenu";
import { useStoredDirectorySort } from "./useStoredDirectorySort";

export interface CollectionDirectoryProps {
  readonly kind: "artist" | "album";
  readonly root: string;
  /** Defaults to false; the shell passes the active root's real state. */
  readonly readOnly?: boolean;
  /** A mutation the sidebar also reflects (playlist membership, deletions). */
  readonly onLibraryChanged?: () => void;
}

export function CollectionDirectory({
  kind,
  root,
  readOnly = false,
  onLibraryChanged,
}: CollectionDirectoryProps) {
  const [search, setSearch] = useState("");
  const [entries, setEntries] = useState<readonly CatalogCollectionView[]>([]);
  const [selected, setSelected] = useState<CatalogCollectionView | null>(null);
  const [songs, setSongs] = useState<readonly SongView[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [refreshEpoch, setRefreshEpoch] = useState(0);
  // Directory and detail queries are independent resources: each keeps its own
  // generation counter so that a late response of one can never invalidate the
  // other. Sharing a single counter is how 歌手 → 专辑 (and back) ended up on an
  // empty directory: switching while a collection was open made the *detail*
  // effect bump the shared counter with the previous `kind`, so the directory
  // response — the one that actually carries the new view — was discarded as
  // stale and the grid stayed empty under the new chrome.
  const directoryRequest = useRef(0);
  const detailRequest = useRef(0);
  const title = kind === "artist" ? "歌手" : "专辑";
  const [sort, setSort] = useStoredDirectorySort(kind);
  const sortOptions =
    kind === "artist"
      ? [
          { value: "name" as const, label: "歌手名称" },
          { value: "songCount" as const, label: "歌曲数量" },
        ]
      : [
          { value: "name" as const, label: "专辑名称" },
          { value: "songCount" as const, label: "歌曲数量" },
        ];
  const orderedEntries = useMemo(() => {
    const compareText = (left: string, right: string) => (left < right ? -1 : left > right ? 1 : 0);
    const nameKey = (entry: CatalogCollectionView) =>
      kind === "artist" ? entry.artistKey : (entry.albumKey ?? "");
    return [...entries].sort((left, right) => {
      const primary =
        sort.field === "name"
          ? compareText(nameKey(left), nameKey(right))
          : left.songCount - right.songCount;
      const directed = sort.direction === "asc" ? primary : -primary;
      if (directed !== 0) return directed;
      const byName = compareText(nameKey(left), nameKey(right));
      if (byName !== 0) return byName;
      return compareText(
        `${left.artistKey}:${left.albumKey ?? ""}`,
        `${right.artistKey}:${right.albumKey ?? ""}`,
      );
    });
  }, [entries, kind, sort.direction, sort.field]);
  const [selectionMode, setSelectionMode] = useState(false);
  const selectionKey = useMemo(
    () => [kind, root, selected?.artistKey ?? "", selected?.albumKey ?? "", search].join("|"),
    [kind, root, search, selected],
  );
  const selection = useSongSelection(selectionKey);
  const clearSelection = selection.clear;
  const allSongsSelected =
    songs.length > 0 && songs.every((song) => selection.selectedIds.has(song.id));
  const selectedSongs = useMemo(
    () => songs.filter((song) => selection.selectedIds.has(song.id)),
    [songs, selection.selectedIds],
  );
  const snapshot = usePlayerSnapshot();

  const [menuFor, setMenuFor] = useState<{ song: SongView; anchor: MenuAnchor } | null>(null);
  const [batchMenuFor, setBatchMenuFor] = useState<{
    readonly song: SongView;
    readonly anchor: MenuAnchor;
  } | null>(null);
  const [batchDeleteFor, setBatchDeleteFor] = useState<readonly SongView[] | null>(null);
  const [addToPlaylistFor, setAddToPlaylistFor] = useState<readonly SongView[] | null>(null);
  const [batchBusy, setBatchBusy] = useState(false);

  const exitSelectionMode = useCallback(() => {
    clearSelection();
    setBatchMenuFor(null);
    setBatchDeleteFor(null);
    setSelectionMode(false);
  }, [clearSelection]);

  const refreshSongs = useCallback(() => setRefreshEpoch((epoch) => epoch + 1), []);

  useEffect(() => {
    setSelected(null);
    // The directory entries must go with them: this instance survives a
    // 歌手 ↔ 专辑 switch (same tree position in App), so leftover artist
    // cards would otherwise paint under album chrome until the refetch lands
    // — or forever, if the refetch fails.
    setEntries([]);
    setSongs([]);
    setSelectionMode(false);
  }, [kind, root]);

  useEffect(() => {
    // Opening another artist/album must not inherit the previous directory's
    // bulk mode or selection.
    clearSelection();
    setBatchMenuFor(null);
    setBatchDeleteFor(null);
    setSelectionMode(false);
  }, [clearSelection, selected]);

  useEffect(() => {
    const id = ++directoryRequest.current;
    setLoading(true);
    setError(null);
    void bridge
      .call("catalog_collections", { kind, search })
      .then((result) => {
        if (directoryRequest.current === id) setEntries(result);
      })
      .catch(() => {
        // Task 10.7: a failed refetch keeps the content already on screen and
        // surfaces the error rather than wiping the view. The stale-view case
        // (this `entries` list belonging to a *different* kind/root) is already
        // handled where the identity changes, not here.
        if (directoryRequest.current === id) setError("加载目录失败，请重试");
      })
      .finally(() => {
        if (directoryRequest.current === id) setLoading(false);
      });
  }, [kind, root, search, refreshEpoch]);

  useEffect(() => {
    // A selection belonging to the previous kind is already on its way out
    // (the identity effect above clears it) — querying the new kind with the
    // old collection's keys would only ask for something that does not exist.
    if (!selected || selected.kind !== kind) return;
    const id = ++detailRequest.current;
    setLoading(true);
    setError(null);
    void bridge
      .call("catalog_collection_songs", {
        kind,
        artistKey: selected.artistKey,
        albumKey: selected.albumKey ?? null,
        search,
      })
      .then((result) => {
        if (detailRequest.current === id) setSongs(result);
      })
      .catch(() => {
        if (detailRequest.current === id) setError("加载歌曲失败，请重试");
      })
      .finally(() => {
        if (detailRequest.current === id) setLoading(false);
      });
  }, [kind, root, search, selected, refreshEpoch]);

  useEffect(() => subscribeLibraryInvalidations(() => setRefreshEpoch((epoch) => epoch + 1)), []);

  // Adopt song updates published by any surface (the player bar, a playlist,
  // another view) so a favorite toggle made elsewhere is reflected in the
  // loaded rows at once — the broadcast carries the committed view.
  useEffect(
    () =>
      subscribeSongUpdates((updated) => {
        setSongs((current) => current.map((song) => (song.id === updated.id ? updated : song)));
      }),
    [],
  );

  const play = useCallback(
    (song: SongView) => {
      const collectionView = kind === "artist" ? "artist" : "album";
      bridge.fireAndForget("play_library_context", {
        view: collectionView,
        query: search,
        sort: "addedAt:desc",
        selectedSong: song.id,
        artistKey: selected?.artistKey,
        albumKey: selected?.albumKey,
      });
    },
    [kind, search, selected],
  );

  const onPlayNext = useCallback((song: SongView) => {
    bridge.fireAndForget("queue_command", { command: "playNext", songId: song.id });
  }, []);

  const onEnqueue = useCallback((song: SongView) => {
    void bridge
      .call("queue_command", { command: "enqueue", songId: song.id })
      .then(() => notify(`已将 ${song.title ?? "歌曲"} 加入播放队列`))
      .catch(() => notify({ message: "加入播放队列失败，请重试", error: true }));
  }, []);

  const onFavorite = useCallback((song: SongView, favorite: boolean) => {
    // Optimistic sidebar count, reverted when the command fails — same deal as
    // the library workspace.
    bumpLibraryCount("favorites", favorite ? 1 : -1);
    void bridge
      .call("set_favorite", { songId: song.id, favorite })
      .then((committed) => publishSongUpdate(committed as SongView))
      .catch(() => {
        bumpLibraryCount("favorites", favorite ? -1 : 1);
        notify({ message: "收藏失败，请重试", error: true });
      });
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
        refreshSongs();
        invalidateLibraryCounts();
        notifyBatch(result);
        exitSelectionMode();
      } catch {
        notify({ message: "批量操作失败，请重试", error: true });
      } finally {
        setBatchBusy(false);
      }
    },
    [exitSelectionMode, notifyBatch, refreshSongs, selectedSongs],
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
    const targets = batchDeleteFor;
    setBatchDeleteFor(null);
    setBatchBusy(true);
    const result = await runDeleteBatch(root, targets);
    refreshSongs();
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
          refreshSongs();
          invalidateLibraryCounts();
          notify({
            message: `撤回删除：成功 ${undo.succeeded}，失败 ${undo.failed}`,
            error: undo.failed > 0,
          });
        });
      },
    });
  }, [batchDeleteFor, exitSelectionMode, refreshSongs, root]);

  const detailTitle = selected ? selected.name : title;
  const detailSub = selected
    ? kind === "artist"
      ? `${selected.songCount} 首歌曲`
      : `${selected.artist} · ${selected.songCount} 首歌曲`
    : null;

  return (
    <>
      <Topbar title={detailTitle}>
        <label className="search" data-testid="collection-search-field">
          <Icon name="search" />
          <input
            type="search"
            placeholder={kind === "artist" ? "搜索歌手" : "搜索专辑或歌手"}
            aria-label={kind === "artist" ? "搜索歌手" : "搜索专辑"}
            value={search}
            onChange={(event) => setSearch(event.target.value)}
          />
        </label>
      </Topbar>

      <main className="content" data-testid={`${kind}-directory`}>
        <div className="library-view">
          <div className="library-head">
            <div className="list-context">
              {selected ? (
                <button
                  className="collection-back is-visible"
                  type="button"
                  aria-label={`返回${title}`}
                  onClick={() => setSelected(null)}
                >
                  <Icon name="chevronLeft" />
                </button>
              ) : null}
              <div>
                <h1>{detailTitle}</h1>
                {detailSub ? <span className="library-total">{detailSub}</span> : null}
              </div>
            </div>
            {selected ? (
              <div className="library-tools">
                <SelectionModeButton
                  active={selectionMode}
                  onToggle={() => {
                    if (selectionMode) exitSelectionMode();
                    else setSelectionMode(true);
                  }}
                />
              </div>
            ) : (
              <div className="library-tools">
                <SortMenu sort={sort} options={sortOptions} onChange={setSort} />
              </div>
            )}
          </div>

          {selected ? (
            <SongList
              songs={songs}
              search={search}
              loading={loading}
              // The directory query returns the whole collection at once —
              // there is no keyset pagination to continue.
              isLast
              readOnly={readOnly}
              currentSongId={snapshot.currentSongId}
              playing={snapshot.state === "playing"}
              selectionMode={selectionMode}
              selectedIds={selection.selectedIds}
              allLoadedSelected={allSongsSelected}
              onToggleSelection={(song) => selection.toggle(song.id)}
              onToggleSelectAll={() => selection.toggleAllLoaded(songs.map((song) => song.id))}
              onContextMenu={(song, anchor) => {
                if (selectionMode) openBatchMenu(song, anchor);
                else setMenuFor({ song, anchor });
              }}
              error={error}
              onRetry={refreshSongs}
              onLoadMore={() => {}}
              onClearSearch={() => setSearch("")}
              onPlay={play}
              onFavorite={onFavorite}
              onPlayNext={onPlayNext}
              onOpenMenu={(song, anchor) => setMenuFor({ song, anchor })}
            />
          ) : (
            <section className="collection-directory" aria-busy={loading}>
              <div className="collection-grid">
                {orderedEntries.map((entry) => (
                  <article
                    className="collection-card"
                    key={`${entry.artistKey}:${entry.albumKey ?? ""}`}
                  >
                    <button
                      type="button"
                      className="collection-open"
                      aria-label={`打开${title} ${entry.name}`}
                      onClick={() => setSelected(entry)}
                    >
                      <span
                        className={`collection-art cover ${coverClass(entry.artistKey)}${entry.coverKey ? " has-image" : ""}`}
                        aria-hidden="true"
                      >
                        {entry.coverKey ? (
                          <img src={bridge.assetUrl(entry.coverKey)} alt="" />
                        ) : null}
                      </span>
                      <span className="collection-copy">
                        <strong>{entry.name}</strong>
                        <span>
                          {kind === "artist"
                            ? `${entry.songCount} 首歌曲`
                            : `${entry.artist} · ${entry.songCount} 首歌曲`}
                        </span>
                      </span>
                      <span className="collection-chevron" aria-hidden="true">
                        <Icon name="chevronRight" />
                      </span>
                    </button>
                  </article>
                ))}
              </div>
              {!loading && entries.length === 0 ? (
                <div className="collection-empty show">
                  <h2>没有找到匹配的内容</h2>
                  <p>试试其他{title}名称。</p>
                </div>
              ) : null}
            </section>
          )}
          {!selected && error ? <p className="collection-error">{error}</p> : null}
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
            play(menuFor.song);
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
          onRefresh={refreshSongs}
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
            refreshSongs();
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
    </>
  );
}
