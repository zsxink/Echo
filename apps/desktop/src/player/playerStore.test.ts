/**
 * Task 11.1 — the player store is a `useSyncExternalStore` boundary driven by
 * the desktop `player://snapshot` event. `startPlayerEvents()` must subscribe
 * that event and forward each server snapshot into `playerStore.publish()`,
 * so the UI never fabricates a final value.
 */

import { beforeEach, describe, expect, it, vi } from "vitest";
import type { EffectsSnapshotDto } from "../ipc/ipc-types.generated";

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

function effectsSnapshot(snapshotSequence?: number): EffectsSnapshotDto {
  const curve = { gainsDb: Array(10).fill(0), preampMode: "auto" as const, requestedPreampDb: 0 };
  return {
    snapshotSequence,
    document: {
      schemaVersion: 1,
      registryVersion: 1,
      userPresets: [],
      selection: { kind: "draft" },
      retainedPayload: { kind: "eq", ...curve },
      draft: curve,
      requestedEnabled: true,
    },
    runtime: {
      revision: 4,
      playbackEpoch: 2,
      persistenceStatus: "unsaved",
      applied: "pending",
      effectivePreampDb: null,
      activeBands: [],
      processingRate: null,
      channelLayout: null,
      reason: null,
    },
    presets: [],
    responsePoints: [],
    referenceResponse: true,
    safePreampDb: 0,
  };
}

describe("effects snapshot ordering", () => {
  beforeEach(() => {
    playerStore.stateForRender().update((state) => ({ ...state, effects: null }));
  });

  it("rejects delayed CRUD replies and queries at the same audio revision and epoch", () => {
    const oldQuery = effectsSnapshot(1);
    const curve = oldQuery.document.draft!;
    const saved: EffectsSnapshotDto = {
      ...oldQuery,
      snapshotSequence: 2,
      document: {
        ...oldQuery.document,
        draft: null,
        selection: { kind: "preset", id: "user:one" },
        userPresets: [{ id: "user:one", name: "Saved", curve }],
      },
      presets: [
        {
          id: "user:one",
          name: "Saved",
          description: "",
          source: "user",
          payload: { kind: "eq", ...curve },
        },
      ],
    };
    const renamed: EffectsSnapshotDto = {
      ...saved,
      snapshotSequence: 3,
      document: { ...saved.document, userPresets: [{ id: "user:one", name: "Renamed", curve }] },
      presets: [{ ...saved.presets[0], name: "Renamed" }],
    };
    const deleted: EffectsSnapshotDto = {
      ...renamed,
      snapshotSequence: 4,
      document: { ...renamed.document, selection: { kind: "none" }, userPresets: [] },
      presets: [],
    };
    playerStore.publishEffects(renamed);
    playerStore.publishEffects(saved);
    playerStore.publishEffects(oldQuery);
    expect(playerStore.getEffects()).toEqual(renamed);
    playerStore.publishEffects(deleted);
    playerStore.publishEffects(renamed);
    expect(playerStore.getEffects()).toEqual(deleted);
  });

  it("accepts later metadata and persistence while retaining terminal audio facts", () => {
    const pending = effectsSnapshot(1);
    const applied: EffectsSnapshotDto = {
      ...pending,
      referenceResponse: false,
      responsePoints: [{ frequencyHz: 1000, gainDb: -3 }],
      safePreampDb: -3,
      runtime: {
        ...pending.runtime,
        applied: "applied",
        effectivePreampDb: -3,
        processingRate: 48_000,
        channelLayout: "stereo",
        activeBands: Array(10).fill(true),
      },
    };
    const metadata: EffectsSnapshotDto = {
      ...pending,
      snapshotSequence: 2,
      recoveryReason: "protected document",
      document: {
        ...pending.document,
        userPresets: [{ id: "user:one", name: "New curve", curve: pending.document.draft! }],
      },
      runtime: { ...pending.runtime, persistenceStatus: "failed" },
    };
    playerStore.publishEffects(applied);
    playerStore.publishEffects(metadata);
    expect(playerStore.getEffects()).toEqual({
      ...metadata,
      runtime: { ...applied.runtime, persistenceStatus: "failed" },
      referenceResponse: applied.referenceResponse,
      responsePoints: applied.responsePoints,
      safePreampDb: applied.safePreampDb,
    });
    playerStore.publishEffects({
      ...metadata,
      snapshotSequence: 3,
      recoveryReason: undefined,
      runtime: { ...metadata.runtime, persistenceStatus: "saved" },
    });
    expect(playerStore.getEffects()?.runtime.applied).toBe("applied");
    expect(playerStore.getEffects()?.runtime.persistenceStatus).toBe("saved");
    expect(playerStore.getEffects()?.recoveryReason).toBeUndefined();
  });

  it("keeps revision and epoch guards for DTOs without a capture sequence", () => {
    const current = effectsSnapshot();
    playerStore.publishEffects(current);
    playerStore.publishEffects({ ...current, runtime: { ...current.runtime, revision: 3 } });
    playerStore.publishEffects({ ...current, runtime: { ...current.runtime, playbackEpoch: 1 } });
    expect(playerStore.getEffects()).toEqual(current);
  });

  it("does not let legacy pending replies replace terminal metadata", () => {
    const pending = effectsSnapshot();
    const terminal: EffectsSnapshotDto = {
      ...pending,
      document: { ...pending.document, selection: { kind: "none" } },
      runtime: { ...pending.runtime, applied: "bypassed", persistenceStatus: "saved" },
    };
    playerStore.publishEffects(terminal);
    playerStore.publishEffects(pending);
    expect(playerStore.getEffects()).toEqual(terminal);
  });
});
