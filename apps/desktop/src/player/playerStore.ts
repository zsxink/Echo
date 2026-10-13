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
 *  - Core query/mutation state → the feature/application query boundary.
 *  - Player live state → this external store (snapshot-driven).
 *  - UI temp state → local component reducer (menus, focus, lyric scroll).
 *
 * Components render from the snapshot; a rejected command just leaves it
 * unchanged (authoritative rollback lives in the Rust actor, task 8.8).
 */

import { useEffect, useState } from "react";

import { subscribe } from "../bridge";
import type { UnlistenFn } from "../bridge";
import type { UiPlayerSnapshot, EffectsSnapshotDto } from "../ipc/ipc-types.generated";
import { ExternalStore, useExternalStore } from "../app/externalStore";

// The event payload is an IPC DTO, so it belongs to the generated contract.
// Re-export from this store for existing presentation consumers while keeping
// Rust's serde shape as the sole source of truth.
export type { UiPlayerSnapshot, UiQueueEntry } from "../ipc/ipc-types.generated";

/** The UI-facing playback state, mirrored from the IPC `PlayerSnapshot`.
 *  The Rust runtime (`runtime::player::UiPlayerSnapshot`) publishes this camelCase
 *  shape via the `player://snapshot` event; the store just adopts it. */
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
  currentArtist: null,
  currentAlbum: null,
  currentCoverKey: null,
  currentLyrics: null,
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
  effectsOpen: boolean;
  effectsTab: "presets" | "equalizer";
  /** The immersive player is expanded or not (task 11.3, kept here so the
   *  overlay stack and the player bar share the same flag). */
  immersiveOpen: boolean;
  /** The lyrics focus mode is on (task 11.6): hides non-essential cover/meta,
   *  leaves the lyrics as the primary reading area. Kept in the overlay stack
   *  so Escape closes the topmost surface only. */
  focusOpen: boolean;
}

interface PlayerStoreState {
  readonly snapshot: UiPlayerSnapshot;
  readonly effects: EffectsSnapshotDto | null;
  readonly ui: PlayerUiState;
  readonly publishedAt: number;
}

class PlayerStore {
  private readonly state = new ExternalStore<PlayerStoreState>({
    snapshot: EMPTY_SNAPSHOT,
    effects: null,
    ui: {
      pending: null,
      queueOpen: false,
      effectsOpen: false,
      effectsTab: "presets",
      immersiveOpen: false,
      focusOpen: false,
    },
    publishedAt: 0,
  });

  getSnapshot(): UiPlayerSnapshot {
    return this.state.getSnapshot().snapshot;
  }

  getUi(): PlayerUiState {
    return this.state.getSnapshot().ui;
  }

  /** Publish a snapshot received from the Desktop (via the bridge). */
  publish(snapshot: UiPlayerSnapshot): void {
    if (snapshot.effects) this.publishEffects(snapshot.effects);
    this.state.update((current) => ({
      ...current,
      snapshot,
      publishedAt: performance.now(),
    }));
  }

  getEffects(): EffectsSnapshotDto | null {
    return this.state.getSnapshot().effects;
  }

  /** Capture order protects metadata too; revisions/epochs support older desktops. */
  publishEffects(effects: EffectsSnapshotDto): void {
    this.state.update((current) => {
      const previousEffects = current.effects;
      const previous = previousEffects?.runtime;
      const incoming = effects.runtime;
      if (
        previousEffects?.snapshotSequence !== undefined &&
        effects.snapshotSequence !== undefined &&
        effects.snapshotSequence <= previousEffects.snapshotSequence
      )
        return current;
      if (
        previous &&
        (incoming.revision < previous.revision ||
          (incoming.revision === previous.revision &&
            incoming.playbackEpoch < previous.playbackEpoch))
      )
        return current;
      // A command/query can finish after its application event. Accepted/Pending
      // retains terminal audio facts for the same request, but its later saved
      // metadata and persistence receipt must still reach the panel.
      if (
        previous &&
        incoming.revision === previous.revision &&
        incoming.playbackEpoch === previous.playbackEpoch &&
        incoming.applied === "pending" &&
        previous.applied !== "pending"
      ) {
        if (
          previousEffects?.snapshotSequence === undefined ||
          effects.snapshotSequence === undefined
        )
          return current;
        return {
          ...current,
          effects: {
            ...effects,
            runtime: { ...previous, persistenceStatus: incoming.persistenceStatus },
            responsePoints: previousEffects.responsePoints,
            referenceResponse: previousEffects.referenceResponse,
            safePreampDb: previousEffects.safePreampDb,
            editableBands: previousEffects.editableBands,
            responseRate: previousEffects.responseRate,
          },
        };
      }
      return { ...current, effects };
    });
  }

