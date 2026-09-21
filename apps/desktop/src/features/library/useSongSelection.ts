import { useCallback, useEffect, useMemo, useState } from "react";

export interface SongSelection {
  readonly selectedIds: ReadonlySet<string>;
  readonly selectedCount: number;
  readonly isSelected: (songId: string) => boolean;
  readonly toggle: (songId: string) => void;
  readonly replace: (songId: string) => void;
  /** Add/remove only the supplied, currently loaded ids. */
  readonly toggleAllLoaded: (songIds: readonly string[]) => void;
  readonly clear: () => void;
}

/**
 * View-scoped selection keyed by stable Song UUIDs.
 *
 * The key is owned by the workspace so changing a search, sort, view, playlist
 * or root creates a new selection scope. Keeping ids outside row components is
 * what makes the selection survive virtual-row unmounts and pagination.
 */
export function useSongSelection(selectionKey: string): SongSelection {
  const [selectedIds, setSelectedIds] = useState<ReadonlySet<string>>(() => new Set());

  useEffect(() => {
    setSelectedIds(new Set());
  }, [selectionKey]);

  const isSelected = useCallback((songId: string) => selectedIds.has(songId), [selectedIds]);

  const toggle = useCallback((songId: string) => {
    setSelectedIds((previous) => {
      const next = new Set(previous);
      if (next.has(songId)) next.delete(songId);
      else next.add(songId);
      return next;
    });
  }, []);

  const replace = useCallback((songId: string) => {
    setSelectedIds(new Set([songId]));
  }, []);

  const toggleAllLoaded = useCallback((songIds: readonly string[]) => {
    setSelectedIds((previous) => {
      const next = new Set(previous);
      const allSelected = songIds.length > 0 && songIds.every((songId) => next.has(songId));
      for (const songId of songIds) {
        if (allSelected) next.delete(songId);
        else next.add(songId);
      }
      return next;
    });
  }, []);

  const clear = useCallback(() => setSelectedIds(new Set()), []);

  return useMemo(
    () => ({
      selectedIds,
      selectedCount: selectedIds.size,
      isSelected,
      toggle,
      replace,
      toggleAllLoaded,
      clear,
    }),
    [clear, isSelected, replace, selectedIds, toggle, toggleAllLoaded],
  );
}
