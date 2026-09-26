/**
 * Playlist content view (task 10.9).
 *
 * A playlist is *the same workspace view* the prototype shows for 全部歌曲 —
 * `header.topbar` + `main.content` + `.library-view` (`.library-head` with the
 * playlist title, its member count and the view tools, then the shared
 * `.table-wrap` song table). It used to sit inside its own `.workspace` grid,
 * which is why it read as a different application.
 *
 * Playlist-level actions live in `.library-tools` as prototype `.tool-button`s
 * (编辑歌单 / 删除歌单), matching the standalone prototype. Deleting a playlist
 * never deletes a song file.
 */

import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { bridge } from "../../bridge";
import { usePlayerSnapshot } from "../../player/playerStore";
import type { PagedSongs, SongView } from "../../ipc/ipc-types.generated";
import {
  BatchSongMenu,
  SelectionModeButton,
  formatBatchFailureDetails,
  formatBatchResult,
  runDeleteBatch,
  runFavoriteBatch,
  runQueueBatch,
  runRemoveFromPlaylistBatch,
  undoDeleteBatch,
  useSongSelection,
  type BatchResult,
  SongList,
  SORT_FIELDS,
  SongSortControl,
  useStoredSongSort,
  bumpLibraryCount,
  invalidateLibraryCounts,
  publishSongUpdate,
  ConfirmationDialog,
  SongMenu,
  type MenuAnchor,
  type SongSort,
  useLocateSong,
  LocateButton,
} from "../library";
import { PlaylistNameDialog } from "./PlaylistNameDialog";
import { AddToPlaylistDialog } from "./AddToPlaylistDialog";
import { Icon } from "../../app/Icon";
import { Topbar } from "../../app/shell";
import { notify } from "../../app/toast";

export interface PlaylistsViewProps {
  readonly playlistId: string;
  /** The playlist's display name, resolved by the shell. */
  readonly title: string;
  readonly coverKey?: string;
  readonly automaticCoverKey?: string;
  readonly hasCustomCover?: boolean;
  readonly root: string;
  /** Authoritative sibling names from the shell, for local rename validation. */
  readonly existingNames: readonly string[];
  readonly readOnly: boolean;
  /** The playlist no longer exists — the shell returns to 全部歌曲. */
  readonly onDeleted?: () => void;
  /** A playlist mutation landed; the shell re-reads the sidebar list. */
  readonly onLibraryChanged?: () => void;
}

