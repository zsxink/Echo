/**
 * Task 11.1 — the player store is a `useSyncExternalStore` boundary driven by
 * the desktop `player://snapshot` event. `startPlayerEvents()` must subscribe
 * that event and forward each server snapshot into `playerStore.publish()`,
 * so the UI never fabricates a final value.
 */

import { describe, expect, it, vi } from "vitest";

import {
  PLAYER_SNAPSHOT_EVENT,
  playerStore,
  startPlayerEvents,
  type UiPlayerSnapshot,
} from "./playerStore";

// Override the test setup's inert `listen` with one that captures the handler
// per event, so the test can drive the subscription like the real desktop.
const { listeners } = vi.hoisted(() => ({
  listeners: new Map<string, (payload: unknown) => void>(),
}));

vi.mock("@tauri-apps/api/event", () => ({
  listen: vi.fn(async (event: string, cb: (e: { payload: unknown }) => void) => {
    listeners.set(event, (payload) => cb({ payload }));
    return () => {
      listeners.delete(event);
    };
  }),
}));

function makeSnapshot(overrides: Partial<UiPlayerSnapshot> = {}): UiPlayerSnapshot {
  return {
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
    ...overrides,
  };
}

describe("playerStore task 11.1", () => {
  it("publish() replaces the authoritative snapshot in the external store", () => {
    const snap = makeSnapshot({ state: "playing", position: 12.5, currentSongId: "s1" });
    playerStore.publish(snap);
    expect(playerStore.getSnapshot()).toEqual(snap);
    expect(playerStore.getSnapshot().state).toBe("playing");
  });

  it("startPlayerEvents subscribes the player://snapshot event into the store", async () => {
    const unlisten = await startPlayerEvents();
    const handler = listeners.get(PLAYER_SNAPSHOT_EVENT);
    expect(handler).toBeDefined();
    expect(listeners.has(PLAYER_SNAPSHOT_EVENT)).toBe(true);

    const playing = makeSnapshot({ state: "playing", currentSongId: "s-abc" });
    handler?.(playing);
    expect(playerStore.getSnapshot().state).toBe("playing");
    expect(playerStore.getSnapshot().currentSongId).toBe("s-abc");

    // A later snapshot can move the state (pause), proving the store follows
    // the server rather than caching a stale value.
    handler?.(makeSnapshot({ state: "paused" }));
    expect(playerStore.getSnapshot().state).toBe("paused");

    await unlisten();
    expect(listeners.has(PLAYER_SNAPSHOT_EVENT)).toBe(false);
  });
});
