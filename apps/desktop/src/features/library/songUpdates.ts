/**
 * Authoritative song-update broadcast (task 10.7 follow-up).
 *
 * A mutation command (`set_favorite`) returns the committed, authoritative
 * `SongView`. Every surface that renders a song's mutable fields — the library
 * table rows, the player bar's 收藏 heart, the immersive player's detail — must
 * reflect that commit immediately instead of waiting for the next full
 * re-query, and must agree on it (a stale heart must never survive on one
 * surface because the toggle happened on another).
 *
 * This module is the rendezvous point: mutators publish the returned view,
 * mounted surfaces subscribe and patch their local state. It carries no cache
 * and no state of its own — a subscriber that is not mounted simply misses the
 * event, and its next authoritative read (re-query, `song_detail`) is already
 * correct. Deliberately **not** a replacement for the Rust-side
 * `library://songs-invalidated` event: this covers only frontend-initiated
 * mutations, synchronously, without an IPC round-trip.
 */

import type { SongView } from "../../ipc/ipc-types.generated";

type Listener = (song: SongView) => void;

const listeners = new Set<Listener>();

/** Publish an authoritative song view returned by a committed mutation. */
export function publishSongUpdate(song: SongView): void {
  for (const listener of listeners) {
    listener(song);
  }
}

/** Subscribe to committed song updates; returns the unsubscribe handle. */
export function subscribeSongUpdates(listener: Listener): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}
