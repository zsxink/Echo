/**
 * `useSongs` playlist scoping (playlist-search-locate-import).
 *
 * A 歌单 view fetches every page through the scoped `search` command (playlist
 * included), never through `all_songs`, and keys the cache by `playlistId` so
 * switching playlists re-fetches instead of reusing another playlist's rows.
 * The scope travels as the `playlist` argument — the name Tauri maps `search`'s
 * Rust `playlist: Option<String>` parameter to.
 */

import { renderHook } from "@testing-library/react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { bridge } from "../../bridge";
import type { PagedSongs } from "../../ipc/ipc-types.generated";

vi.mock("../../bridge", () => ({
  bridge: { call: vi.fn() },
}));

import { useSongs } from "./useSongs";

const call = vi.mocked(bridge.call);

function emptyPage(): PagedSongs {
  return { items: [], totalCount: 0, isLast: true };
}

const baseQuery = {
  view: "playlist" as const,
  search: "",
  sort: { field: "addedAt" as const, direction: "desc" as const },
  inFavorites: false,
  playlistId: "playlist-1",
  root: "root-1",
  readOnly: false,
};

beforeEach(() => {
  call.mockReset();
  call.mockResolvedValue(emptyPage());
});

describe("useSongs 歌单视图分发", () => {
  it("playlist 视图带搜索词时调用带 playlistId 的 search", async () => {
    renderHook(() => useSongs({ ...baseQuery, search: "晴天" }));
    await vi.waitFor(() => expect(call).toHaveBeenCalled());
    expect(call).toHaveBeenCalledWith("search", {
      query: "晴天",
      inFavorites: false,
      playlist: "playlist-1",
      sort: "addedAt:desc",
      cursor: null,
      limit: 200,
    });
  });

  it("playlist 视图空搜索词也走 search（恢复歌单全量），不分发到 all_songs", async () => {
    renderHook(() => useSongs(baseQuery));
    await vi.waitFor(() => expect(call).toHaveBeenCalled());
    expect(call).toHaveBeenCalledWith("search", {
      query: "",
      inFavorites: false,
      playlist: "playlist-1",
      sort: "addedAt:desc",
      cursor: null,
      limit: 200,
    });
    const commands = call.mock.calls.map(([command]) => command);
    expect(commands).not.toContain("all_songs");
  });

  it("playlistId 变化触发新的抓取请求（缓存键区分布同歌单）", async () => {
    const { rerender } = renderHook(({ playlistId }) => useSongs({ ...baseQuery, playlistId }), {
      initialProps: { playlistId: "playlist-1" },
    });
    await vi.waitFor(() => expect(call).toHaveBeenCalledTimes(1));
    rerender({ playlistId: "playlist-2" });
    await vi.waitFor(() => expect(call).toHaveBeenCalledTimes(2));
    expect(call).toHaveBeenLastCalledWith(
      "search",
      expect.objectContaining({ playlist: "playlist-2" }),
    );
  });
});
