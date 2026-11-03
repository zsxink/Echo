/**
 * Mock Tauri bridge for the browser E2E (task 13.1).
 *
 * The desktop UI talks to the backend exclusively through
 * `@tauri-apps/api/core.invoke` (and `@tauri-apps/api/event.listen`), which in
 * a real build read `window.__TAURI_INTERNALS__`. This module installs a
 * controllable mock of those globals so the *unmodified* app can run in a
 * plain Chromium against an in-page fake backend. It implements just enough of
 * the command/event contract (the camelCase DTOs in
 * `src/ipc/ipc-types.generated.ts`) to drive the non-platform PRD acceptance
 * paths A1–A10, A12–A14 (A11 delete/undo is exercised by the Rust fault
 * matrix + the UI component tests; here we cover its UI-facing surface).
 *
 * The mock is reset per scenario via `mockBridge.reset()` and asserted through
 * the `mockBridge.spy` call log exposed on `window.__echoE2E__`.
 *
 * This file owns the bridge runtime itself — the handler table, the event
 * listener registry and the page installer. The pieces it is built from live
 * beside it, one responsibility each:
 *
 * - `mock-bridge-commands.ts` — the command contract (the name union, the
 *   shared handler signature and the two DTO helpers).
 * - `mock-bridge-handlers-library.ts` / `-playback.ts` / `-sync.ts` /
 *   `-effects.ts` — the handler tables, one per domain.
 * - `mock-bridge-player-streams.ts` — the position/queue/effects stream
 *   projections, mirroring the runtime's split streams.
 */

/** The subset of the generated DTOs the browser journey needs. */
import { applySeed, defaultState } from "./mock-bridge-state";
import type { E2EState } from "./mock-bridge-state";
export type { MockSong } from "./mock-bridge-state";
import { coverAssetUrl } from "./mock-cover";
import type { AnyRecord, Command, Handler } from "./mock-bridge-commands";
import { MOCK_NAMES_EVERY_COMMAND } from "./mock-bridge-commands";
import { buildEffectsHandlers } from "./mock-bridge-handlers-effects";
import { buildLibraryHandlers } from "./mock-bridge-handlers-library";
import { buildPlaybackHandlers } from "./mock-bridge-handlers-playback";
import { buildSyncHandlers } from "./mock-bridge-handlers-sync";
import {
  PLAYER_COMMANDS,
  PLAYER_EFFECTS_EVENT,
  PLAYER_QUEUE_EVENT,
  PLAYER_SNAPSHOT_EVENT,
  queueProjectionOf,
  queueSignatureOf,
  snapshotOf,
} from "./mock-bridge-player-streams";

export { MOCK_NAMES_EVERY_COMMAND };
export type { AnyRecord, Command, Handler };

/** Build the default handler table for the mock backend.
 *
 * Partial by design: `Command` names the whole bridge contract so it can be
 * checked against the real one, while this table answers only the part the
 * browser journey drives. Anything named but not answered throws with its own
 * name — see `invoke`. */
function buildHandlers(state: E2EState): Partial<Record<Command, Handler>> {
  return {
    ...buildLibraryHandlers(state),
    ...buildPlaybackHandlers(state),
    ...buildSyncHandlers(state),
    ...buildEffectsHandlers(),
  };
}

/** The mock internals installed on `window.__TAURI_INTERNALS__`. */
export class MockBridge {
  private handlers: Partial<Record<Command, Handler>>;
  readonly state: E2EState;
  readonly calls: string[] = [];
  private listeners = new Map<string, Set<(payload: unknown) => void>>();
  /** Callback registry behind the mocked `transformCallback` (event handlers). */
  private callbacks = new Map<number, (event: unknown) => void>();
  private nextCallbackId = 1;
  /** The revision of the queue projection the frontend holds (issue #112). */
  private queueRevision = 0;
  /** The content the last published projection was built from. */
  private queueSignature = "";

  constructor() {
    this.state = defaultState();
    this.handlers = buildHandlers(this.state);
  }

  reset() {
    Object.assign(this.state, defaultState());
    this.calls.length = 0;
    this.listeners.clear();
    this.callbacks.clear();
    this.nextCallbackId = 1;
    this.queueRevision = 0;
    this.queueSignature = "";
    this.handlers = buildHandlers(this.state);
  }

  /**
   * Mirror the real `transformCallback`: park the callback under `_<id>` on the
   * page and hand back the id. `@tauri-apps/api/event` then passes that id to
   * `plugin:event|listen`, which is how the mock learns which function to call
   * when it emits.
   */
  transformCallback(callback: (event: unknown) => void): number {
    const id = this.nextCallbackId;
    this.nextCallbackId += 1;
    this.callbacks.set(id, callback);
    (window as unknown as Record<string, unknown>)[`_${id}`] = callback;
    return id;
  }

  /** Replace or append a command handler for a specific scenario. */
  stub(command: Command, fn: Handler) {
    (this.handlers as Record<string, Handler>)[command] = fn;
  }

  /**
   * Publish the queue projection when the queue changed.
   *
   * The real runtime rebuilds the projection only when the revision moved; the
   * mock compares the content signature instead, so a scenario sees the same
   * "the queue did not change, nothing was published" behaviour.
   */
  private publishQueueIfChanged(): void {
    const signature = queueSignatureOf(this.state);
    if (signature === this.queueSignature) return;
    this.queueSignature = signature;
    this.queueRevision += 1;
    this.emit(PLAYER_QUEUE_EVENT, queueProjectionOf(this.state, this.queueRevision));
  }

