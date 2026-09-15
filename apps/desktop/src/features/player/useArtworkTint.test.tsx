import { act, renderHook } from "@testing-library/react";
import { afterEach, describe, expect, it, vi } from "vitest";
import { clearArtworkTint, useArtworkTint } from "./useArtworkTint";
import { loadArtworkPalette, type ArtworkPalette } from "./artworkPalette";

vi.mock("./artworkPalette", () => ({ loadArtworkPalette: vi.fn() }));
const palette = { tint: "#aaaaaa", background: "#111111", glow: "#222222" };
afterEach(() => {
  clearArtworkTint();
  vi.clearAllMocks();
  vi.useRealTimers();
});

describe("immersive artwork tint", () => {
  it("ignores an old song result after a fast track switch", async () => {
    let first!: (p: ArtworkPalette) => void;
    vi.mocked(loadArtworkPalette)
      .mockImplementationOnce(
        () =>
          new Promise((resolve) => {
            first = resolve;
          }),
      )
      .mockResolvedValueOnce(palette);
    const { rerender } = renderHook(({ key }) => useArtworkTint(key, key, false), {
      initialProps: { key: "a" },
    });
    rerender({ key: "b" });
    await act(async () => {});
    expect(document.body.style.getPropertyValue("--player-background")).toBe("#111111");
    await act(async () => first({ ...palette, background: "#ffffff" }));
    expect(document.body.style.getPropertyValue("--player-background")).toBe("#111111");
  });
  it("keeps color across track remounts, then falls back if metadata never arrives", async () => {
    vi.useFakeTimers();
    vi.mocked(loadArtworkPalette).mockResolvedValue(palette);
    const { unmount } = renderHook(() => useArtworkTint("a", "a", false));
    await act(async () => {});
    unmount();
    renderHook(() => useArtworkTint(null, null, true));
    expect(document.body.style.getPropertyValue("--player-tint")).toBe(palette.tint);
    act(() => vi.advanceTimersByTime(3000));
    expect(document.body.style.getPropertyValue("--player-tint")).toBe("");
  });
  it("clears stale artwork for missing and failed covers", async () => {
    document.body.style.setProperty("--player-tint", "red");
    const { rerender } = renderHook(
      ({ key }: { key: string | null }) => useArtworkTint(key, key, false),
      { initialProps: { key: null as string | null } },
    );
    expect(document.body.style.getPropertyValue("--player-tint")).toBe("");
    document.body.style.setProperty("--player-tint", "red");
    vi.mocked(loadArtworkPalette).mockResolvedValue(null);
    rerender({ key: "failed" });
    await act(async () => {});
    expect(document.body.style.getPropertyValue("--player-tint")).toBe("");
  });
});
