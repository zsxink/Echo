/**
 * Issue #168 — the lyrics column must align against *settled* geometry.
 *
 * #85 made the immersive surface enter through a FLIP transition: for
 * `--motion-base` the cover is still scaled/translated towards the 常驻播放栏.
 * `getBoundingClientRect()` during that window reports the distorted box, so
 * the old alignment put the current line near the player bar and never
 * corrected itself (`transform` does not resize anything the `ResizeObserver`
 * watches). These cases stub the rects, `matchMedia` and the observer the same
 * way the other jsdom player tests do, and assert the contract the real
 * Chromium gate (A85) then verifies against actual layout:
 *
 *  - nothing is written while the entry/exit transform is running;
 *  - the current line lands on the record centre (wide) or the lyrics viewport
 *    centre (narrow / 歌词专注阅读) once geometry settles;
 *  - late lyrics, track changes, interrupted exits and returns from focus all
 *    realign, overwriting the previous scroll and edge padding;
 *  - reduced motion settles immediately, so the first frame aligns.
 */

import { act, render } from "@testing-library/react";
import type { ReactNode } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { LyricsColumn } from "./LyricsColumn";
import type { SongLyrics } from "./useLyrics";
import { IMMERSIVE_OPEN_MS, useImmersiveTransition } from "./useImmersiveTransition";

const originalGetBoundingClientRect = HTMLElement.prototype.getBoundingClientRect;
const originalGetComputedStyle = window.getComputedStyle;

/** Controllable rAF: the column's alignment frame must be flushable. */
let frames: Map<number, FrameRequestCallback>;
let nextFrame: number;
let wide: boolean;
let reduced: boolean;
let coverTop: number;

class StubObserver {
  readonly observe = vi.fn();
  readonly disconnect = vi.fn();
  constructor(readonly callback: ResizeObserverCallback) {
    observers.push(this);
  }
}

let observers: StubObserver[] = [];

function rect(top: number, height: number, width = 400): DOMRect {
  return {
    top,
    bottom: top + height,
    left: 0,
    right: width,
    width,
    height,
    x: 0,
    y: top,
    toJSON: () => ({}),
  } as DOMRect;
}

function makeLyrics(): SongLyrics {
  return {
    source: "embedded",
    timed: true,
    plainText: "",
    lines: Array.from({ length: 24 }, (_, idx) => ({
      seconds: idx * 10,
      text: `第 ${idx + 1} 行`,
    })),
  };
}

function flushFrame() {
  const pending = [...frames.entries()];
  frames.clear();
  act(() => {
    for (const [, callback] of pending) callback(performance.now());
  });
}

/** The surface wrapper whose `.playing-cover` is the wide-layout target. */
function Surface({ children }: { children: ReactNode }) {
  return (
    <div className="now-playing-popover" data-phase="open">
      <div className="playing-cover" />
      {children}
    </div>
  );
}

interface ColumnProps {
  readonly lyrics: SongLyrics | null;
  readonly currentPosition: number;
  readonly focusOpen?: boolean;
  readonly autoScroll?: boolean;
}

function column(props: ColumnProps, geometrySettled: boolean) {
  return (
    <Surface>
      <LyricsColumn
        lyrics={props.lyrics}
        focusOpen={props.focusOpen ?? false}
        autoScroll={props.autoScroll ?? true}
        currentPosition={props.currentPosition}
        geometrySettled={geometrySettled}
        onResume={() => {}}
        onToggleFocus={() => {}}
      />
    </Surface>
  );
}

/** Drives the real phase machine, as `ImmersivePlayer` does. */
function TransitionedColumn(props: ColumnProps & { open: boolean }) {
  const { settled } = useImmersiveTransition(props.open);
  return column(props, settled);
}

/** jsdom reports 0 for every box; the column needs a real viewport height. */
function mountViewport(): HTMLElement {
  const viewport = document.querySelector<HTMLElement>(".lyrics")!;
  Object.defineProperty(viewport, "clientHeight", { configurable: true, value: 500 });
  return viewport;
}

