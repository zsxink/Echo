/**
 * Cover placeholders and library counts.
 *
 * `coverClass` reproduces the prototype's five-step cover palette (A–E, brand
 * §6): a library without embedded artwork still reads as data instead of empty
 * boxes. The tint is derived from the id, so a song keeps the same cover colour
 * across renders, views and restarts.
 *
 * The count store answers one question per library view — 这个视图有多少首歌 —
 * and it must answer it **before the user opens the view**, because deciding
 * whether to open it is the only reason a navigation count exists. So the
 * authoritative source is the backend's `library_counts` command, not the rows
 * a view happens to have paged in: a paged query only knows its total once it
 * reaches the last page, and a 50,000-song view may never be paged that far.
 *
 * `publishLibraryCount` survives as an *optimistic overlay* only. A heart
 * toggle is a local fact the UI already knows — waiting a round-trip to move
 * the number feels broken — so it nudges the cached value immediately and the
 * next authoritative fetch corrects it if the guess was wrong.
 */

import { useCallback, useEffect, useState, useSyncExternalStore } from "react";

import { bridge, subscribe } from "../../bridge";
import { subscribeSongUpdates } from "./songUpdates";
import type { LibraryCountsDto } from "../../ipc/ipc-types.generated";

const PALETTE = ["", "cover-b", "cover-c", "cover-d", "cover-e"] as const;

/** A stable A–E cover tint class for a song or playlist id. */
export function coverClass(seed: string): string {
  let hash = 0;
  for (let i = 0; i < seed.length; i += 1) {
    hash = (hash * 31 + seed.charCodeAt(i)) % 1_000_003;
  }
  return PALETTE[hash % PALETTE.length];
}

export type LibraryCountView = "all" | "favorites" | "recent";

export interface LibraryCounts {
  readonly all: number | null;
  readonly favorites: number | null;
  readonly recent: number | null;
}

let counts: LibraryCounts = { all: null, favorites: null, recent: null };
const listeners = new Set<() => void>();

function emit(): void {
  for (const listener of listeners) listener();
}

/**
 * Optimistically nudge one view's count by `delta`.
 *
 * Only used for facts the UI just committed locally (a favorite toggle). It
 * never invents a number out of nothing: with no cached value there is nothing
 * to nudge, and the next fetch supplies the authoritative one.
 */
export function publishLibraryCount(view: LibraryCountView, total: number): void {
  const current = counts[view];
  if (current === total) return;
  counts = { ...counts, [view]: total };
  emit();
}

/**
 * Adjust one view's count relative to its cached value — the shape a favorite
 * toggle actually knows (`+1` / `-1`), which it cannot express as a total.
 */
export function bumpLibraryCount(view: LibraryCountView, delta: number): void {
  const current = counts[view];
  if (current === null) return;
  publishLibraryCount(view, Math.max(0, current + delta));
}

/**
 * Drop the cached totals entirely (test seams and library teardown only).
 *
 * Deliberately not named `clear…` in the product path — see
 * [`invalidateLibraryCounts`] for why a live reset is the wrong shape there.
 */
export function resetLibraryCounts(): void {
  if (counts.all === null && counts.favorites === null && counts.recent === null) return;
  counts = { all: null, favorites: null, recent: null };
  emit();
}

/** Replace the cached totals with an authoritative server snapshot. */
export function setLibraryCounts(next: LibraryCounts): void {
  if (
    counts.all === next.all &&
    counts.favorites === next.favorites &&
    counts.recent === next.recent
  ) {
    return;
  }
  counts = next;
  emit();
}

export function useLibraryCounts(): LibraryCounts {
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener);
      return () => {
        listeners.delete(listener);
      };
    },
    () => counts,
  );
}

/**
 * Fetch the authoritative per-view totals and keep them fresh.
 *
 * Three things can make a count stale, and all three funnel into one
 * `revision` counter rather than into three ad-hoc call sites:
 *
 * - `library://status` — the root switched, a scan finished, availability moved.
 * - `songUpdates` — a committed `set_favorite` from any surface.
 * - `invalidateLibraryCounts()` — an import, delete or playlist change.
 *
 * A failed fetch keeps the last known numbers: a stale count is still
 * navigation, whereas a blank one tells the user nothing.
 */
export function useLibraryCountSync(enabled = true): void {
  const [revision, setRevision] = useState(0);

  const invalidate = useCallback(() => setRevision((value) => value + 1), []);

  useEffect(() => {
    if (!enabled) return;
    let cancelled = false;
    void bridge
      .call("library_counts")
      .then((value: unknown) => {
        if (cancelled) return;
        const dto = value as LibraryCountsDto;
        setLibraryCounts({
          all: dto.all,
          favorites: dto.favorites,
          recent: dto.recent,
        });
      })
      .catch(() => {
        // Keep the last known counts; a retry follows the next invalidation.
      });
    return () => {
      cancelled = true;
    };
  }, [revision, enabled]);

  useEffect(() => {
    if (!enabled) return;
    let unlisten: (() => void) | undefined;
    void subscribe("library://status", () => invalidate()).then((fn) => {
      unlisten = fn;
    });
    const unsubscribe = subscribeSongUpdates(() => invalidate());
    // Non-React callers (mutation callbacks deep in a view) reach the same
    // invalidation through `invalidateLibraryCounts()`.
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

/**
 * Mark the cached counts as no longer trustworthy.
 *
 * Named "invalidate", not "clear": the previous name implied blanking the
 * numbers, which would make the sidebar flash empty on every mutation.
 * Re-fetching keeps showing the last known value until the new one lands.
 */
export function invalidateLibraryCounts(): void {
  invalidateHandler?.();
}