  /** Publish the current queue projection and effects snapshot on demand. */
  private publishPlayerStreams(): void {
    this.publishQueueIfChanged();
    this.emit(PLAYER_EFFECTS_EVENT, this.handlers.get_audio_effects_snapshot?.({}, this.state));
  }

  invoke(cmd: string, args: unknown): unknown {
    this.calls.push(cmd);
    if (cmd === "plugin:event|listen") {
      // `listen(event, handler)` registers the transformCallback id here; wire
      // it to the same event so `emit` reaches the app's subscriber.
      const { event, handler } = (args ?? {}) as { event?: string; handler?: number };
      const callback = handler === undefined ? undefined : this.callbacks.get(handler);
      if (event !== undefined && callback) {
        this.on(event, (payload) => callback({ event, id: handler, payload }));
        // The runtime publishes the current snapshot as soon as a subscriber
        // attaches, so a seeded now-playing item is on screen without a click.
        if (event === PLAYER_SNAPSHOT_EVENT) {
          this.emit(event, snapshotOf(this.state, this.queueRevision));
        }
      }
      return String(handler ?? this.nextCallbackId);
    }
    if (cmd === "plugin:event|unlisten") {
      return null;
    }
    // Window-plugin commands (issue #76): the Windows caption buttons go
    // through `getCurrentWindow()`, which invokes these names. Modeled
    // statefully so the e2e can assert the toggle's two phases.
    if (cmd === "plugin:window|is_maximized") {
      return this.state.windowMaximized;
    }
    if (cmd === "plugin:window|minimize") {
      this.state.windowActions.push("minimize");
      return null;
    }
    if (cmd === "plugin:window|toggle_maximize") {
      this.state.windowMaximized = !this.state.windowMaximized;
      this.state.windowActions.push("toggle_maximize");
      // The real runtime fires `tauri://resize` on every state change; the
      // caption glyph listens to it, so the mock must too.
      this.emit("tauri://resize", null);
      return null;
    }
    if (cmd === "plugin:window|close") {
      this.state.windowActions.push("close");
      return null;
    }
    const handler = this.handlers[cmd as Command];
    if (!handler) {
      // Either the app invokes a command this double has never heard of, or it
      // names one in `Command` and deliberately leaves it unmodelled. Both are
      // drift between the app and the mock; the command name is the whole of
      // what a reader needs to act on it.
      throw new Error(`mock-bridge: \`${cmd}\` has no handler`);
    }
    const result = handler((args ?? {}) as AnyRecord, this.state);
    // The catch-up entry point of the split streams (issue #112): the frontend
    // asks for the current queue projection and effects snapshot once, after it
    // registered its listeners.
    if (cmd === "player_sync") {
      this.publishPlayerStreams();
      return result;
    }
    // A playback request republishes the snapshot, as the real runtime does
    // once the actor has accepted it.
    if (PLAYER_COMMANDS.has(cmd)) {
      this.publishQueueIfChanged();
      this.emit(PLAYER_SNAPSHOT_EVENT, snapshotOf(this.state, this.queueRevision));
    }
    return result;
  }

  emit(event: string, payload: unknown) {
    for (const fn of this.listeners.get(event) ?? []) fn(payload);
  }

  on(event: string, fn: (payload: unknown) => void) {
    if (!this.listeners.has(event)) this.listeners.set(event, new Set());
    this.listeners.get(event)!.add(fn);
  }
}

/**
 * Install the mock on the page before the app bundle runs.
 *
 * A scenario seed may be carried in the URL query (`?seed=<name>`) so the
 * browser E2E can bootstrap a deterministic initial library state across
 * navigations/reloads (a fresh `document` always re-runs this installer, and
 * the app hydrates once). The driver picks the seed and navigates to it.
 */
export function installMockBridge(): MockBridge {
  const bridge = new MockBridge();
  const params = new URLSearchParams(window.location.search);
  const seed = params.get("seed");
  if (seed) applySeed(bridge.state, seed);
  // Platform forcing (issue #76): a scenario pins the platform-gated chrome
  // branch by pre-setting the same `documentElement.dataset.platform` value
  // `applyPlatform()` publishes; production never pre-sets it, so an unset
  // parameter leaves the sniffing path untouched (`web` in the browser).
  const platform = params.get("platform");
  if (platform) document.documentElement.dataset.platform = platform;
  (window as unknown as Record<string, unknown>).__TAURI_INTERNALS__ = {
    // `getCurrentWindow()` (the window-control facade) reads this label.
    metadata: { currentWindow: { label: "main" } },
    invoke: (cmd: string, args: unknown, _options?: unknown) => bridge.invoke(cmd, args),
    transformCallback: (cb: (event: unknown) => void) => bridge.transformCallback(cb),
    // The e2e build has no cover protocol, so covers resolve to a data: URI;
    // `assetUrl` recognizes a non-cover base and leaves it without a tier
    // suffix.
    convertFileSrc: (path: string, protocol = "asset") =>
      protocol === "cover" ? coverAssetUrl(path) : `${protocol}://${btoa(path)}`,
  };
  // event plugin internals used by `@tauri-apps/api/event`:
  (window as unknown as Record<string, unknown>).__TAURI_EVENT_PLUGIN_INTERNALS__ = {
    unregisterListener: () => {},
  };
  (window as unknown as Record<string, unknown>).__echoE2E__ = bridge;
  return bridge;
}
