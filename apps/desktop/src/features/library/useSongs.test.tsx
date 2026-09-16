import { act, renderHook, waitFor } from "@testing-library/react";
import { invoke } from "@tauri-apps/api/core";
import { beforeEach, describe, expect, it } from "vitest";

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
});
