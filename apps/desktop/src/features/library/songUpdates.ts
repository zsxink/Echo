/**
 * Authoritative song-update broadcast (task 10.7 follow-up, task 8.2).
 *
 * A mutation command (`set_favorite`) returns the committed, authoritative
 * `SongView`. Every surface that renders a song's mutable fields — the library
 * table rows, the player bar's 收藏 heart, the immersive player's detail — must
 * reflect that commit immediately instead of waiting for the next full
 * re-query, and must agree on it (a stale heart must never survive on one
 * surface because the toggle happened on another).
 *
 * This module is the rendezvous point: mutators publish the committed fields,
 * mounted surfaces subscribe and merge them into their local state.
 *
 * The payload is a **patch**, not a replacement: a surface that only knows the
 * mutable fields (the player bar reads `song_detail`, which carries no
 * `quality` tier) publishes just those. Replacing the row outright would blank
 * every field the publisher does not know — a 已保存待更新 like made from the
 * bar would silently erase the row's title.
 *
 * It carries no cache and no state of its own — a subscriber that is not
 * mounted simply misses the event, and its next authoritative read (re-query,
 * `song_detail`) is already correct. Deliberately **not** a replacement for the
 * Rust-side `library://songs-invalidated` event: this covers only
 * frontend-initiated mutations, synchronously, without an IPC round-trip.
 */

import type { SongView } from "../../ipc/ipc-types.generated";
import { ExternalStore } from "../../app/externalStore";

/** Committed fields of one song. Only `id` is required; everything else is
 *  merged onto whatever the surface already holds. */
export type SongUpdate = Pick<SongView, "id"> & Partial<Omit<SongView, "id">>;

const songUpdateStore = new ExternalStore<SongUpdate | null>(null);

/** Publish the authoritative fields returned by a committed mutation. */
export function publishSongUpdate(update: SongUpdate): void {
  songUpdateStore.setSnapshot(update);
}

/** Subscribe to committed song updates; returns the unsubscribe handle. */
export function subscribeSongUpdates(listener: (update: SongUpdate) => void): () => void {
  return songUpdateStore.subscribe(() => {
    const update = songUpdateStore.getSnapshot();
    if (update) listener(update);
  });
}

/** Test teardown seam for the shared event snapshot. */
export function resetSongUpdates(): void {
  songUpdateStore.setSnapshot(null);
}
