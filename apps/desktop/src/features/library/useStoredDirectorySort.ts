import { useCallback, useEffect, useState } from "react";

export type DirectorySortField = "name" | "songCount";
export type DirectorySortScope = "artist" | "album";

export interface DirectorySort {
  readonly field: DirectorySortField;
  readonly direction: "asc" | "desc";
}

const DEFAULT_SORT: DirectorySort = { field: "name", direction: "asc" };

function load(scope: DirectorySortScope): DirectorySort {
  try {
    const value: unknown = JSON.parse(
      localStorage.getItem(`echo-directory-sort:${scope}`) ?? "null",
    );
    if (
      value &&
      typeof value === "object" &&
      ["name", "songCount"].includes((value as DirectorySort).field) &&
      ["asc", "desc"].includes((value as DirectorySort).direction)
    ) {
      return value as DirectorySort;
    }
  } catch {
    // Stored view preferences are optional; use the predictable default.
  }
  return DEFAULT_SORT;
}

/** Keep artist and album directory preferences separate across view switches. */
export function useStoredDirectorySort(
  scope: DirectorySortScope,
): readonly [DirectorySort, (next: DirectorySort) => void] {
  const [entry, setEntry] = useState(() => ({ scope, sort: load(scope) }));
  const sort = entry.scope === scope ? entry.sort : load(scope);
  useEffect(() => {
    if (entry.scope !== scope) setEntry({ scope, sort });
  }, [entry.scope, scope, sort]);
  const update = useCallback(
    (next: DirectorySort) => {
      setEntry({ scope, sort: next });
      try {
        localStorage.setItem(`echo-directory-sort:${scope}`, JSON.stringify(next));
      } catch {
        // Storage may be unavailable in private/restricted environments.
      }
    },
    [scope],
  );
  return [sort, update];
}
