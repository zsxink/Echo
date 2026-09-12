/**
 * Library workspace: toolbar + song list (task 10.4 / 10.5 / 10.6).
 *
 * Hosts the current view (全部歌曲 / 最近添加 / 喜欢的音乐), the search overlay
 * (150 ms debounced, cancelled by request id), the four sort fields with
 * direction, and the virtualized song list bound to stable SongIds. It wires
 * row actions to the bridge (favorite mutation; play → player command; menu →
 * detail / reveal / delete / add-to-playlist). Excluded actions (sync, bulk
 * select, manual playlist sort, song editing) are intentionally absent.
 */

import { useCallback, useMemo, useState } from "react";

import { bridge } from "../../bridge";
import { usePlayerSnapshot } from "../../player/playerStore";
import type { ImportBatchDto, SongView } from "../../ipc/ipc-types.generated";
import { LibraryViewKind, SongSortField } from "./types";
import { SongList } from "./SongList";
import { useSongs } from "./useSongs";
import { SongMenu } from "./SongMenu";
import { AddToPlaylistDialog } from "../playlists/AddToPlaylistDialog";
import { ImportBatchDialog } from "../import/ImportBatchDialog";
import "./library.css";

interface LibraryWorkspaceProps {
  readonly view: LibraryViewKind;
  readonly root: string;
  readonly readOnly: boolean;
}

export function LibraryWorkspace({ view, root, readOnly }: LibraryWorkspaceProps) {
  const [search, setSearch] = useState("");
  const [sortField, setSortField] = useState<SongSortField>("addedAt");
  const [sortDir, setSortDir] = useState<"asc" | "desc">("desc");
  const [menuFor, setMenuFor] = useState<SongView | null>(null);
  const [importOpen, setImportOpen] = useState(false);
  const [lastImport, setLastImport] = useState<ImportBatchDto | null>(null);
  const [addToPlaylistFor, setAddToPlaylistFor] = useState<string | null>(null);
  const snapshot = usePlayerSnapshot();

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
  const { page, loading, error, loadMore, reset, retry } = useSongs(query);

  const toggleSortDir = useCallback(() => {
    setSortDir((d) => (d === "asc" ? "desc" : "asc"));
  }, []);

  const onFavorite = useCallback(
    (song: SongView, favorite: boolean) => {
      void bridge.call("set_favorite", { songId: song.id, favorite }).catch(() => {
        // A failed mutation leaves the state authoritative — re-render from
        // the server (task 10.7: 过期/失败不覆盖新状态).
        reset();
      });
    },
    [reset],
  );

  const onPlay = useCallback(
    (song: SongView) => {
      // Play a context built from the current loaded view (task 11.1): the
      // desktop player resolves each SongId → file and drives the queue; the
      // UI only sends the deterministic list + the selected start index.
      const selectedIndex = page.songs.findIndex((s) => s.id === song.id);
      void bridge.call("play_context", {
        songs: page.songs.map((s) => s.id),
        selectedIndex: selectedIndex < 0 ? 0 : selectedIndex,
      });
    },
    [page.songs],
  );

  const onPlayNext = useCallback((song: SongView) => {
    // Insert the song right after the current one ("下一首播放", task 10.6).
    void bridge.call("queue_command", { command: "playNext", songId: song.id });
  }, []);

  const onEnqueue = useCallback((song: SongView) => {
    // Append the song to the end of the queue ("加入播放队列", task 10.6).
    void bridge.call("queue_command", { command: "enqueue", songId: song.id });
  }, []);

  const onClearSearch = useCallback(() => setSearch(""), []);

  const viewTitle = useMemo(() => {
    switch (view) {
      case "recent":
        return "最近添加";
      case "favorites":
        return "喜欢的音乐";
      case "playlist":
        return "歌单";
      default:
        return "全部歌曲";
    }
  }, [view]);

  // Sort options shown only in the "全部歌曲" view; other views keep their own
  // deterministic order and must not borrow the all-songs sort control.
  const showSort = view === "all";

  return (
    <div className="workspace" data-testid="library-workspace">
      <div className="workspace-toolbar">
        <h2 className="workspace-title">{viewTitle}</h2>
        <label className="search-field" htmlFor="song-search">
          <span className="sr-only">搜索歌曲</span>
          <input
            id="song-search"
            className="search-input"
            placeholder="搜索标题 / 艺人 / 专辑"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            data-testid="search-input"
          />
        </label>
        {showSort ? (
          <div className="sort-control">
            <label htmlFor="sort-field">排序</label>
            <select
              id="sort-field"
              className="sort-select"
              value={sortField}
              onChange={(e) => setSortField(e.target.value as SongSortField)}
              data-testid="sort-field"
            >
              <option value="addedAt">最近添加</option>
              <option value="title">歌曲名称</option>
              <option value="artist">艺人</option>
              <option value="playCount">播放次数</option>
            </select>
            <button
              type="button"
              className="btn-link"
              onClick={toggleSortDir}
              aria-label={`切换排序方向，当前${sortDir === "asc" ? "升序" : "降序"}`}
            >
              {sortDir === "asc" ? "↑" : "↓"}
            </button>
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
        error={error}
        onRetry={retry}
        onImport={() => setImportOpen(true)}
        onLoadMore={loadMore}
        onClearSearch={onClearSearch}
        onPlay={onPlay}
        onFavorite={onFavorite}
        onOpenMenu={setMenuFor}
      />

      {menuFor ? (
        <SongMenu
          song={menuFor}
          root={root}
          readOnly={readOnly}
          onClose={() => setMenuFor(null)}
          onPlay={() => {
            onPlay(menuFor);
            setMenuFor(null);
          }}
          onPlayNext={() => {
            onPlayNext(menuFor);
            setMenuFor(null);
          }}
          onEnqueue={() => {
            onEnqueue(menuFor);
            setMenuFor(null);
          }}
          onFavorite={(fav) => {
            onFavorite(menuFor, fav);
            setMenuFor(null);
          }}
          onAddToPlaylist={() => {
            setAddToPlaylistFor(menuFor.id);
            setMenuFor(null);
          }}
          onRefresh={reset}
        />
      ) : null}

      {addToPlaylistFor ? (
        <AddToPlaylistDialog
          songId={addToPlaylistFor}
          onClose={() => setAddToPlaylistFor(null)}
          onDone={reset}
        />
      ) : null}

      {importOpen ? (
        <ImportBatchDialog
          onClose={() => setImportOpen(false)}
          onDone={(batch) => {
            setLastImport(batch);
            reset();
          }}
        />
      ) : null}
      {lastImport ? (
        <button
          type="button"
          className="btn"
          onClick={() => setImportOpen(true)}
          style={{ position: "fixed", bottom: 90, right: 20 }}
        >
          导入结果
        </button>
      ) : null}
    </div>
  );
}
