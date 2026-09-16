/**
 * Core query-cache-backed song list hook (task 10.2 / 10.5).
 *
 * Reads the pageable catalog (all / search / favorites / recent) through the
 * bridge with keyset cursors. A short query (150ms debounce) is cancelled by a
 * request id so an outdated response never overwrites the current one; clearing
 * restores the full view immediately. Results are keyed by SongId so a virtual
 * list keeps stable identity (task 10.6).
 */

import { useCallback, useEffect, useRef, useState } from "react";

import { bridge } from "../../bridge";
import type { PagedSongs, SongView } from "../../ipc/ipc-types.generated";
import type { LibraryViewKind, SongSort } from "./types";

export interface SongPage {
  readonly songs: readonly SongView[];
  readonly nextCursor?: string;
  readonly isLast: boolean;
}

export interface SongQuery {
  readonly view: LibraryViewKind;
  readonly search: string;
  readonly sort: SongSort;
  readonly inFavorites: boolean;
  readonly playlistId?: string | null;
  readonly root?: string;
  readonly readOnly: boolean;
}

const SEARCH_DEBOUNCE_MS = 150;

function pageKey(query: SongQuery): string {
  return [
    query.view,
    query.search.trim().toLowerCase(),
    query.sort.field,
    query.sort.direction,
    query.playlistId ?? "",
    // The desktop resolves queries against its active root.  Cache and request
    // identity must nevertheless include that root so a late old-root reply
    // cannot win after the shell has switched roots.
    query.root ?? "",
  ].join("|");
}

async function fetchPage(query: SongQuery, cursor?: string | null): Promise<PagedSongs> {
  const limit = 200;
  if (query.view === "recent") {
    const items = (await bridge.call("recent", { query: query.search.trim() })) as SongView[];
    return { items, isLast: true };
  }
  const sort = `${query.sort.field}:${query.sort.direction}`;
  if (query.view === "favorites" || query.inFavorites) {
    if (query.search.trim().length > 0) {
      return bridge.call("search", {
        query: query.search,
        inFavorites: true,
        sort,
        cursor: cursor ?? null,
        limit,
      });
    }
    return bridge.call("favorites", { sort, cursor: cursor ?? null, limit });
  }
  if (query.search.trim().length > 0) {
    return bridge.call("search", {
      query: query.search,
      // Tauri maps Rust's `in_favorites` argument to the `inFavorites` JSON
      // key. Sending snake_case makes command deserialization fail before the
      // search service runs.
      inFavorites: false,
      sort,
      cursor: cursor ?? null,
      limit,
    });
  }
  return bridge.call("all_songs", { sort, cursor: cursor ?? null, limit });
}

export function useSongs(query: SongQuery): {
  readonly page: SongPage;
  readonly loading: boolean;
  /** A recoverable load error (task 10.7): existing content is kept and a
   *  retry is offered rather than wiping or faking a result. */
  readonly error: string | null;
  readonly loadMore: () => void;
  readonly reset: () => void;
  readonly retry: () => void;
  /** Apply one authoritative `SongView` (a committed mutation) to the loaded
   *  page in place, so a toggle is visible without a full re-query. */
  readonly patchSong: (song: SongView) => void;
} {
  const [page, setPage] = useState<SongPage>({ songs: [], isLast: false });
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // Mutations such as an import keep the query itself unchanged. Keep a
  // separate revision so `reset()` can invalidate the loaded page *and* issue
  // a new first-page request for the currently selected view.
  const [refreshEpoch, setRefreshEpoch] = useState(0);
  const requests = useRef(new Map<string, number>());
  const key = pageKey(query);
  const cursorRef = useRef<string | undefined>(undefined);
  // The latest effect run's loader, exposed as a stable `retry()` to the UI.
  const retryRef = useRef<() => void>(() => {});

  // Load the first page whenever the query key changes (debounced search).
  useEffect(() => {
    const trimmed = query.search.trim();
    let timer: ReturnType<typeof setTimeout> | undefined;
    const run = async () => {
      const reqId = Date.now() + Math.random();
      requests.current.set(key, reqId);
      setLoading(true);
      setError(null);
      try {
        const result = await fetchPage(query);
        if (requests.current.get(key) === reqId) {
          cursorRef.current = result.nextCursor;
          setPage({ songs: result.items, nextCursor: result.nextCursor, isLast: result.isLast });
        }
      } catch (err) {
        // A failed/stale fetch must not wipe existing content (task 10.7);
        // surface a retryable message instead.
        if (requests.current.get(key) === reqId) {
          setError(messageOf(err));
        }
      } finally {
        if (requests.current.get(key) === reqId) setLoading(false);
      }
    };
    if (trimmed.length > 0) {
      timer = setTimeout(() => void run(), SEARCH_DEBOUNCE_MS);
    } else {
      void run();
    }
    // Expose the current loader as the stable `retry()` used by the UI.
    retryRef.current = run;
    return () => {
      if (timer) clearTimeout(timer);
      requests.current.delete(key);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, refreshEpoch]);

  const loadMore = useCallback(async () => {
    if (!cursorRef.current || loading) return;
    const cursor = cursorRef.current;
    const reqId = Date.now() + Math.random();
    requests.current.set(key, reqId);
    setLoading(true);
    try {
      const result = await fetchPage(query, cursor);
      if (requests.current.get(key) === reqId) {
        cursorRef.current = result.nextCursor;
        setPage((prev) => ({
          songs: [...prev.songs, ...result.items],
          nextCursor: result.nextCursor,
          isLast: result.isLast,
        }));
      }
    } finally {
      if (requests.current.get(key) === reqId) setLoading(false);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key, loading]);

  const reset = useCallback(() => {
    cursorRef.current = undefined;
    setPage({ songs: [], isLast: false });
    setError(null);
    setRefreshEpoch((epoch) => epoch + 1);
  }, []);

  const retry = useCallback(() => retryRef.current(), []);

  // In-view patch for a committed mutation (e.g. `set_favorite`'s returned
  // authoritative SongView). The favorites view only ever shows favorites, so
  // there a committed un-favorite removes the row; a song that just became a
  // favorite is appended (the next full query restores the authoritative sort).
  const patchSong = useCallback(
    (song: SongView) => {
      setPage((prev) => {
        const index = prev.songs.findIndex((s) => s.id === song.id);
        if (query.inFavorites && !song.favorite) {
          if (index < 0) return prev;
          return { ...prev, songs: prev.songs.filter((s) => s.id !== song.id) };
        }
        if (index >= 0) {
          const songs = prev.songs.slice();
          songs[index] = song;
          return { ...prev, songs };
        }
        if (query.inFavorites) {
          return { ...prev, songs: [...prev.songs, song] };
        }
        return prev;
      });
    },
    [query.inFavorites],
  );

  return { page, loading, error, loadMore, reset, retry, patchSong };
}

/** Turn a bridge failure into a short, user-safe message (no paths, ids). */
function messageOf(err: unknown): string {
  if (err instanceof Error) {
    if ("code" in err) {
      const code = (err as unknown as { code?: string }).code;
      if (code === "unavailable") return "资料库暂不可用，请重试";
      if (code === "conflict") return "查询已过期，请重试";
    }
    return "加载歌曲失败，请重试";
  }
  return "加载歌曲失败，请重试";
}
