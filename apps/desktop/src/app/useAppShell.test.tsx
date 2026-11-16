import { act, renderHook } from "@testing-library/react";
import { describe, expect, it } from "vitest";

import { useAppShell } from "./useAppShell";

describe("useAppShell collection navigation", () => {
  it("records the source view and switches to the target directory", () => {
    const { result } = renderHook(() => useAppShell());
    act(() => result.current.selectView("playlist", "playlist-1"));

    act(() => result.current.openCollection({ kind: "album", albumKey: "abbey road" }));

    expect(result.current.libraryView).toBe("albums");
    expect(result.current.activePlaylistId).toBeNull();
    expect(result.current.collectionTarget).toEqual({
      kind: "album",
      albumKey: "abbey road",
    });
    expect(result.current.collectionSource).toEqual({
      view: "playlist",
      playlistId: "playlist-1",
    });
  });

  it("keeps the first source when targets are clicked in quick succession", () => {
    const { result } = renderHook(() => useAppShell());
    act(() => result.current.selectView("favorites"));

    act(() => result.current.openCollection({ kind: "artist", artistKey: "artist-a" }));
    act(() => result.current.openCollection({ kind: "album", albumKey: "album-b" }));

    expect(result.current.collectionTarget).toEqual({ kind: "album", albumKey: "album-b" });
    expect(result.current.collectionSource).toEqual({ view: "favorites", playlistId: null });
  });

  it("consumes only the target and keeps the source for the return", () => {
    const { result } = renderHook(() => useAppShell());
    act(() => result.current.selectView("recent"));
    act(() => result.current.openCollection({ kind: "artist", artistKey: "artist-a" }));

    act(() => result.current.consumeCollectionTarget());

    expect(result.current.collectionTarget).toBeNull();
    expect(result.current.collectionSource).toEqual({ view: "recent", playlistId: null });
  });

  it("selectView clears an unconsumed target and the source", () => {
    const { result } = renderHook(() => useAppShell());
    act(() => result.current.openCollection({ kind: "artist", artistKey: "artist-a" }));

    act(() => result.current.selectView("all"));

    expect(result.current.collectionTarget).toBeNull();
    expect(result.current.collectionSource).toBeNull();
  });

  it("returnToSource restores the recorded view and playlist", () => {
    const { result } = renderHook(() => useAppShell());
    act(() => result.current.selectView("playlist", "playlist-1"));
    act(() => result.current.openCollection({ kind: "artist", artistKey: "artist-a" }));
    act(() => result.current.consumeCollectionTarget());

    act(() => result.current.returnToSource());

    expect(result.current.libraryView).toBe("playlist");
    expect(result.current.activePlaylistId).toBe("playlist-1");
    expect(result.current.collectionSource).toBeNull();
  });

  it("entering a directory directly produces no source record", () => {
    const { result } = renderHook(() => useAppShell());

    act(() => result.current.selectView("artists"));

    expect(result.current.collectionTarget).toBeNull();
    expect(result.current.collectionSource).toBeNull();
  });
});
