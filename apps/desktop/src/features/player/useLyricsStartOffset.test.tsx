/**
 * fix-lyrics-start-offset-remeasure — the offset must be *observed*, not
 * signature-triggered (issue #75).
 *
 * jsdom has no layout, so these cases stub the three inputs the hook reads
 * (`matchMedia`, the two boxes' `getBoundingClientRect`) and the one channel it
 * subscribes to beyond `resize` (`ResizeObserver`), then assert the measurement
 * contract: the initial layout-effect pass, the ResizeObserver re-measure on a
 * height change the info signature cannot see, the resize re-measure, and the
 * per-signature re-observe. Real geometry stays proven by the e2e A9 gate
 * (`run-e2e.mjs`), which drives the same contract in Chromium.
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

/** info: top 100 → bottom `infoBottom`; lyrics: top 100 (the shared grid area). */
function Host({ signature, infoBottom }: { signature: string; infoBottom: number }) {
  const ref = useRef<HTMLDivElement>(null);
  useLyricsStartOffset(ref, signature);
  return (
    <div ref={ref}>
      <div className="now-playing-info" data-bottom={infoBottom} />
      <div className="lyrics" />
    </div>
  );
}

function offsetOf(container: HTMLElement): string {
  return container
    .querySelector<HTMLElement>(".lyrics")!
    .style.getPropertyValue("--lyrics-start-offset");
}

beforeEach(() => {
  observers.length = 0;
  vi.stubGlobal(
    "matchMedia",
    vi.fn(() => ({ matches: true, addListener: () => {}, removeListener: () => {} })),
  );
  vi.stubGlobal("ResizeObserver", StubObserver);
  // The hook reads geometry off the live elements; jsdom's rects are all zero,
  // so every element answers top 100 and `.now-playing-info` bottoms out at
  // its `data-bottom` prop.
  HTMLElement.prototype.getBoundingClientRect = function (this: HTMLElement) {
    const bottom = Number(this.dataset.bottom ?? 100);
    return {
      top: 100,
      bottom,
      left: 0,
      right: 400,
      width: 400,
      height: bottom - 100,
      x: 0,
      y: 100,
      toJSON: () => ({}),
    } as DOMRect;
  };
});

afterEach(() => {
  vi.unstubAllGlobals();
  HTMLElement.prototype.getBoundingClientRect = originalGetBoundingClientRect;
});

describe("useLyricsStartOffset — 持续对齐 (fix-lyrics-start-offset-remeasure)", () => {
  it("measures the offset in the layout effect", () => {
    const { container } = render(<Host signature="a" infoBottom={200} />);
    // 200 (info bottom) − 100 (lyrics top) + 20 gap.
    expect(offsetOf(container)).toBe("120px");
  });

  it("re-measures when the info block resizes (ResizeObserver)", () => {
    // The whole point of issue #75: the height changes *after* the initial
    // measurement and the title/artist/album/duration signature does not.
    const { container, rerender } = render(<Host signature="a" infoBottom={200} />);
    rerender(<Host signature="a" infoBottom={260} />);
    expect(observers).toHaveLength(1);
    expect(observers[0].observe).toHaveBeenCalledWith(
      container.querySelector(".now-playing-info"),
      { box: "border-box" },
    );
    observers[0].callback([], {} as unknown as ResizeObserver);
    expect(offsetOf(container)).toBe("180px");
  });

  it("re-measures on window resize", () => {
    const { container, rerender } = render(<Host signature="a" infoBottom={200} />);
    rerender(<Host signature="a" infoBottom={240} />);
    fireEvent(window, new Event("resize"));
    expect(offsetOf(container)).toBe("160px");
  });

  it("re-observes when the signature changes", () => {
    const { rerender } = render(<Host signature="a" infoBottom={200} />);
    expect(observers).toHaveLength(1);
    rerender(<Host signature="b" infoBottom={200} />);
    expect(observers).toHaveLength(2);
    expect(observers[0].disconnect).toHaveBeenCalled();
  });

  it("does not write an offset below the wide breakpoint", () => {
    vi.stubGlobal(
      "matchMedia",
      vi.fn(() => ({ matches: false, addListener: () => {}, removeListener: () => {} })),
    );
    const { container } = render(<Host signature="a" infoBottom={200} />);
    expect(offsetOf(container)).toBe("");
  });
});
