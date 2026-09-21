import { ExternalStore } from "../../app/externalStore";

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