export function PlaylistsView({
  playlistId,
  title,
  coverKey,
  automaticCoverKey,
  hasCustomCover = false,
  root,
  existingNames,
  readOnly,
  onDeleted,
  onLibraryChanged,
}: PlaylistsViewProps) {
  const [members, setMembers] = useState<readonly SongView[]>([]);
  const [sort, setSort] = useStoredSongSort(`playlist:${playlistId}`);
  const [name, setName] = useState(title);
  const [searchText, setSearchText] = useState("");
  const [searching, setSearching] = useState(false);
  // 歌单内搜索（playlist-search-locate-import）：非空搜索词由后端 `search`
  // 命令在“当前歌单”范围内分页返回；空词时 `searched` 为 null，视图回退到
  // 全量成员列表（保留追加顺序语义与批量操作）。
  const [searched, setSearched] = useState<PagedSongs | null>(null);
  const searchRequest = useRef(0);
  const [menuFor, setMenuFor] = useState<{ song: SongView; anchor: MenuAnchor } | null>(null);
  const [batchMenuFor, setBatchMenuFor] = useState<{
    readonly song: SongView;
    readonly anchor: MenuAnchor;
  } | null>(null);
  const [addToPlaylistFor, setAddToPlaylistFor] = useState<readonly SongView[] | null>(null);
  const [batchDeleteFor, setBatchDeleteFor] = useState<readonly SongView[] | null>(null);
  const [batchBusy, setBatchBusy] = useState(false);
  const [renaming, setRenaming] = useState(false);
  const [confirmDelete, setConfirmDelete] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);
  const request = useRef(0);
  const snapshot = usePlayerSnapshot();
  const { locateSongId, onLocate, onLocateSettled } = useLocateSong();
  const selectionKey = `${playlistId}|${sort.field}|${sort.direction}`;
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
    // A different playlist is a different selection context; leave multi-
    // select before showing its members.
    clearSelection();
    setBatchMenuFor(null);
    setBatchDeleteFor(null);
    setSelectionMode(false);
  }, [clearSelection, playlistId]);

  // The shell owns the list; keep the optimistic name in step with it.
  useEffect(() => setName(title), [title]);

  const loadMembers = useCallback(() => {
    const id = ++request.current;
    void bridge
      .call("playlist_members", { playlistId })
      // A malformed payload degrades to an empty list instead of tearing down
      // the whole view (the IPC contract says this is always an array).
      .then((value: unknown) => {
        if (id !== request.current) return;
        if (Array.isArray(value)) {
          // The native command already returns newest playlist additions
          // first. Keep that order for both the rendered list and playback.
          setMembers(value as SongView[]);
          clearSelection();
          setLoadError(null);
        }
      })
      .catch(() => {
        if (id === request.current) setLoadError("加载歌单失败，请重试");
      });
  }, [clearSelection, playlistId]);

  useEffect(loadMembers, [loadMembers]);

  // 歌单内搜索：非空词走后端 `search`（限定当前歌单），空词清空搜索结果
  // 让视图回退到全量成员。相同 playlistId + 排序下改变搜索词才重新请求。
  const playlistsMemberSearch = useCallback(
    (query: string) => {
      const trimmed = query.trim();
      const id = ++searchRequest.current;
      if (trimmed.length === 0) {
        setSearched(null);
        setSearching(false);
        return;
      }
      setSearching(true);
      void bridge
        .call("search", {
          query: trimmed,
          inFavorites: false,
          // IPC key is `playlist` (Tauri's camelCase of Rust's `playlist` param).
          playlist: playlistId,
          sort: `${sort.field}:${sort.direction}`,
          cursor: null,
          limit: 200,
        })
        .then((value) => {
          if (id !== searchRequest.current) return;
          setSearched(value as PagedSongs);
          setSearching(false);
          clearSelection();
        })
        .catch(() => {
          if (id !== searchRequest.current) return;
          setSearching(false);
          notify({ message: "搜索歌单失败，请重试", error: true });
        });
    },
    [clearSelection, playlistId, sort],
  );

  useEffect(() => {
    playlistsMemberSearch(searchText);
  }, [playlistsMemberSearch, searchText]);

  const refreshAfterSongMutation = useCallback(() => {
    loadMembers();
    onLibraryChanged?.();
  }, [loadMembers, onLibraryChanged]);

  async function deletePlaylist() {
    setConfirmDelete(false);
    try {
      await bridge.call("delete_playlist", { id: playlistId });
      invalidateLibraryCounts();
      onLibraryChanged?.();
      onDeleted?.();
    } catch {
      notify({ message: "删除歌单失败", error: true });
    }
  }

  const onPlay = useCallback(
    (song: SongView) => {
      // The desktop resolves the playlist context itself; the UI submits only
      // the selected song plus the active in-playlist filter. A search-preset
      // view therefore plays exactly the filtered queue, never the whole
      // playlist (spec: 歌单搜索后播放队列与筛选后列表一致).
      bridge.fireAndForget("play_playlist_context", {
        playlist: playlistId,
        selectedSong: song.id,
        query: searchText.trim() || null,
      });
    },
    [playlistId, searchText],
  );

  const onFavorite = useCallback(
    (song: SongView, favorite: boolean) => {
      // Optimistic: the sidebar count moves with the click, not after it.
      bumpLibraryCount("favorites", favorite ? 1 : -1);
      void bridge
        .call("set_favorite", { songId: song.id, favorite })
        .then((committed) => {
          // Broadcast the committed view (player bar / other surfaces adopt
          // the new heart), then refresh the member list.
          publishSongUpdate(committed as SongView);
          loadMembers();
        })
        .catch(() => {
          bumpLibraryCount("favorites", favorite ? -1 : 1);
          notify({ message: "收藏失败，请重试", error: true });
        });
    },
    [loadMembers],
  );

  const onEnqueue = useCallback((song: SongView) => {
    void bridge
      .call("queue_command", { command: "enqueue", songId: song.id })
      .then(() => notify(`已将 ${song.title ?? "歌曲"} 加入播放队列`))
      .catch(() => notify({ message: "加入播放队列失败，请重试", error: true }));
  }, []);

  async function removeMember(song: SongView) {
    try {
      await bridge.call("remove_playlist_song", { playlist: playlistId, song: song.id });
      loadMembers();
      onLibraryChanged?.();
      setMenuFor(null);
    } catch {
      notify({ message: "移除歌曲失败，请重试", error: true });
    }
  }

  const unavailableCount = members.filter((s) => s.availability !== "available").length;
  const sortedMembers = useMemo(() => sortPlaylistMembers(members, sort), [members, sort]);
  // 搜索态展示后端筛选结果，空词回退到全量成员；批量/全选按当前显示集作用。
  const displayedSongs = searched ? searched.items : sortedMembers;
  const selectedSongs = useMemo(
    () => displayedSongs.filter((song) => selection.selectedIds.has(song.id)),
    [displayedSongs, selection.selectedIds],
  );
  const allLoadedSelected =
    displayedSongs.length > 0 && displayedSongs.every((song) => selection.selectedIds.has(song.id));

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
        loadMembers();
        notifyBatch(result);
        exitSelectionMode();
      } catch {
        notify({ message: "批量操作失败，请重试", error: true });
      } finally {
        setBatchBusy(false);
      }
    },
    [exitSelectionMode, loadMembers, notifyBatch, selectedSongs],
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

  const applyBatchRemove = useCallback(async () => {
    setBatchMenuFor(null);
    setBatchBusy(true);
    try {
      const result = await runRemoveFromPlaylistBatch(playlistId, batchMenuSongs);
      loadMembers();
      onLibraryChanged?.();
      notifyBatch(result);
      exitSelectionMode();
    } catch {
      notify({ message: "批量操作失败，请重试", error: true });
    } finally {
      setBatchBusy(false);
    }
  }, [batchMenuSongs, exitSelectionMode, loadMembers, notifyBatch, onLibraryChanged, playlistId]);

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
      onRemoveFromPlaylist: () => void applyBatchRemove(),
    }),
    [applyBatchFavorite, applyBatchQueue, applyBatchRemove, batchMenuSongs],
  );

  const confirmBatchDelete = useCallback(async () => {
    if (!batchDeleteFor) return;
    const songs = batchDeleteFor;
    setBatchDeleteFor(null);
    setBatchBusy(true);
    const result = await runDeleteBatch(root, songs);
    loadMembers();
    onLibraryChanged?.();
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
          loadMembers();
          onLibraryChanged?.();
          invalidateLibraryCounts();
          notify({
            message: `撤回删除：成功 ${undo.succeeded}，失败 ${undo.failed}`,
            error: undo.failed > 0,
          });
        });
      },
    });
  }, [batchDeleteFor, exitSelectionMode, loadMembers, onLibraryChanged, root]);

  return (
    <>
      <Topbar title={name}>
        <label className="search" data-testid="playlist-search-field">
          <Icon name="search" />
          <input
            type="search"
            placeholder="搜索歌曲、艺人或专辑"
            value={searchText}
            onChange={(event) => setSearchText(event.target.value)}
          />
        </label>
      </Topbar>

      <main className="content">
        <div className="library-view" data-testid="playlist-view">
          <div className="library-head">
            <div className="list-context">
              <h1 id="view-title" data-testid="view-title">
                {name}
              </h1>
              <span className="library-total">
                {searched ? `${searched.totalCount} 首` : `${members.length} 首`}
              </span>
            </div>
            <div className="library-tools">
              <SongSortControl
                sort={sort}
                onChange={setSort}
                fields={SORT_FIELDS.filter(({ value }) => value !== "album")}
              />
              <LocateButton onClick={onLocate} />
              <SelectionModeButton
                active={selectionMode}
                onToggle={() => {
                  if (selectionMode) exitSelectionMode();
                  else setSelectionMode(true);
                }}
              />
              <button
                type="button"
                className="tool-button"
                disabled={readOnly}
                onClick={() => setRenaming(true)}
              >
                <Icon name="edit" />
                编辑歌单
              </button>
              <button
                type="button"
                className="tool-button"
                disabled={readOnly}
                onClick={() => setConfirmDelete(true)}
              >
                <Icon name="trash" />
                删除歌单
              </button>
            </div>
          </div>

          <p className="playlist-summary">
            共 {searched ? searched.totalCount : members.length} 首
            {!searched && unavailableCount > 0 ? `（${unavailableCount} 首不可用）` : ""}
          </p>
          {loadError ? (
            <button type="button" className="btn" onClick={loadMembers}>
              {loadError}
            </button>
          ) : null}

          <SongList
            songs={displayedSongs}
            search={searchText}
            loading={searching}
            isLast={searched ? searched.isLast : true}
            readOnly={readOnly}
            currentSongId={snapshot.currentSongId}
            playing={snapshot.state === "playing"}
            locateSongId={locateSongId}
            onLocateSettled={onLocateSettled}
            selectionMode={selectionMode}
            selectedIds={selection.selectedIds}
            allLoadedSelected={allLoadedSelected}
            onToggleSelection={(song) => selection.toggle(song.id)}
            onToggleSelectAll={() =>
              selection.toggleAllLoaded(displayedSongs.map((song) => song.id))
            }
            onContextMenu={(song, anchor) => {
              if (selectionMode) openBatchMenu(song, anchor);
              else setMenuFor({ song, anchor });
            }}
            onLoadMore={() => {}}
            onClearSearch={() => setSearchText("")}
            onPlay={onPlay}
            onFavorite={onFavorite}
            onPlayNext={(song) =>
              bridge.fireAndForget("queue_command", { command: "playNext", songId: song.id })
            }
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
            bridge.fireAndForget("queue_command", {
              command: "playNext",
              songId: menuFor.song.id,
            });
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
            // Reuse the same selection dialog the batch path uses: a single
            // song opens the picker just as it does in the library workspace.
            setAddToPlaylistFor([menuFor.song]);
            setMenuFor(null);
          }}
          onRefresh={refreshAfterSongMutation}
          extraActions={
            <button
              type="button"
              className="menu-action danger"
              onClick={() => void removeMember(menuFor.song)}
            >
              <Icon name="trash" />
              从歌单移除
            </button>
          }
        />
      ) : null}

      {selectionMode && batchMenuFor && batchMenuSongs.length > 0 ? (
        <BatchSongMenu
          anchor={batchMenuFor.anchor}
          songs={batchMenuSongs}
          readOnly={readOnly || batchBusy}
          inPlaylist
          handlers={batchHandlers}
          onClose={() => setBatchMenuFor(null)}
        />
      ) : null}

      {addToPlaylistFor ? (
        <AddToPlaylistDialog
          songIds={addToPlaylistFor.map((song) => song.id)}
          songs={addToPlaylistFor}
          root={root}
          readOnly={readOnly || batchBusy}
          onClose={() => setAddToPlaylistFor(null)}
          onDone={() => {
            loadMembers();
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

      {renaming ? (
        <PlaylistNameDialog
          mode="edit"
          playlistId={playlistId}
          initialName={name}
          initialCoverKey={coverKey}
          automaticCoverKey={automaticCoverKey}
          hasCustomCover={hasCustomCover}
          existingNames={existingNames}
          onClose={() => setRenaming(false)}
          onDone={(next) => {
            setName(next);
            onLibraryChanged?.();
          }}
        />
      ) : null}

      {confirmDelete ? (
        <ConfirmationDialog
          title={`删除歌单「${name}」？`}
          description="只会删除歌单本身，资料库中的歌曲文件不会被删除。"
          confirmLabel="确认删除歌单"
          onConfirm={() => void deletePlaylist()}
          onCancel={() => setConfirmDelete(false)}
        />
      ) : null}
    </>
  );
}

/** The native list is newest membership first. Other choices sort only this
 * playlist's loaded members, keeping its membership order as the tie-break. */
function sortPlaylistMembers(members: readonly SongView[], sort: SongSort): readonly SongView[] {
  const indexed = members.map((song, index) => ({ song, index }));
  const direction = sort.direction === "asc" ? 1 : -1;
  const compareText = (left: string | undefined, right: string | undefined) =>
    (left ?? "").localeCompare(right ?? "", "zh-Hans-CN", { sensitivity: "base" });
  indexed.sort((left, right) => {
    if (sort.field === "addedAt") {
      return sort.direction === "desc" ? left.index - right.index : right.index - left.index;
    }
    let value = 0;
    switch (sort.field) {
      case "title":
        value = compareText(left.song.title, right.song.title);
        break;
      case "artist":
        value = compareText(left.song.artist, right.song.artist);
        break;
      case "album":
        value = compareText(
          left.song.album?.trim() || "未知专辑",
          right.song.album?.trim() || "未知专辑",
        );
        break;
      case "playCount":
        value = left.song.playCount - right.song.playCount;
        break;
    }
    return value === 0 ? left.index - right.index : value * direction;
  });
  return indexed.map(({ song }) => song);
}
