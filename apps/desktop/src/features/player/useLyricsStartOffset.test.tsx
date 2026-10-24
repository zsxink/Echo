/**
 * The lyrics offset must be *observed*, not signature-triggered (issue #75), and
 * an observation that measured nothing must never be written out as a result
 * (issue #90).
 *
 * jsdom has no layout, so these cases stub the three inputs the hook reads
 * (`matchMedia`, the two boxes' `getBoundingClientRect`, the media query's own
 * `change`) and the one channel it subscribes to beyond `resize`
 * (`ResizeObserver`), then assert the measurement contract: the initial layout-
 * effect pass, the ResizeObserver re-measure on a height change the info
 * signature cannot see, the resize re-measure, the per-signature re-observe, and
 * the three ways a pass can produce no measurement at all — an unrendered
 * metadata block, geometry that is not the shared grid area, and 歌词专注阅读,
 * where the column owns its own margin — and the pass that runs while a
 * previous offset is still laid out (WebView2's stale layout read). Real
 * geometry stays proven by the e2e A9 gate (`run-e2e.mjs`), which drives the
 * same contract in Chromium.
 */

import { fireEvent, render } from "@testing-library/react";
import { useRef } from "react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { useLyricsStartOffset } from "./useLyricsStartOffset";

/** Captured before the beforeEach patch replaces it, restored per test. */
const originalGetBoundingClientRect = HTMLElement.prototype.getBoundingClientRect;

class StubObserver {
  readonly observe = vi.fn();
  readonly disconnect = vi.fn();
  constructor(readonly callback: ResizeObserverCallback) {
    observers.push(this);
  }
}

/** One observer per effect run; tests fire `callback` to simulate a resize. */
const observers: StubObserver[] = [];

/** Every `matchMedia` subscription the live effects registered. */
const queryListeners: (() => void)[] = [];

interface Geometry {
  /** `display: none` answers with an all-zero rect; the hook must not read it. */
  readonly collapsed?: boolean;
  readonly top?: number;
  readonly bottom?: number;
  /**
   * The margin the *layout* still reflects when it lags the inline style, as
   * WebView2's does for a frame or two after a write. Defaults to the inline
   * margin, i.e. a layout that is perfectly up to date.
   */
  readonly staleMargin?: number;
}

function rectOf(element: HTMLElement, geometry: Geometry): DOMRect {
  if (geometry.collapsed) {
    return {
      top: 0,
      bottom: 0,
      left: 0,
      right: 0,
      width: 0,
      height: 0,
      x: 0,
      y: 0,
      toJSON: () => ({}),
    } as DOMRect;
  }
  // The shared grid area starts at 100, and the lyrics column moves by whatever
  // margin is laid out — the only layout effect the hook depends on.
  const margin = geometry.staleMargin ?? (parseFloat(element.style.marginTop) || 0);
  const top = (geometry.top ?? 100) + margin;
  const bottom = geometry.bottom ?? 700;
  return {
    top,
    bottom,
    left: 0,
    right: 400,
    width: 400,
    height: bottom - top,
    x: 0,
    y: top,
    toJSON: () => ({}),
  } as DOMRect;
}

function Host({
  signature,
  focus,
  info,
  lyrics,
}: {
  signature: string;
  focus: boolean;
  info: Geometry;
  lyrics: Geometry;
}) {
  const ref = useRef<HTMLDivElement>(null);
  useLyricsStartOffset(ref, signature, focus);
  return (
    <div ref={ref}>
      <div
        className="now-playing-info"
        data-collapsed={info.collapsed ? "1" : undefined}
        data-top={info.top}
        data-bottom={info.bottom}
      />
      <div
        className="lyrics"
        data-collapsed={lyrics.collapsed ? "1" : undefined}
        data-top={lyrics.top}
        data-bottom={lyrics.bottom}
        data-stale-margin={lyrics.staleMargin}
      />
    </div>
  );
}

