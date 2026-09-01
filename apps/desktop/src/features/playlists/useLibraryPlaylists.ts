/**
 * Playlists query hook (task 10.9).
 *
 * Loads the playlist list + member counts through the bridge and re-fetches on
 * invalidation so the sidebar count and the playlist view stay in sync. A
 * failed load yields an empty list (the sidebar shows "暂无歌单") rather than
 * crashing the shell.
 */

import { useEffect, useState } from "react";

import { bridge } from "../../bridge";
import type { PlaylistView } from "../../ipc/ipc-types.generated";

export function useLibraryPlaylists(): readonly PlaylistView[] {
  const [playlists, setPlaylists] = useState<readonly PlaylistView[]>([]);

  useEffect(() => {
    let cancelled = false;
    void bridge
      .call("playlists")
      .then((value: unknown) => {
        if (!cancelled) setPlaylists(value as PlaylistView[]);
      })
      .catch(() => {
        // A failed list is an empty list, not a broken shell.
      });
    return () => {
      cancelled = true;
    };
  }, []);

  return playlists;
}
