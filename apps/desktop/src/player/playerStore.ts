/**
 * Player external store (task 10.2 / 11.1).
 *
 * Playback state is **authoritative on the desktop PlayerSnapshot**, which
 * arrives as a throttled IPC event. This module is a `useSyncExternalStore`
 * boundary: it holds the latest snapshot plus a small amount of UI-only
 * transient state (optimistic "pending" volume, the open queue panel), and
 * never fabricates a final value the snapshot has not confirmed.
 *
 * The three-part state split (design §14):
 *  - Core query/mutation state → TanStack-style query cache (see `app/store`).
 *  - Player live state → this external store (snapshot-driven).
 *  - UI temp state → local component reducer (menus, focus, lyric scroll).
 *
 * Components render from the snapshot; a rejected command just leaves it
 * unchanged (authoritative rollback lives in the Rust actor, task 8.8).
 */

import { useSyncExternalStore } from "react";

import { subscribe } from "../bridge";
import type { UnlistenFn } from "../bridge";

/** The UI-facing playback state, mirrored from the IPC `PlayerSnapshot`.
 *  The Rust runtime (`runtime::player::UiPlayerSnapshot`) publishes this camelCase
 *  shape via the `player://snapshot` event; the store just adopts it. */
/** One entry of the playback queue as the queue panel renders it (task 11.2).
 *  Mirrored from the IPC snapshot; `entryId` is the stable queue identity (a
 *  repeated song appears as independent entries), `songId`/`title` identify the
 *  item, and `failed` marks an entry that failed to load/decode this round. */
export interface UiQueueEntry {
  readonly entryId: string;
  readonly songId: string | null;
  readonly title: string | null;
  readonly isCurrent: boolean;
  readonly failed: boolean;
  /** True when this entry is a session-only temporary item (no library song_id)
   *  that can be imported into the active library (task 11.7). */
  readonly canImport: boolean;
}

export interface UiPlayerSnapshot {
  readonly state: "stopped" | "loading" | "playing" | "paused" | "ended" | "failed";
  readonly position: number | null;
  readonly duration: number | null;
  readonly volume: number;
  readonly muted: boolean;
  readonly currentQueueEntryId: string | null;
  readonly currentSongId: string | null;
  readonly queueLen: number;
  readonly mode: "sequential" | "shuffle" | "repeatOne";
  /** Display title of the current entry (temporary file), when not a library song. */
  readonly currentTitle?: string | null;
  /** True when the current entry is a temporary item that can be imported into
   *  the active library (task 11.7). The frontend uses this to show the
   *  "导入到资料库" entry point and disable favorite/playlist controls. */
  readonly currentCanImport: boolean;
  /** The full queue the panel renders (current + pending, in play order). */
  readonly queue: readonly UiQueueEntry[];
}

export const EMPTY_SNAPSHOT: UiPlayerSnapshot = {
  state: "stopped",
  position: null,
  duration: null,
  volume: 1,
  muted: false,
  currentQueueEntryId: null,
  currentSongId: null,
  queueLen: 0,
  mode: "sequential",
  currentTitle: null,
  currentCanImport: false,
  queue: [],
};

/** Transient UI state that must never drive a fake success. */
interface PlayerUiState {
  /** A command we are awaiting a snapshot confirmation for (pending).
   *  A rejected command simply leaves the snapshot untouched. */
  pending: "play" | "pause" | "next" | "previous" | "seek" | null;
  /** The queue panel is open or not (component-local, kept here for the
   *  overlay stack). */
  queueOpen: boolean;
  /** The immersive player is expanded or not (task 11.3, kept here so the
   *  overlay stack and the player bar share the same flag). */
  immersiveOpen: boolean;
  /** The lyrics focus mode is on (task 11.6): hides non-essential cover/meta,
   *  leaves the lyrics as the primary reading area. Kept in the overlay stack
   *  so Escape closes the topmost surface only. */
  focusOpen: boolean;
}

type Listener = () => void;

class PlayerStore {
  private snapshot: UiPlayerSnapshot = EMPTY_SNAPSHOT;
  private ui: PlayerUiState = {
    pending: null,
    queueOpen: false,
    immersiveOpen: false,
    focusOpen: false,
  };
  private listeners = new Set<Listener>();

  getSnapshot(): UiPlayerSnapshot {
    return this.snapshot;
  }

  getUi(): PlayerUiState {
    return this.ui;
  }

  /** Publish a snapshot received from the Desktop (via the bridge). */
  publish(snapshot: UiPlayerSnapshot): void {
    this.snapshot = snapshot;
    this.emit();
  }

  /** Set an optimistic pending action. It is cleared by the next snapshot
   *  publish — the snapshot is always the authority. */
  setPending(pending: PlayerUiState["pending"]): void {
    this.ui = { ...this.ui, pending };
    this.emit();
  }

  setQueueOpen(open: boolean): void {
    this.ui = { ...this.ui, queueOpen: open };
    this.emit();
  }

  /** Open or close the immersive player (task 11.3). */
  setImmersiveOpen(open: boolean): void {
    this.ui = { ...this.ui, immersiveOpen: open };
    this.emit();
  }

  /** Open or close the lyrics focus mode (task 11.6). */
  setFocusOpen(open: boolean): void {
    this.ui = { ...this.ui, focusOpen: open };
    this.emit();
  }

  subscribe(listener: Listener): () => void {
    this.listeners.add(listener);
    return () => {
      this.listeners.delete(listener);
    };
  }

  private emit(): void {
    for (const listener of this.listeners) {
      listener();
    }
  }
}

/** The single shared player store instance. */
export const playerStore = new PlayerStore();

/** A hook that renders from the player snapshot (external store). */
export function usePlayerSnapshot(): UiPlayerSnapshot {
  // Subscribe to the combined snapshot+ui so any change re-renders; the hook
  // returns only the authoritative snapshot.
  return useSyncExternalStore(
    (cb) => playerStore.subscribe(cb),
    () => playerStore.getSnapshot(),
  );
}

/** A hook for the UI-only player state (queue panel open, pending action). */
export function usePlayerUi(): PlayerUiState {
  return useSyncExternalStore(
    (cb) => playerStore.subscribe(cb),
    () => playerStore.getUi(),
  );
}

/** The event the Rust runtime publishes each snapshot (matches `PLAYER_SNAPSHOT_EVENT`). */
export const PLAYER_SNAPSHOT_EVENT = "player://snapshot";

/**
 * Subscribe to the desktop player snapshot stream and feed `playerStore`.
 * Called once at app bootstrap; returns the de-registration handle. The Rust
 * runtime maps the authoritative `PlayerSnapshot` + coordinator queue into the
 * UI shape, so the store never fabricates a final value.
 */
export function startPlayerEvents(): Promise<UnlistenFn> {
  return subscribe<UiPlayerSnapshot>(PLAYER_SNAPSHOT_EVENT, (snapshot) => {
    playerStore.publish(snapshot);
  });
}
