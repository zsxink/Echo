/**
 * The embedded-artwork store (design §115 内置优先).
 *
 * The list must ask the desktop once per rendered window — never per row — and
 * must not turn "this file has no embedded cover" into an endless retry or a
 * pointless re-render. These tests lock that contract: batching, one-shot
 * resolution, snapshot stability when nothing was found, and re-asking after a
 * rescan/root switch invalidates the cache.
 */

import { invoke } from "@tauri-apps/api/core";
import { act, renderHook, waitFor } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { invalidateCovers, useCoverKeys } from "./coverArt";

/** The hook the test setup exposes on `globalThis` to seed command results. */
const testHooks = (
  globalThis as unknown as {
    __echoTest: { setInvoke(command: string, value: unknown): void };
  }
).__echoTest;

const mockedInvoke = vi.mocked(invoke);

/** Every `song_cover_keys` request the app has issued so far. */
function coverRequests(): unknown[][] {
  return mockedInvoke.mock.calls
    .filter(([command]) => command === "song_cover_keys")
    .map(([, args]) => [args]);
}

beforeEach(() => {
  // The shared Tauri stub records calls for the whole file; each case counts
  // only its own.
  mockedInvoke.mockClear();
});

describe("useCoverKeys", () => {
  it("resolves a whole window with one command", async () => {
    testHooks.setInvoke("song_cover_keys", { "song-1": "cv1-a" });

    const { result } = renderHook(() => useCoverKeys(["song-1", "song-2"]));

    await waitFor(() => expect(result.current.get("song-1")).toBe("cv1-a"));
    // A song absent from the answer has no embedded artwork — not "pending".
    expect(result.current.get("song-2")).toBeUndefined();
    expect(coverRequests()).toEqual([[{ songIds: ["song-1", "song-2"] }]]);
  });

  it("does not re-ask for a window it has already resolved", async () => {
    testHooks.setInvoke("song_cover_keys", { "song-1": "cv1-a" });

    const { result, rerender } = renderHook((props: { ids: string[] }) => useCoverKeys(props.ids), {
      initialProps: { ids: ["song-1", "song-2"] },
    });
    await waitFor(() => expect(result.current.get("song-1")).toBe("cv1-a"));

    rerender({ ids: ["song-1", "song-2"] });
    rerender({ ids: ["song-1", "song-2"] });

    expect(coverRequests()).toHaveLength(1);
  });

  it("keeps the snapshot identity when a window turns out to have no artwork", async () => {
    // `{}` is the answer for a library whose files carry no embedded cover.
    testHooks.setInvoke("song_cover_keys", {});

    const { result, rerender } = renderHook((props: { ids: string[] }) => useCoverKeys(props.ids), {
      initialProps: { ids: ["song-1"] },
    });
    await waitFor(() => expect(coverRequests()).toHaveLength(1));
    const settled = result.current;

    rerender({ ids: ["song-1"] });

    // Nothing was found, so nothing is republished: subscribers are not woken
    // for a change that cannot alter what they render.
    expect(result.current).toBe(settled);
    expect(settled.size).toBe(0);
  });

  it("treats a failed lookup as 'no artwork' instead of retrying on every render", async () => {
    // No handler registered for the command, so the bridge rejects.
    const { result, rerender } = renderHook((props: { ids: string[] }) => useCoverKeys(props.ids), {
      initialProps: { ids: ["song-1"] },
    });
    await waitFor(() => expect(coverRequests()).toHaveLength(1));

    rerender({ ids: ["song-1"] });
    rerender({ ids: ["song-1"] });

    expect(coverRequests()).toHaveLength(1);
    expect(result.current.size).toBe(0);
  });

  it("re-asks the visible window after the cache is invalidated", async () => {
    testHooks.setInvoke("song_cover_keys", { "song-1": "cv1-a" });

    const { result } = renderHook(() => useCoverKeys(["song-1"]));
    await waitFor(() => expect(result.current.get("song-1")).toBe("cv1-a"));
    expect(coverRequests()).toHaveLength(1);

    // A rescan or a root switch can change the bytes behind an unchanged SongId.
    act(() => {
      invalidateCovers();
    });

    await waitFor(() => expect(result.current.get("song-1")).toBe("cv1-a"));
    expect(coverRequests()).toHaveLength(2);
  });
});
