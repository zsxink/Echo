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
  ].join("|");
}

async function fetchPage(query: SongQuery, cursor?: string | null): Promise<PagedSongs> {
  const limit = 200;
  if (query.view === "recent") {
    const items = (await bridge.call("recent")) as SongView[];
    return { items, isLast: true };
  }
  const sort = `${query.sort.field}:${query.sort.direction}`;
  if (query.view === "favorites" || query.inFavorites) {
    return bridge.call("favorites", { sort, cursor: cursor ?? null, limit });
  }
  if (query.search.trim().length > 0) {
    return bridge.call("search", {
      query: query.search,
      in_favorites: false,
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
  readonly loadMore: () => void;
  readonly reset: () => void;
} {
  const [page, setPage] = useState<SongPage>({ songs: [], isLast: false });
  const [loading, setLoading] = useState(false);
  const requests = useRef(new Map<string, number>());
  const key = pageKey(query);
  const cursorRef = useRef<string | undefined>(undefined);

  // Load the first page whenever the query key changes (debounced search).
  useEffect(() => {
    const trimmed = query.search.trim();
    let timer: ReturnType<typeof setTimeout> | undefined;
    const run = async () => {
      const reqId = Date.now() + Math.random();
      requests.current.set(key, reqId);
      setLoading(true);
      try {
        const result = await fetchPage(query);
        if (requests.current.get(key) === reqId) {
          cursorRef.current = result.nextCursor;
          setPage({ songs: result.items, nextCursor: result.nextCursor, isLast: result.isLast });
        }
      } catch {
        // A failed/stale fetch must not wipe existing content (task 10.7).
      } finally {
        if (requests.current.get(key) === reqId) setLoading(false);
      }
    };
    if (trimmed.length > 0) {
      timer = setTimeout(() => void run(), SEARCH_DEBOUNCE_MS);
    } else {
      void run();
    }
    return () => {
      if (timer) clearTimeout(timer);
      requests.current.delete(key);
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);

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
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [key]);

  return { page, loading, loadMore, reset };
}