const track = () => document.querySelector<HTMLElement>(".lyrics-track");
const edge = (name: string) => track()!.style.getPropertyValue(name);

beforeEach(() => {
  observers = [];
  frames = new Map();
  nextFrame = 0;
  wide = true;
  reduced = false;
  coverTop = 150;
  vi.useFakeTimers();

  vi.stubGlobal("requestAnimationFrame", (callback: FrameRequestCallback) => {
    nextFrame += 1;
    frames.set(nextFrame, callback);
    return nextFrame;
  });
  vi.stubGlobal("cancelAnimationFrame", (id: number) => {
    frames.delete(id);
  });
  vi.stubGlobal("ResizeObserver", StubObserver);
  vi.stubGlobal("matchMedia", (query: string) => ({
    matches: query.includes("min-width: 981px")
      ? wide
      : query.includes("prefers-reduced-motion")
        ? reduced
        : false,
    media: query,
    onchange: null,
    addEventListener: () => {},
    removeEventListener: () => {},
    addListener: () => {},
    removeListener: () => {},
    dispatchEvent: () => false,
  }));
  vi.stubGlobal("getComputedStyle", (element: Element) =>
    element.classList.contains("lyrics")
      ? ({ paddingTop: "0px", paddingBottom: "0px" } as CSSStyleDeclaration)
      : originalGetComputedStyle.call(window, element),
  );

  // The surface is a 1200×800 popover at the origin, the record centre is 250,
  // and the lyrics viewport starts at 100 with a 500px height. Each line is
  // 100px apart; its document box moves with the viewport's scroll, exactly
  // like real layout, so repeated alignments converge on the same target.
  // `useLyricsStartOffset` moves the whole viewport with an inline `margin-top`,
  // so the stub reads that margin back: the record does not move with it.
  HTMLElement.prototype.getBoundingClientRect = function (this: HTMLElement) {
    const viewport = document.querySelector<HTMLElement>(".lyrics");
    const scroll = viewport?.scrollTop ?? 0;
    const offset = Number.parseFloat(viewport?.style.marginTop ?? "") || 0;
    if (this.classList.contains("playing-cover")) return rect(coverTop, 200);
    if (this.classList.contains("lyrics")) return rect(100 + offset, 500);
    if (this.classList.contains("now-playing-popover")) return rect(0, 800, 1200);
    if (this.classList.contains("lyrics-track")) return rect(100 + offset, 2400);
    if (this.classList.contains("lyric-line")) {
      const idx = Number(this.dataset.lineIdx);
      if (Number.isFinite(idx)) return rect(100 + offset + idx * 100 - scroll, 24);
    }
    return rect(0, 0);
  };
});

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
  HTMLElement.prototype.getBoundingClientRect = originalGetBoundingClientRect;
});

