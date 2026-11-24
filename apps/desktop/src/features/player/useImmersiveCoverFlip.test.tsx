import { render } from "@testing-library/react";
import { useRef } from "react";
import { afterEach, describe, expect, it, vi } from "vitest";

import { computeCoverFlip, useImmersiveCoverFlip } from "./useImmersiveCoverFlip";
import type { ImmersivePhase } from "./useImmersiveTransition";

const rect = (left: number, top: number, width: number, height: number) => ({
  left,
  top,
  width,
  height,
  right: left + width,
  bottom: top + height,
  x: left,
  y: top,
  toJSON: () => ({}),
});

describe("computeCoverFlip", () => {
  it("maps the source centre onto the target centre and scales by width", () => {
    const flip = computeCoverFlip(
      rect(10, 20, 48, 48),
      rect(400, 100, 200, 200),
      rect(0, 0, 1280, 800),
    );
    expect(flip).toEqual({
      x: Math.round(34 - 500),
      y: Math.round(44 - 200),
      scale: 0.24,
      originX: 34,
      originY: 44,
    });
  });

  it("reports the origin in the surface's coordinate space, not the viewport's", () => {
    const flip = computeCoverFlip(
      rect(110, 220, 48, 48),
      rect(400, 100, 200, 200),
      rect(100, 200, 1280, 800),
    );
    expect(flip?.originX).toBe(34);
    expect(flip?.originY).toBe(44);
  });

  it("clamps the growth ratio at both ends", () => {
    expect(
      computeCoverFlip(rect(0, 0, 1000, 1000), rect(0, 0, 200, 200), rect(0, 0, 10, 10))?.scale,
    ).toBe(1.6);
    expect(
      computeCoverFlip(rect(0, 0, 10, 10), rect(0, 0, 200, 200), rect(0, 0, 10, 10))?.scale,
    ).toBe(0.2);
  });

  it("degrades when either rect is missing or has no box", () => {
    const target = rect(400, 100, 200, 200);
    const surface = rect(0, 0, 1280, 800);
    expect(computeCoverFlip(null, target, surface)).toBeNull();
    expect(computeCoverFlip(rect(0, 0, 0, 0), target, surface)).toBeNull();
    expect(computeCoverFlip(rect(0, 0, 48, 48), null, surface)).toBeNull();
    expect(computeCoverFlip(rect(0, 0, 48, 48), rect(0, 0, 0, 200), surface)).toBeNull();
    expect(computeCoverFlip(rect(0, 0, 48, 48), target, null)).toBeNull();
  });
});

function Harness({ phase, withSource = true }: { phase: ImmersivePhase; withSource?: boolean }) {
  const ref = useRef<HTMLDivElement>(null);
  useImmersiveCoverFlip(ref, phase);
  return (
    <div>
      {withSource ? (
        <div className="playerbar">
          <div className="mini-cover" />
        </div>
      ) : null}
      <div className="now-playing-popover" ref={ref} data-phase={phase}>
        <div className="playing-cover" />
      </div>
    </div>
  );
}

describe("useImmersiveCoverFlip", () => {
  afterEach(() => {
    vi.restoreAllMocks();
    vi.unstubAllGlobals();
  });

  function stubLayout() {
    vi.spyOn(HTMLElement.prototype, "getBoundingClientRect").mockImplementation(function (
      this: HTMLElement,
    ) {
      if (this.classList.contains("mini-cover")) return rect(10, 20, 48, 48) as DOMRect;
      if (this.classList.contains("playing-cover")) return rect(400, 100, 200, 200) as DOMRect;
      if (this.classList.contains("now-playing-popover")) return rect(0, 0, 1280, 800) as DOMRect;
      return rect(0, 0, 0, 0) as DOMRect;
    });
  }

  it("publishes entry offsets and the surface origin once, never per frame", () => {
    stubLayout();
    const { container } = render(<Harness phase="opening" />);
    const shell = container.querySelector<HTMLElement>(".now-playing-popover")!;
    expect(shell.dataset.flip).toBe("cover");
    expect(shell.style.getPropertyValue("--flip-x")).toBe("-466px");
    expect(shell.style.getPropertyValue("--flip-y")).toBe("-156px");
    expect(shell.style.getPropertyValue("--flip-scale")).toBe("0.24");
    expect(shell.style.getPropertyValue("--transition-origin-x")).toBe("34px");
    expect(shell.style.getPropertyValue("--transition-origin-y")).toBe("44px");
    // The measuring state must never survive the pass.
    expect(shell.hasAttribute("data-measuring")).toBe(false);
  });

  it("writes the exit variable family when collapsing", () => {
    stubLayout();
    const { container, rerender } = render(<Harness phase="closed" />);
    const shell = container.querySelector<HTMLElement>(".now-playing-popover")!;
    expect(shell.dataset.flip).toBeUndefined();
    rerender(<Harness phase="closing" />);
    expect(shell.dataset.flip).toBe("cover");
    expect(shell.style.getPropertyValue("--flip-exit-x")).toBe("-466px");
    expect(shell.style.getPropertyValue("--flip-exit-y")).toBe("-156px");
    expect(shell.style.getPropertyValue("--flip-exit-scale")).toBe("0.24");
  });

  it("marks a missing source as degraded and leaves the fade alone", () => {
    stubLayout();
    const { container } = render(<Harness phase="opening" withSource={false} />);
    const shell = container.querySelector<HTMLElement>(".now-playing-popover")!;
    expect(shell.dataset.flip).toBe("none");
    expect(shell.style.getPropertyValue("--flip-x")).toBe("0px");
    expect(shell.style.getPropertyValue("--flip-scale")).toBe("1");
  });

  it("skips measurement entirely under reduced motion", () => {
    stubLayout();
    vi.stubGlobal("matchMedia", () => ({
      matches: true,
      media: "(prefers-reduced-motion: reduce)",
      onchange: null,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => false,
    }));
    const { container } = render(<Harness phase="opening" />);
    const shell = container.querySelector<HTMLElement>(".now-playing-popover")!;
    expect(shell.dataset.flip).toBe("none");
    expect(shell.style.getPropertyValue("--flip-x")).toBe("");
  });
});
