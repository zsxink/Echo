import { act, renderHook, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { useSongs, type SongQuery } from "./useSongs";

const mocks = (
  globalThis as unknown as {
    __echoTest: { setInvoke: (command: string, value: unknown) => void };
  }
).__echoTest;

const query: SongQuery = {
  view: "all",
  search: "",
  sort: { field: "addedAt", direction: "desc" },
  inFavorites: false,
  readOnly: false,
};

describe("useSongs search command", () => {
  beforeEach(() => {
    mocks.setInvoke("all_songs", { items: [], isLast: true, nextCursor: null });
    mocks.setInvoke("search", { items: [], isLast: true, nextCursor: null });
  });

  it("uses Tauri's camelCase argument name for the favorites filter", async () => {
    const { rerender } = renderHook(({ value }) => useSongs(value), {
      initialProps: { value: query },
    });

    await waitFor(() => expect(invoke).toHaveBeenCalledWith("all_songs", expect.anything()));
    act(() => rerender({ value: { ...query, search: "Echo" } }));

    await waitFor(() => expect(invoke).toHaveBeenCalledWith("search", expect.anything()));
    const calls = (invoke as unknown as { mock: { calls: unknown[][] } }).mock.calls;
    const [, payload] = calls.find(([command]) => command === "search")!;
    expect(payload).toMatchObject({
      query: "Echo",
      inFavorites: false,
      sort: "addedAt:desc",
      cursor: null,
      limit: 200,
    });
    expect(payload).not.toHaveProperty("in_favorites");
  });

  it("discards a delayed old-root response after the active root changes", async () => {
    let resolveOld!: (value: unknown) => void;
    const oldReply = new Promise((resolve) => {
      resolveOld = resolve;
    });
    vi.mocked(invoke)
      .mockImplementationOnce(() => oldReply as never)
      .mockResolvedValueOnce({
        items: [{ id: "new-root-song", title: "新资料库" }],
        isLast: true,
        nextCursor: null,
      } as never);

    const { result, rerender } = renderHook(({ value }) => useSongs(value), {
      initialProps: { value: { ...query, root: "root-old" } },
    });
    await waitFor(() => expect(invoke).toHaveBeenCalled());
    act(() => rerender({ value: { ...query, root: "root-new" } }));
    await waitFor(() => expect(result.current.page.songs[0]?.id).toBe("new-root-song"));

    await act(async () => {
      resolveOld({
        items: [{ id: "old-root-song", title: "旧资料库" }],
        isLast: true,
        nextCursor: null,
      });
      await Promise.resolve();
    });
    expect(result.current.page.songs[0]?.id).toBe("new-root-song");
  });

  it("sends normalized search through the recent view command", async () => {
    mocks.setInvoke("recent", []);
    renderHook(() => useSongs({ ...query, view: "recent", search: "  夜晚  " }));

    await waitFor(() => expect(invoke).toHaveBeenCalledWith("recent", { query: "夜晚" }));
  });

  it("uses the filtered search command for favorites instead of bypassing the query", async () => {
    renderHook(() => useSongs({ ...query, view: "favorites", inFavorites: true, search: "爵士" }));

    await waitFor(() =>
      expect(invoke).toHaveBeenCalledWith("search", {
        query: "爵士",
        inFavorites: true,
        sort: "addedAt:desc",
        cursor: null,
        limit: 200,
      }),
    );
  });

  it("puts a newly favorited song at the top of the loaded favorites", async () => {
    mocks.setInvoke("favorites", {
      items: [{ id: "older", title: "Earlier favorite" }],
      isLast: true,
      nextCursor: null,
    });
    const { result } = renderHook(() => useSongs({ ...query, view: "favorites", inFavorites: true }));

    await waitFor(() => expect(result.current.page.songs.map((song) => song.id)).toEqual(["older"]));
    act(() => result.current.patchSong({ id: "newest", title: "Just favorited", favorite: true } as never));

    expect(result.current.page.songs.map((song) => song.id)).toEqual(["newest", "older"]);
  });

  it("inserts a newly favorited song into the active manual order", async () => {
    mocks.setInvoke("favorites", {
      items: [{ id: "zebra", title: "Zebra" }],
      isLast: true,
      nextCursor: null,
    });
    const { result } = renderHook(() =>
      useSongs({ ...query, view: "favorites", inFavorites: true, sort: { field: "title", direction: "asc" } }),
    );

    await waitFor(() => expect(result.current.page.songs.map((song) => song.id)).toEqual(["zebra"]));
    act(() => result.current.patchSong({ id: "apple", title: "Apple", favorite: true } as never));

    expect(result.current.page.songs.map((song) => song.id)).toEqual(["apple", "zebra"]);
  });
});
