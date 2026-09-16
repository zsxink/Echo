/**
 * Playlists query hook (task 10.9).
 *
 * Loads the playlist list + member counts through the bridge and exposes a
 * `reload` so the sidebar counts, the playlist view and the add-to-playlist
 * picker stay in sync after a create / rename / delete / membership change. A
 * failed load yields an empty list (the sidebar just renders nothing) rather
 * than crashing the shell.
 */

import { useCallback, useEffect, useRef, useState } from "react";

import { bridge } from "../../bridge";
import type { PlaylistView } from "../../ipc/ipc-types.generated";

export interface LibraryPlaylists {
  readonly playlists: readonly PlaylistView[];
  /** Re-fetch the list; identity is stable so it is safe in effect deps. */
  readonly reload: () => void;
}

/**
 * @param enabled false while there is no library yet — asking for playlists
 *   before a root is activated would only produce an error. The query runs as
 *   soon as the library becomes configured, so a freshly activated library's
 *   playlists arrive without any extra wiring in the shell.
 */
export function useLibraryPlaylists(enabled = true, activeRoot?: string): LibraryPlaylists {
  const [playlists, setPlaylists] = useState<readonly PlaylistView[]>([]);
  const [revision, setRevision] = useState(0);
  const rootRef = useRef<string | undefined>(activeRoot);

  useEffect(() => {
    if (!enabled) {
      setPlaylists([]);
      return;
    }
    if (rootRef.current !== activeRoot) {
      rootRef.current = activeRoot;
      setPlaylists([]);
    }
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
  // Root identity is intentionally a query input, even though the command
  // itself resolves the active root on desktop.  It resets the stale sidebar
  // immediately and makes a delayed previous-root result harmless.
  }, [revision, enabled, activeRoot]);

  const reload = useCallback(() => setRevision((value) => value + 1), []);

  return { playlists, reload };
}