/** The wide layout with a metadata block that ends at `infoBottom`. */
const wide = (infoBottom: number) => ({
  signature: "a",
  focus: false,
  info: { bottom: infoBottom },
  lyrics: {},
});

function offsetOf(container: HTMLElement): string {
  // The offset is written as a plain inline margin (see the hook's header for
  // why it no longer routes through the `--lyrics-start-offset` property).
  return container.querySelector<HTMLElement>(".lyrics")!.style.marginTop;
}

function advanceFrames(count: number): Promise<void> {
  return new Promise((resolve) => {
    let left = count;
    const step = () => {
      left -= 1;
      if (left <= 0) resolve();
      else requestAnimationFrame(step);
    };
    requestAnimationFrame(step);
  });
}

/** Drive the observer the current effect registered (a metadata resize). */
function fireObserver(container: HTMLElement) {
  const last = observers[observers.length - 1];
  last.callback([], {} as unknown as ResizeObserver);
  return container;
}

beforeEach(() => {
  observers.length = 0;
  queryListeners.length = 0;
  vi.stubGlobal(
    "matchMedia",
    vi.fn(() => ({
      matches: true,
      addEventListener: (_: string, listener: () => void) => {
        queryListeners.push(listener);
      },
      removeEventListener: (_: string, listener: () => void) => {
        const at = queryListeners.indexOf(listener);
        if (at >= 0) queryListeners.splice(at, 1);
      },
    })),
  );
  vi.stubGlobal("ResizeObserver", StubObserver);
  // The hook reads geometry off the live elements; jsdom's rects are all zero,
  // so each element answers from its own `data-*` box.
  HTMLElement.prototype.getBoundingClientRect = function (this: HTMLElement) {
    const isInfo = this.classList.contains("now-playing-info");
    const geometry: Geometry = {
      collapsed: this.dataset.collapsed === "1",
      top: this.dataset.top ? Number(this.dataset.top) : undefined,
      bottom: this.dataset.bottom ? Number(this.dataset.bottom) : isInfo ? 200 : 700,
      staleMargin: this.dataset.staleMargin ? Number(this.dataset.staleMargin) : undefined,
    };
    return rectOf(this, geometry);
  };
});

afterEach(() => {
  vi.unstubAllGlobals();
  HTMLElement.prototype.getBoundingClientRect = originalGetBoundingClientRect;
});

describe("useLyricsStartOffset — 持续对齐 (issue #75)", () => {
  it("measures the offset in the layout effect", () => {
    const { container } = render(<Host {...wide(200)} />);
    // 200 (info bottom) − 100 (lyrics top) + 20 gap.
    expect(offsetOf(container)).toBe("120px");
  });

  it("re-measures when the info block resizes (ResizeObserver)", () => {
    // The whole point of issue #75: the height changes *after* the initial
    // measurement and the title/artist/album/duration signature does not.
    const { container, rerender } = render(<Host {...wide(200)} />);
    rerender(<Host {...wide(260)} />);
    expect(observers).toHaveLength(1);
    expect(observers[0].observe).toHaveBeenCalledWith(
      container.querySelector(".now-playing-info"),
      { box: "border-box" },
    );
    fireObserver(container);
    expect(offsetOf(container)).toBe("180px");
  });

  it("re-measures on window resize", () => {
    const { container, rerender } = render(<Host {...wide(200)} />);
    rerender(<Host {...wide(240)} />);
    fireEvent(window, new Event("resize"));
    expect(offsetOf(container)).toBe("160px");
  });

  it("re-measures when the wide breakpoint itself flips", () => {
    // A classic scrollbar appearing or going crosses the breakpoint with no
    // `resize` behind it; the media query's own change event is the only notice.
    const { container, rerender } = render(<Host {...wide(200)} />);
    rerender(<Host {...wide(260)} />);
    expect(queryListeners.length).toBeGreaterThan(0);
    queryListeners[queryListeners.length - 1]();
    expect(offsetOf(container)).toBe("180px");
  });

  it("re-observes when the signature changes", () => {
    const { rerender } = render(<Host {...wide(200)} />);
    expect(observers).toHaveLength(1);
    rerender(<Host signature="b" focus={false} info={{ bottom: 200 }} lyrics={{}} />);
    expect(observers).toHaveLength(2);
    expect(observers[0].disconnect).toHaveBeenCalled();
  });

  it("does not write an offset below the wide breakpoint", () => {
    vi.stubGlobal(
      "matchMedia",
      vi.fn(() => ({ matches: false, addEventListener: () => {}, removeEventListener: () => {} })),
    );
    const { container } = render(<Host {...wide(200)} />);
    expect(offsetOf(container)).toBe("");
  });
});