  /** When the current snapshot arrived (for position interpolation). */
  publishedAt(): number {
    return this.state.getSnapshot().publishedAt;
  }

  /** Set an optimistic pending action. It is cleared by the next snapshot
   *  publish — the snapshot is always the authority. */
  setPending(pending: PlayerUiState["pending"]): void {
    this.updateUi((ui) => ({ ...ui, pending }));
  }

  setQueueOpen(open: boolean): void {
    this.updateUi((ui) => ({ ...ui, queueOpen: open, effectsOpen: open ? false : ui.effectsOpen }));
  }

  setEffectsOpen(open: boolean): void {
    this.updateUi((ui) => ({ ...ui, effectsOpen: open, queueOpen: open ? false : ui.queueOpen }));
  }

  setEffectsTab(effectsTab: PlayerUiState["effectsTab"]): void {
    this.updateUi((ui) => ({ ...ui, effectsTab }));
  }

  /** Open or close the immersive player (task 11.3). Closing it also drops
   *  歌词专注阅读, which is a state of that surface and would otherwise linger
   *  and silently re-apply the next time the player opens. */
  setImmersiveOpen(open: boolean): void {
    this.updateUi((ui) => ({ ...ui, immersiveOpen: open, focusOpen: open ? ui.focusOpen : false }));
  }

  /** Open or close the lyrics focus mode (task 11.6). */
  setFocusOpen(open: boolean): void {
    this.updateUi((ui) => ({ ...ui, focusOpen: open }));
  }

  subscribe(listener: () => void): () => void {
    return this.state.subscribe(listener);
  }

  stateForRender(): ExternalStore<PlayerStoreState> {
    return this.state;
  }

  private updateUi(updater: (ui: PlayerUiState) => PlayerUiState): void {
    this.state.update((current) => ({ ...current, ui: updater(current.ui) }));
  }
}

/** The single shared player store instance. */
export const playerStore = new PlayerStore();

/** A hook that renders from the player snapshot (external store). */
export function usePlayerSnapshot(): UiPlayerSnapshot {
  // Subscribe to the combined snapshot+ui so any change re-renders; the hook
  // returns only the authoritative snapshot.
  return useExternalStore(playerStore.stateForRender(), (state) => state.snapshot);
}

/** A hook for the UI-only player state (queue panel open, pending action). */
export function usePlayerUi(): PlayerUiState {
  return useExternalStore(playerStore.stateForRender(), (state) => state.ui);
}

export function useAudioEffects(): EffectsSnapshotDto | null {
  return useExternalStore(playerStore.stateForRender(), (state) => state.effects);
}

/** The event the Rust runtime publishes each snapshot (matches `PLAYER_SNAPSHOT_EVENT`). */
export const PLAYER_SNAPSHOT_EVENT = "player://snapshot";

/**
 * A per-frame estimate of the playback position.
 *
 * The desktop publishes snapshots at 10 Hz (foreground throttle), so rendering
 * `snapshot.position` directly makes the progress bar, the transport readout
 * and the lyric highlight step in visible ~100 ms jumps. While the state is
 * `playing`, this hook advances the last authoritative position by wall-clock
 * time via `requestAnimationFrame`; each arriving snapshot re-anchors the
 * estimate, so seek/pause corrections are adopted immediately and drift never
 * accumulates. Paused/stopped states render the snapshot value verbatim —
 * this hook never fabricates a value the snapshot has not confirmed.
 */
export function useSmoothPosition(): number | null {
  const snapshot = usePlayerSnapshot();
  const { position, duration, state } = snapshot;
  const [smooth, setSmooth] = useState<number | null>(position);

  useEffect(() => {
    if (state !== "playing" || position === null) {
      setSmooth(position);
      return;
    }
    const anchoredAt = playerStore.publishedAt();
    let raf = 0;
    const tick = () => {
      const estimate = position + (performance.now() - anchoredAt) / 1000;
      setSmooth(duration !== null ? Math.min(estimate, duration) : estimate);
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [state, position, duration]);

  return smooth;
}

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
