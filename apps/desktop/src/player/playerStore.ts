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

/** The UI-facing playback state, mirrored from the IPC `PlayerSnapshot`.
 *  The generated DTO does not yet carry a player snapshot, so the bridge maps
 *  the Rust `PlayerSnapshot` into this stable camelCase shape. */
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
};

/** Transient UI state that must never drive a fake success. */
interface PlayerUiState {
  /** A command we are awaiting a snapshot confirmation for (pending).
   *  A rejected command simply leaves the snapshot untouched. */
  pending: "play" | "pause" | "next" | "previous" | "seek" | null;
  /** The queue panel is open or not (component-local, kept here for the
   *  overlay stack). */
  queueOpen: boolean;
}

type Listener = () => void;

class PlayerStore {
  private snapshot: UiPlayerSnapshot = EMPTY_SNAPSHOT;
  private ui: PlayerUiState = { pending: null, queueOpen: false };
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
