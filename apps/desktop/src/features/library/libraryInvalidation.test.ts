/**
 * Cross-window invalidation bridge.
 *
 * Each window owns its own JavaScript context, so a catalog mutation made from
 * the notch player (favorite) is invisible to the main window's React state
 * until the shell broadcasts it. These cases pin that bridge.
 */

import { afterEach, describe, expect, it, vi } from "vitest";

import { startLibraryEvents, subscribeLibraryInvalidations } from "./libraryInvalidation";

const echoTest = (
  globalThis as unknown as { __echoTest: { emit(event: string, payload: unknown): void } }
).__echoTest;

afterEach(() => {
  vi.restoreAllMocks();
});

describe("library invalidation bridge", () => {
  it("re-reads when the shell reports a catalog mutation from another window", async () => {
    const listener = vi.fn();
    const stop = subscribeLibraryInvalidations(listener);
    const dispose = startLibraryEvents();
    // `listen` is async; give the subscription a macrotask to register.
    await new Promise((resolve) => setTimeout(resolve, 0));

    echoTest.emit("library://invalidated", { reason: "favorite" });

    expect(listener).toHaveBeenCalled();
    dispose();
    stop();
  });

  it("stops listening once disposed", async () => {
    const listener = vi.fn();
    const stop = subscribeLibraryInvalidations(listener);
    const dispose = startLibraryEvents();
    await new Promise((resolve) => setTimeout(resolve, 0));
    dispose();

    echoTest.emit("library://invalidated", { reason: "favorite" });

    expect(listener).not.toHaveBeenCalled();
    stop();
  });
});
