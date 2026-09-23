import { act, fireEvent, render, renderHook, screen } from "@testing-library/react";
import { describe, expect, it, vi } from "vitest";

import { SongSortControl } from "./SongSortControl";
import { SORT_FIELDS } from "./types";
import { useStoredSongSort } from "./useStoredSongSort";

describe("SongSortControl", () => {
  it("uses 歌手 terminology, offers 专辑, and updates the selected sort", () => {
    const onChange = vi.fn();
    render(<SongSortControl sort={{ field: "artist", direction: "asc" }} onChange={onChange} />);

    fireEvent.click(screen.getByTestId("sort-button"));
    expect(screen.getByRole("menuitemradio", { name: "歌手" })).toHaveAttribute(
      "aria-checked",
      "true",
    );
    fireEvent.click(screen.getByRole("menuitemradio", { name: "专辑" }));
    expect(onChange).toHaveBeenCalledWith({ field: "album", direction: "asc" });
  });

  it("allows a consumer to hide sort options that do not apply to its view", () => {
    render(
      <SongSortControl
        sort={{ field: "addedAt", direction: "desc" }}
        onChange={vi.fn()}
        fields={SORT_FIELDS.filter(({ value }) => value !== "album")}
      />,
    );

    fireEvent.click(screen.getByTestId("sort-button"));
    expect(screen.queryByRole("menuitemradio", { name: "专辑" })).not.toBeInTheDocument();
  });
});

describe("useStoredSongSort", () => {
  it("retains the existing artist preference and restores the album preference", () => {
    localStorage.setItem(
      "echo-song-sort:all",
      JSON.stringify({ field: "artist", direction: "asc" }),
    );
    const { result, unmount } = renderHook(() => useStoredSongSort("all"));
    expect(result.current[0]).toEqual({ field: "artist", direction: "asc" });

    act(() => result.current[1]({ field: "album", direction: "desc" }));
    expect(JSON.parse(localStorage.getItem("echo-song-sort:all") ?? "null")).toEqual({
      field: "album",
      direction: "desc",
    });
    unmount();
    const restored = renderHook(() => useStoredSongSort("all"));
    expect(restored.result.current[0]).toEqual({ field: "album", direction: "desc" });
    restored.unmount();
  });
});