describe("useLyricsStartOffset — 空测量不得成为结果 (issue #90)", () => {
  it("keeps the last good offset while the metadata block has no box", async () => {
    const { container, rerender } = render(<Host {...wide(200)} />);
    expect(offsetOf(container)).toBe("120px");

    // 歌词专注阅读 hides `.now-playing-info`: an all-zero rect whose bottom sits
    // far above the lyrics' top. Clamped, that reads as an offset of 0/20px —
    // exactly the margin that paints the first lyric lines through the title.
    rerender(
      <Host signature="a" focus={false} info={{ collapsed: true, bottom: 300 }} lyrics={{}} />,
    );
    fireObserver(container);
    expect(offsetOf(container)).toBe("120px");

    // And the rejected pass asks again: the metadata block comes back with a
    // height the rejected measurement never saw.
    rerender(<Host {...wide(300)} />);
    await advanceFrames(4);
    expect(offsetOf(container)).toBe("220px");
  });

  it("rejects geometry that is not the shared grid area", () => {
    const { container, rerender } = render(<Host {...wide(200)} />);
    expect(offsetOf(container)).toBe("120px");

    // The metadata ending *above* the lyrics column with room to spare is the
    // stacked layout: the two boxes share no area, so their distance is not an
    // offset. The old clamp turned it into `0px` and wrote that out.
    rerender(
      <Host
        signature="a"
        focus={false}
        info={{ top: 100, bottom: 140 }}
        lyrics={{ top: 200, bottom: 700 }}
      />,
    );
    fireObserver(container);
    expect(offsetOf(container)).toBe("120px");
  });

  it("stands down for 歌词专注阅读 and re-measures on the way back", () => {
    const { container, rerender } = render(<Host {...wide(200)} />);
    expect(offsetOf(container)).toBe("120px");

    // The focused column takes the whole surface and sets `margin: 0 auto`
    // itself; an inline margin here beats that rule, so it has to come off.
    rerender(<Host signature="a" focus info={{ bottom: 200 }} lyrics={{}} />);
    expect(offsetOf(container)).toBe("");

    rerender(<Host {...wide(260)} />);
    expect(offsetOf(container)).toBe("180px");
  });
});

describe("useLyricsStartOffset — 测量不得基于被偏移过的位置 (issue #90)", () => {
  it("measures the metadata's height while the previous offset is still laid out", () => {
    const { container, rerender } = render(<Host {...wide(169)} />);
    // 169 (info bottom) − 100 (shared-area top) + 20 gap.
    expect(offsetOf(container)).toBe("89px");

    // The metadata grows by 48px (the A9 "info block grew" step) while WebView2
    // has not laid out the next margin yet: the lyrics box still answers from
    // `areaTop + 89`. Clearing and re-reading in that state measures
    // 217 − 189 + 20 = 48px and writes it — the lyrics run through the title.
    rerender(
      <Host signature="a" focus={false} info={{ bottom: 217 }} lyrics={{ staleMargin: 89 }} />,
    );
    fireObserver(container);
    // The metadata's own 117px height plus the gap, whatever the stale read says.
    expect(offsetOf(container)).toBe("137px");
  });
});
