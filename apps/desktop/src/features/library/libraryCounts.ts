/**
 * Authoritative navigation counts and their optimistic favourite overlay.
 *
 * The shared external-store primitive keeps the snapshot and subscription
 * contract consistent with the rest of the desktop app. Counts are fetched
 * before a view opens because a paged list cannot know its full total.
 */

import { useCallback, useEffect, useState } from "react";

import { ExternalStore, useExternalStore } from "../../app/externalStore";
import { bridge, reportBridgeFailure, subscribe } from "../../bridge";
import { subscribeSongUpdates } from "./songUpdates";

export type LibraryCountView = "all" | "favorites" | "recent";

export interface LibraryCounts {
  readonly all: number | null;
  readonly favorites: number | null;
  readonly recent: number | null;
}

const EMPTY_COUNTS: LibraryCounts = { all: null, favorites: null, recent: null };
const libraryCountsStore = new ExternalStore<LibraryCounts>(EMPTY_COUNTS);

/** Replace cached totals with the backend's authoritative snapshot. */
export function setLibraryCounts(next: LibraryCounts): void {
  const current = libraryCountsStore.getSnapshot();
  if (
    current.all === next.all &&
    current.favorites === next.favorites &&
    current.recent === next.recent
  ) {
    return;
  }
  libraryCountsStore.setSnapshot(next);
}

/** Optimistically replace one cached total when the UI has committed a fact. */
export function publishLibraryCount(view: LibraryCountView, total: number): void {
  const current = libraryCountsStore.getSnapshot();
  if (current[view] === total) return;
  setLibraryCounts({ ...current, [view]: total });
}

/** Optimistically nudge a known count; unknown counts stay unknown. */
export function bumpLibraryCount(view: LibraryCountView, delta: number): void {
  const current = libraryCountsStore.getSnapshot()[view];
  if (current !== null) publishLibraryCount(view, Math.max(0, current + delta));
}

/** Test/teardown seam; product invalidation deliberately keeps visible totals. */
export function resetLibraryCounts(): void {
  setLibraryCounts(EMPTY_COUNTS);
}

/** Read the current navigation totals through the common external-store API. */
export function useLibraryCounts(): LibraryCounts {
  return useExternalStore(libraryCountsStore);
}

/** Refresh counts on root status, committed song updates, or explicit invalidation. */
export function useLibraryCountSync(enabled = true): void {
  const [revision, setRevision] = useState(0);
  const invalidate = useCallback(() => setRevision((value) => value + 1), []);

  useEffect(() => {
    if (!enabled) return;
    let cancelled = false;
    void bridge
      .call("library_counts")
      .then((dto) => {
        if (!cancelled) {
          setLibraryCounts({ all: dto.all, favorites: dto.favorites, recent: dto.recent });
        }
      })
      .catch((error: unknown) => reportBridgeFailure("library_counts", error));
    return () => {
      cancelled = true;
    };
  }, [revision, enabled]);

  useEffect(() => {
    if (!enabled) return;
    let unlisten: (() => void) | undefined;
    void subscribe("library://status", () => invalidate())
      .then((fn) => {
        unlisten = fn;
      })
      .catch((error: unknown) => reportBridgeFailure("library_status", error));
    const unsubscribe = subscribeSongUpdates(invalidate);
    setInvalidateHandler(invalidate);
    return () => {
      unlisten?.();
      unsubscribe();
      if (invalidateHandler === invalidate) invalidateHandler = null;
    };
  }, [enabled, invalidate]);
}

type InvalidateHandler = () => void;
let invalidateHandler: InvalidateHandler | null = null;

function setInvalidateHandler(handler: InvalidateHandler): void {
  invalidateHandler = handler;
}

/** Request an authoritative re-count without blanking currently visible totals. */
export function invalidateLibraryCounts(): void {
  invalidateHandler?.();
}
