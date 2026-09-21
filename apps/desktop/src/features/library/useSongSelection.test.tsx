import { act, renderHook } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { useSongSelection } from "./useSongSelection";

describe("useSongSelection", () => {
  it("toggles, replaces and clears stable ids", () => {
    const { result } = renderHook(({ key }) => useSongSelection(key), {
      initialProps: { key: "all|" },
    });

    act(() => result.current.toggle("song-a"));
    expect(Array.from(result.current.selectedIds)).toEqual(["song-a"]);

    act(() => result.current.replace("song-b"));
    expect(Array.from(result.current.selectedIds)).toEqual(["song-b"]);

    act(() => result.current.clear());
    expect(result.current.selectedCount).toBe(0);
  });

  it("selects and deselects only the loaded ids", () => {
    const { result } = renderHook(() => useSongSelection("all|"));

    act(() => result.current.toggle("older-page"));
    act(() => result.current.toggleAllLoaded(["song-a", "song-b"]));
    expect(new Set(result.current.selectedIds)).toEqual(
      new Set(["older-page", "song-a", "song-b"]),
    );

    act(() => result.current.toggleAllLoaded(["song-a", "song-b"]));
    expect(new Set(result.current.selectedIds)).toEqual(new Set(["older-page"]));
  });

  it("clears when the view/query selection scope changes", () => {
    const { result, rerender } = renderHook(({ key }) => useSongSelection(key), {
      initialProps: { key: "all|search-a" },
    });

    act(() => result.current.toggle("song-a"));
    rerender({ key: "all|search-b" });

    expect(result.current.selectedCount).toBe(0);
  });
});