describe("LyricsColumn — settled geometry (issue #168)", () => {
  it("does not write scroll while opening and aligns the current line with the record centre after settle", () => {
    render(<TransitionedColumn open lyrics={makeLyrics()} currentPosition={65} />);
    const viewport = mountViewport();

    // The entry transform is running: no observer and no write. (The single
    // pending frame belongs to the phase machine's opening → open edge.)
    expect(viewport.scrollTop).toBe(0);
    expect(observers).toHaveLength(0);

    // The phase machine reaches `open` after two frames, but the CSS entry
    // transition still runs for IMMERSIVE_OPEN_MS.
    flushFrame();
    flushFrame();
    expect(viewport.scrollTop).toBe(0);
    expect(frames.size).toBe(0);
    expect(observers).toHaveLength(0);

    act(() => vi.advanceTimersByTime(IMMERSIVE_OPEN_MS));
    expect(frames.size).toBeGreaterThan(0);
    flushFrame();

    // Line 6 centre (712) − record centre (250) = 462.
    expect(viewport.scrollTop).toBe(462);
    expect(edge("--lyrics-edge-top")).toBe("138px");
    expect(edge("--lyrics-edge-bottom")).toBe("338px");
    expect(observers).toHaveLength(1);

    const current = document.querySelector<HTMLElement>('[data-line-idx="6"]')!;
    const line = current.getBoundingClientRect();
    expect(line.top + line.height / 2).toBe(250);
  });

  it("aligns when expanded while playback is paused and needs no later line change", () => {
    render(<TransitionedColumn open lyrics={makeLyrics()} currentPosition={65} />);
    const viewport = mountViewport();

    flushFrame();
    flushFrame();
    act(() => vi.advanceTimersByTime(IMMERSIVE_OPEN_MS));
    flushFrame();
    expect(viewport.scrollTop).toBe(462);

    // Paused: the current line never changes, so nothing else may be needed.
    act(() => vi.advanceTimersByTime(5_000));
    expect(frames.size).toBe(0);
    expect(viewport.scrollTop).toBe(462);
  });

  it("realigns when the start-offset margin lands after the entry settles", async () => {
    render(column({ lyrics: makeLyrics(), currentPosition: 65 }, true));
    const viewport = mountViewport();
    flushFrame();
    expect(viewport.scrollTop).toBe(462);

    // `useLyricsStartOffset` measures the metadata block and writes the column's
    // inline `margin-top` — on a cold start `fonts.ready` can land well after
    // the entry settle. The margin moves the viewport and every line but not
    // the record, and changes no box size the `ResizeObserver` watches, so only
    // the inline-style mutation can trigger the correction.
    await act(async () => {
      viewport.style.marginTop = "215px";
    });
    flushFrame();

    // Everything moved down 215px, so the line needs 215px more scroll.
    expect(viewport.scrollTop).toBe(677);
    const current = document.querySelector<HTMLElement>('[data-line-idx="6"]')!;
    const line = current.getBoundingClientRect();
    expect(line.top + line.height / 2).toBe(250);
  });

  it("realigns when a throttled entry transform finishes after the settle timer", () => {
    render(column({ lyrics: makeLyrics(), currentPosition: 65 }, true));
    const viewport = mountViewport();
    flushFrame();
    expect(viewport.scrollTop).toBe(462);

    // An occluded window can hold the compositor at the FLIP start while the
    // settle timer fires. When the transform finally ends, its `transitionend`
    // is the only trigger left — the ResizeObserver sees no size change.
    coverTop = 350;
    act(() => {
      document
        .querySelector<HTMLElement>(".playing-cover")!
        .dispatchEvent(new Event("transitionend", { bubbles: true }));
    });
    flushFrame();

    // The record centre moved from 250 to 450, so the column scrolls back 200px.
    expect(viewport.scrollTop).toBe(262);
    const current = document.querySelector<HTMLElement>('[data-line-idx="6"]')!;
    const line = current.getBoundingClientRect();
    expect(line.top + line.height / 2).toBe(450);
  });

  it("ignores lyrics that arrive during the entry and aligns them once geometry settles", () => {
    const { rerender } = render(<TransitionedColumn open lyrics={null} currentPosition={65} />);
    const viewport = mountViewport();
    flushFrame();
    flushFrame();

    // Lyrics arrive while the entry transition is still running.
    rerender(<TransitionedColumn open lyrics={makeLyrics()} currentPosition={65} />);
    expect(frames.size).toBe(0);
    expect(viewport.scrollTop).toBe(0);

    act(() => vi.advanceTimersByTime(IMMERSIVE_OPEN_MS - 1));
    expect(viewport.scrollTop).toBe(0);
    act(() => vi.advanceTimersByTime(1));
    flushFrame();
    expect(viewport.scrollTop).toBe(462);
  });

  it("uses the lyrics viewport centre in the narrow layout", () => {
    wide = false;
    render(column({ lyrics: makeLyrics(), currentPosition: 65 }, true));
    const viewport = mountViewport();
    flushFrame();

    // Line 6 centre (712) − viewport centre (350) = 362.
    expect(viewport.scrollTop).toBe(362);
    const current = document.querySelector<HTMLElement>('[data-line-idx="6"]')!;
    const line = current.getBoundingClientRect();
    expect(line.top + line.height / 2).toBe(350);
  });

  it("uses the lyrics viewport centre in 歌词专注阅读", () => {
    render(column({ lyrics: makeLyrics(), currentPosition: 65, focusOpen: true }, true));
    const viewport = mountViewport();
    flushFrame();

    expect(viewport.scrollTop).toBe(362);
  });

  it("realigns after a track change without keeping the old scroll", () => {
    const { rerender } = render(column({ lyrics: makeLyrics(), currentPosition: 65 }, true));
    const viewport = mountViewport();
    flushFrame();
    expect(viewport.scrollTop).toBe(462);

    rerender(column({ lyrics: makeLyrics(), currentPosition: 155 }, true));
    flushFrame();

    // Line 15 centre (1612) − record centre (250) = 1362.
    expect(viewport.scrollTop).toBe(1362);
    expect(edge("--lyrics-edge-top")).toBe("138px");
    const current = document.querySelector<HTMLElement>('[data-line-idx="15"]')!;
    const line = current.getBoundingClientRect();
    expect(line.top + line.height / 2).toBe(250);
  });

  it("realigns when returning from 歌词专注阅读", () => {
    const { rerender } = render(column({ lyrics: makeLyrics(), currentPosition: 65 }, true));
    const viewport = mountViewport();
    flushFrame();
    expect(viewport.scrollTop).toBe(462);
    expect(edge("--lyrics-edge-top")).toBe("138px");
    expect(edge("--lyrics-edge-bottom")).toBe("338px");

    rerender(column({ lyrics: makeLyrics(), currentPosition: 65, focusOpen: true }, true));
    flushFrame();
    expect(viewport.scrollTop).toBe(362);
    // Focus targets the viewport centre, so the edge padding is rewritten.
    expect(edge("--lyrics-edge-top")).toBe("238px");
    expect(edge("--lyrics-edge-bottom")).toBe("238px");

    rerender(column({ lyrics: makeLyrics(), currentPosition: 65 }, true));
    flushFrame();
    expect(viewport.scrollTop).toBe(462);
    expect(edge("--lyrics-edge-top")).toBe("138px");
    expect(edge("--lyrics-edge-bottom")).toBe("338px");
  });

  it("writes nothing while closing and realigns after an interrupted exit settles", () => {
    const { rerender } = render(
      <TransitionedColumn open lyrics={makeLyrics()} currentPosition={65} />,
    );
    const viewport = mountViewport();
    flushFrame();
    flushFrame();
    act(() => vi.advanceTimersByTime(IMMERSIVE_OPEN_MS));
    flushFrame();
    expect(viewport.scrollTop).toBe(462);

    // The surface stays mounted for the exit, but its geometry is no longer
    // stable; a line change during the exit must not move the column.
    rerender(<TransitionedColumn open={false} lyrics={makeLyrics()} currentPosition={155} />);
    expect(frames.size).toBe(0);
    act(() => vi.advanceTimersByTime(90));
    expect(viewport.scrollTop).toBe(462);
    expect(edge("--lyrics-edge-top")).toBe("138px");

    // Reopen mid-exit: `open` resumes immediately, yet the surface is still
    // transitioning back from the exit transform, so the wait starts again.
    rerender(<TransitionedColumn open lyrics={makeLyrics()} currentPosition={155} />);
    expect(frames.size).toBe(0);
    act(() => vi.advanceTimersByTime(IMMERSIVE_OPEN_MS - 1));
    expect(viewport.scrollTop).toBe(462);
    act(() => vi.advanceTimersByTime(1));
    flushFrame();
    expect(viewport.scrollTop).toBe(1362);
  });

  it("aligns on the first frame under prefers-reduced-motion", () => {
    reduced = true;
    render(<TransitionedColumn open lyrics={makeLyrics()} currentPosition={65} />);
    const viewport = mountViewport();

    // The hook settles synchronously, so the column already scheduled its one
    // alignment frame — no transition timer is left to wait for.
    expect(frames.size).toBeGreaterThan(0);
    flushFrame();
    expect(viewport.scrollTop).toBe(462);
    expect(vi.getTimerCount()).toBe(0);
  });
});
