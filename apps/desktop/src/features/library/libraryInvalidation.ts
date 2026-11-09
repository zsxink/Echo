import { ExternalStore } from "../../app/externalStore";
import { subscribe } from "../../bridge";
import { publishSongUpdate, type SongUpdate } from "./songUpdates";

/**
 * A feature-local invalidation signal for catalog mutations that complete
 * outside the component that owns a song query (for example, the player bar).
 * Consumers always re-read their own authoritative view; the counter carries
 * no optimistic song data and therefore cannot cross-contaminate filters.
 */
const libraryInvalidationStore = new ExternalStore(0);

/** Notify every mounted library query and count consumer to re-read. */
export function invalidateLibrary(): void {
  libraryInvalidationStore.update((revision) => revision + 1);
}

/** Subscribe to catalog invalidations; intended for non-rendering data hooks. */
export function subscribeLibraryInvalidations(listener: () => void): () => void {
  return libraryInvalidationStore.subscribe(listener);
}

/** Test/teardown seam for the feature-owned external state. */
export function resetLibraryInvalidations(): void {
  libraryInvalidationStore.setSnapshot(0);
}

/**
 * Bridge the shell's cross-window invalidation into this window's store.
 *
 * Each window has its own JavaScript context, so a favorite toggled from the
 * macOS notch player is invisible to the main window's React state. The shell
 * emits `library://invalidated` after a successful catalog mutation and every
 * window re-reads its own authoritative data.
 *
 * Returns a disposer; call once per window at boot.
 */
export function startLibraryEvents(): () => void {
  let dispose: (() => void) | undefined;
  let cancelled = false;
  void subscribe<{ reason?: string; song?: SongUpdate }>("library://invalidated", (payload) => {
    // Adopt the committed row first: the other window's heart updates in the
    // same tick as the click instead of one round trip later.
    if (payload?.song) publishSongUpdate(payload.song);
    invalidateLibrary();
  })
    .then((unlisten) => {
      if (cancelled) {
        unlisten();
        return;
      }
      dispose = unlisten;
    })
    .catch(() => {
      // A subscription failure only costs live cross-window refresh; every
      // surface still re-reads on mount, so this is not a fatal path.
    });
  return () => {
    cancelled = true;
    dispose?.();
  };
}
