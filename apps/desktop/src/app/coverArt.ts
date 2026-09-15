/**
 * Embedded cover art for the UI (design §115 内置优先).
 *
 * The scan persists the artwork **embedded in the audio file** as an opaque
 * `cv1-…` asset key, and the desktop's `cover://` protocol resolves that key to
 * bytes (design §16). This module is the `useSyncExternalStore` boundary in
 * front of that lookup:
 *
 *  - a song with no embedded artwork is remembered as *asked, none*, so the
 *    caller keeps the prototype's palette placeholder and never re-asks;
 *  - one request carries the whole window, because the lookup is per song and a
 *    list must not issue a command per row;
 *  - ids already asked about (or in flight) are dropped from a request, so a
 *    re-render or a scroll that repeats a window costs no extra round trip;
 *  - the published snapshot changes **only when artwork is actually found**, so
 *    a library whose files carry no artwork never wakes a subscriber up.
 *
 * Nothing is persisted: artwork is a property of the library, so a rescan or a
 * root switch invalidates it ([`invalidateCovers`]) rather than trusting a key
 * that may now point at different bytes. Invalidation bumps a *generation*, and
 * `useCoverKeys` re-asks when either the rendered window or that generation
 * changes — otherwise a rescan would leave every visible row on the placeholder
 * until the user happened to scroll.
 */

import { useEffect, useRef, useSyncExternalStore } from "react";

import { bridge } from "../bridge";

/** One reading of the cache. `generation` changes on invalidation. */
interface CoverSnapshot {
  readonly generation: number;
  /** `songId → asset key`. Only songs that carry artwork appear here. */
  readonly covers: ReadonlyMap<string, string>;
}

let snapshot: CoverSnapshot = { generation: 0, covers: new Map() };
/** Ids already asked about — with or without artwork — so we never re-ask. */
const asked = new Set<string>();
/** Ids with a request in flight, so a re-render cannot duplicate one. */
const inFlight = new Set<string>();
const listeners = new Set<() => void>();

function emit(): void {
  for (const listener of listeners) listener();
}

function subscribe(listener: () => void): () => void {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
}

function getSnapshot(): CoverSnapshot {
  return snapshot;
}

/** Ask the desktop for any of `songIds` whose artwork is not resolved yet. */
export function requestCovers(songIds: readonly string[]): void {
  const wanted = songIds.filter((id) => !asked.has(id) && !inFlight.has(id));
  if (wanted.length === 0) return;
  for (const id of wanted) inFlight.add(id);

  void bridge
    .call<[{ songIds: string[] }], Record<string, string> | null | undefined>("song_cover_keys", {
      songIds: wanted,
    })
    .then((found) => {
      // A shell that cannot answer the command (an older build, a test double
      // with no handler) resolves to a non-object: every song is then
      // artwork-less, which is the same end state as a file with no embedded
      // cover — never a thrown request that loses the whole batch.
      const answer = found ?? {};
      const next = new Map(snapshot.covers);
      let changed = false;
      for (const id of wanted) {
        asked.add(id);
        const key = answer[id];
        if (typeof key === "string" && key.length > 0) {
          next.set(id, key);
          changed = true;
        }
      }
      // No artwork in this window is the common case: republishing an identical
      // map would wake every subscriber for no visible change.
      if (!changed) return;
      snapshot = { generation: snapshot.generation, covers: next };
      emit();
    })
    .catch(() => {
      // A failed lookup must not become a retry loop on every render: record the
      // ids as asked so the placeholder stands. A rescan (or a root switch)
      // clears them again.
      for (const id of wanted) asked.add(id);
    })
    .finally(() => {
      for (const id of wanted) inFlight.delete(id);
    });
}

/**
 * Drop every cached key and force the next render of any window to re-ask:
 * a rescan or a root switch can change the bytes behind an unchanged `SongId`.
 */
export function invalidateCovers(): void {
  asked.clear();
  inFlight.clear();
  snapshot = { generation: snapshot.generation + 1, covers: new Map() };
  emit();
}

/**
 * The artwork keys of the songs currently on screen.
 *
 * The *window* is identified by its id list, not by array identity: a re-render
 * that renders the same songs never re-asks the backend, while a scroll that
 * changes the window does. A song absent from the returned map has no embedded
 * artwork (or has not been answered yet); either way its placeholder stands, and
 * a late answer for one row never reflows the others.
 */
export function useCoverKeys(songIds: readonly string[]): ReadonlyMap<string, string> {
  const current = useSyncExternalStore(subscribe, getSnapshot);
  const latest = useRef(songIds);
  latest.current = songIds;
  // Re-ask when the rendered window changes *or* when the cache was invalidated
  // underneath it — the window alone would not change on a rescan.
  const requestKey = `${current.generation}\u0001${songIds.join("\u0000")}`;
  useEffect(() => {
    requestCovers(latest.current);
  }, [requestKey]);
  return current.covers;
}
