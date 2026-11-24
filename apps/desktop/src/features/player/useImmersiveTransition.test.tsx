import { act, renderHook } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import {
  IMMERSIVE_EXIT_MS,
  IMMERSIVE_OPEN_MS,
  useImmersiveTransition,
} from "./useImmersiveTransition";

/** Controllable rAF: the hook's frames must be flushable one at a time. */
let frames: Map<number, FrameRequestCallback>;
let nextFrame: number;

function flushFrame() {
  const pending = [...frames.entries()];
  frames.clear();
  act(() => {
    for (const [, callback] of pending) callback(performance.now());
  });
}

function stubReducedMotion(matches: boolean) {
  vi.stubGlobal("matchMedia", (query: string) => ({
    matches: matches && query.includes("prefers-reduced-motion"),
    media: query,
    onchange: null,
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
    dispatchEvent: () => false,
  }));
}

describe("useImmersiveTransition", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    frames = new Map();
    nextFrame = 0;
    vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
      nextFrame += 1;
      frames.set(nextFrame, callback);
      return nextFrame;
    });
    vi.stubGlobal("cancelAnimationFrame", (id: number) => {
      frames.delete(id);
    });
  });

  afterEach(() => {
    vi.useRealTimers();
    vi.unstubAllGlobals();
  });

  const openToSettled = (rerender: (props: { open: boolean }) => void) => {
    act(() => rerender({ open: true }));
    expect(frames.size).toBeGreaterThan(0);
    flushFrame();
    flushFrame();
    // `open` is reached on the second frame; geometry is only stable after the
    // entry transition's full duration.
    act(() => vi.advanceTimersByTime(IMMERSIVE_OPEN_MS));
  };

  it("advances closed → opening → open on the second frame", () => {
    const { result, rerender } = renderHook(
      ({ open }: { open: boolean }) => useImmersiveTransition(open),
      { initialProps: { open: false } },
    );
    expect(result.current.phase).toBe("closed");

    act(() => rerender({ open: true }));
    expect(result.current.phase).toBe("opening");

    // First frame only paints the FLIP start state; the second advances, so the
    // transition has a painted origin to grow from.
    flushFrame();
    expect(result.current.phase).toBe("opening");
    flushFrame();
    expect(result.current.phase).toBe("open");
  });

  it("starts in opening when mounted already open", () => {
    const { result } = renderHook(() => useImmersiveTransition(true));
    expect(result.current.phase).toBe("opening");
  });

  it("closes through a delayed exit and lands on exactly one closed", () => {
    const { result, rerender } = renderHook(
      ({ open }: { open: boolean }) => useImmersiveTransition(open),
      { initialProps: { open: false } },
    );
    openToSettled(rerender);
    expect(result.current.phase).toBe("open");

    act(() => rerender({ open: false }));
    expect(result.current.phase).toBe("closing");

    act(() => vi.advanceTimersByTime(IMMERSIVE_EXIT_MS - 1));
    expect(result.current.phase).toBe("closing");
    act(() => vi.advanceTimersByTime(1));
    expect(result.current.phase).toBe("closed");

    // Nothing is left scheduled that could produce a second closed.
    act(() => vi.advanceTimersByTime(IMMERSIVE_EXIT_MS * 3));
    expect(result.current.phase).toBe("closed");
  });

  it("converges from opening → closing when the user closes mid-entry", () => {
    const { result, rerender } = renderHook(
      ({ open }: { open: boolean }) => useImmersiveTransition(open),
      { initialProps: { open: false } },
    );
    act(() => rerender({ open: true }));
    expect(result.current.phase).toBe("opening");
    flushFrame(); // the advance to open is now queued on the second frame

    act(() => rerender({ open: false }));
    expect(result.current.phase).toBe("closing");

    // The queued open must not win the race.
    flushFrame();
    expect(result.current.phase).toBe("closing");

    act(() => vi.advanceTimersByTime(IMMERSIVE_EXIT_MS));
    expect(result.current.phase).toBe("closed");
  });

  it("resumes straight to open when reopened mid-exit", () => {
    const { result, rerender } = renderHook(
      ({ open }: { open: boolean }) => useImmersiveTransition(open),
      { initialProps: { open: false } },
    );
    openToSettled(rerender);
    act(() => rerender({ open: false }));
    expect(result.current.phase).toBe("closing");

    act(() => rerender({ open: true }));
    expect(result.current.phase).toBe("open");

    // The exit timer was cleared: the surface must not close underneath.
    act(() => vi.advanceTimersByTime(IMMERSIVE_EXIT_MS * 3));
    expect(result.current.phase).toBe("open");
  });

  it("completes immediately under prefers-reduced-motion", () => {
    stubReducedMotion(true);
    const { result, rerender } = renderHook(
      ({ open }: { open: boolean }) => useImmersiveTransition(open),
      { initialProps: { open: false } },
    );

    act(() => rerender({ open: true }));
    expect(result.current.phase).toBe("open");
    expect(frames.size).toBe(0);

    act(() => rerender({ open: false }));
    expect(result.current.phase).toBe("closed");
    // No delayed unmount: nothing is left on the timer queue.
    expect(vi.getTimerCount()).toBe(0);
  });

  it("has no delayed unmount after a full open/close", () => {
    const { result, rerender } = renderHook(
      ({ open }: { open: boolean }) => useImmersiveTransition(open),
      { initialProps: { open: false } },
    );
    openToSettled(rerender);
    expect(result.current.phase).toBe("open");
    act(() => rerender({ open: false }));
    act(() => vi.advanceTimersByTime(IMMERSIVE_EXIT_MS));
    expect(result.current.phase).toBe("closed");
    expect(vi.getTimerCount()).toBe(0);
  });

  it("uses the documented entry/exit durations", () => {
    expect(IMMERSIVE_OPEN_MS).toBe(220);
    expect(IMMERSIVE_EXIT_MS).toBe(180);
  });

  it("settles only after the entry transition duration", () => {
    const { result, rerender } = renderHook(
      ({ open }: { open: boolean }) => useImmersiveTransition(open),
      { initialProps: { open: false } },
    );
    expect(result.current.settled).toBe(false);

    act(() => rerender({ open: true }));
    expect(result.current.settled).toBe(false);
    flushFrame();
    flushFrame();
    expect(result.current.phase).toBe("open");
    expect(result.current.settled).toBe(false);

    act(() => vi.advanceTimersByTime(IMMERSIVE_OPEN_MS - 1));
    expect(result.current.settled).toBe(false);
    act(() => vi.advanceTimersByTime(1));
    expect(result.current.settled).toBe(true);
  });

  it("settles immediately under prefers-reduced-motion", () => {
    stubReducedMotion(true);
    const { result, rerender } = renderHook(
      ({ open }: { open: boolean }) => useImmersiveTransition(open),
      { initialProps: { open: false } },
    );

    act(() => rerender({ open: true }));
    expect(result.current.phase).toBe("open");
    expect(result.current.settled).toBe(true);
  });

  it("starts settled when mounted open under prefers-reduced-motion", () => {
    stubReducedMotion(true);
    const { result } = renderHook(() => useImmersiveTransition(true));
    expect(result.current.phase).toBe("open");
    expect(result.current.settled).toBe(true);
  });

  it("un-settles on exit and re-waits after an interrupted exit", () => {
    const { result, rerender } = renderHook(
      ({ open }: { open: boolean }) => useImmersiveTransition(open),
      { initialProps: { open: false } },
    );
    openToSettled(rerender);
    expect(result.current.settled).toBe(true);

    act(() => rerender({ open: false }));
    expect(result.current.phase).toBe("closing");
    expect(result.current.settled).toBe(false);

    // The surface never unmounted, but it is transitioning back from the exit
    // transform, so the new `open` still has to wait a full entry duration.
    act(() => rerender({ open: true }));
    expect(result.current.phase).toBe("open");
    expect(result.current.settled).toBe(false);

    act(() => vi.advanceTimersByTime(IMMERSIVE_OPEN_MS - 1));
    expect(result.current.settled).toBe(false);
    act(() => vi.advanceTimersByTime(1));
    expect(result.current.settled).toBe(true);
  });

  it("never settles when the entry is interrupted by a close", () => {
    const { result, rerender } = renderHook(
      ({ open }: { open: boolean }) => useImmersiveTransition(open),
      { initialProps: { open: false } },
    );
    act(() => rerender({ open: true }));
    expect(result.current.settled).toBe(false);
    flushFrame();

    act(() => rerender({ open: false }));
    expect(result.current.phase).toBe("closing");
    expect(result.current.settled).toBe(false);

    act(() => vi.advanceTimersByTime(IMMERSIVE_EXIT_MS));
    expect(result.current.phase).toBe("closed");
    expect(result.current.settled).toBe(false);
    expect(vi.getTimerCount()).toBe(0);
  });
});
