import { useCallback, useEffect, useState } from "react";

import type { SongSort, SongSortField } from "./types";

const DEFAULT_SORT: SongSort = { field: "addedAt", direction: "desc" };
const SORT_FIELDS: readonly SongSortField[] = ["addedAt", "title", "artist", "playCount"];

function load(scope: string): SongSort {
  try {
    const value: unknown = JSON.parse(localStorage.getItem(`echo-song-sort:${scope}`) ?? "null");
    if (
      value &&
      typeof value === "object" &&
      SORT_FIELDS.includes((value as SongSort).field) &&
      ["asc", "desc"].includes((value as SongSort).direction)
    ) {
      return value as SongSort;
    }
  } catch {
    // Storage is a convenience only; the default remains fully usable.
  }
  return DEFAULT_SORT;
}

/** A song view owns its sorting preference; it never borrows another view's. */
export function useStoredSongSort(scope: string): readonly [SongSort, (next: SongSort) => void] {
  const [entry, setEntry] = useState(() => ({ scope, sort: load(scope) }));
  // A workspace can switch views without unmounting. Derive the next scope's
  // value during that render so it never briefly borrows the prior view's sort.
  const sort = entry.scope === scope ? entry.sort : load(scope);
  useEffect(() => {
    if (entry.scope !== scope) setEntry({ scope, sort });
  }, [entry.scope, scope, sort]);
  const update = useCallback(
    (next: SongSort) => {
      setEntry({ scope, sort: next });
      try {
        localStorage.setItem(`echo-song-sort:${scope}`, JSON.stringify(next));
      } catch {
        // Storage may be unavailable in private/restricted environments.
      }
    },
    [scope],
  );
  return [sort, update];
}
