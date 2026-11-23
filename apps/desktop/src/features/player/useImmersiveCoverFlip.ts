import { useLayoutEffect, useRef, type RefObject } from "react";

import type { ImmersivePhase } from "./useImmersiveTransition";
import { prefersReducedMotion } from "./useImmersiveTransition";

/**
 * One measured rectangle. `DOMRect` is structurally compatible, so tests can
 * pass a plain object with the same four numbers.
 */
export type FlipRect = {
  readonly left: number;
  readonly top: number;
  readonly width: number;
  readonly height: number;
};

export type CoverFlip = {
  readonly x: number;
  readonly y: number;
  readonly scale: number;
  /** Source centre relative to the surface's own top-left (for `transform-origin`). */
  readonly originX: number;
  readonly originY: number;
};

/** The prototype clamps the growth so a tiny or huge source cannot explode it. */
const MIN_SCALE = 0.2;
const MAX_SCALE = 1.6;

function measurable(rect: FlipRect | null | undefined): rect is FlipRect {
  return !!rect && rect.width > 0 && rect.height > 0;
}

/**
 * FLIP geometry from the 常驻播放栏's `.mini-cover` (source) to the immersive
 * `.playing-cover` (target). Pure, so the centre/scale/degenerate arithmetic is
 * unit-testable without a layout engine.
 *
 * `transform-origin: center` (player.css) means the scale is around the target's
 * own centre, so the translation is simply the centre-to-centre delta.
 * `surface` only shifts the reported origin into the surface's coordinate
 * space; it does not take part in the transform.
 */
export function computeCoverFlip(
  source: FlipRect | null | undefined,
  target: FlipRect | null | undefined,
  surface: FlipRect | null | undefined,
): CoverFlip | null {
  if (!measurable(source) || !measurable(target) || !measurable(surface)) return null;
  const scale = Math.max(MIN_SCALE, Math.min(MAX_SCALE, source.width / target.width));
  const sourceX = source.left + source.width / 2;
  const sourceY = source.top + source.height / 2;
  const targetX = target.left + target.width / 2;
  const targetY = target.top + target.height / 2;
  return {
    x: Math.round(sourceX - targetX),
    y: Math.round(sourceY - targetY),
    scale: Math.round(scale * 1000) / 1000,
    originX: Math.round(sourceX - surface.left),
    originY: Math.round(sourceY - surface.top),
  };
}

/**
 * Measure the two covers once per transition and publish the FLIP variables.
 *
 * Runs in a layout effect, so no intermediate state is painted: the surface
 * mounts with `data-phase="opening"` (entry transform applied), the hook reads
 * the *settled* geometry under a one-shot `data-measuring` state (which removes
 * the phase transform from the reading), writes `--flip-*`, and only then does
 * the machine advance to `open` on the next frame. Exit is the mirror image,
 * writing `--flip-exit-*`.
 *
 * A source that cannot be measured (no current item, a hidden control, a
 * layout switch) publishes `data-flip="none"` and writes no offsets — the
 * surface keeps its plain fade instead of jumping.
 */
export function useImmersiveCoverFlip(
  shellRef: RefObject<HTMLDivElement | null>,
  phase: ImmersivePhase,
): { applied: "cover" | "none" | null } {
  const appliedRef = useRef<"cover" | "none" | null>(null);

  useLayoutEffect(() => {
    if (phase !== "opening" && phase !== "closing") return;
    const shell = shellRef.current;
    if (!shell) return;

    const reset = (marker: "cover" | "none") => {
      appliedRef.current = marker;
      shell.dataset.flip = marker;
    };

    if (prefersReducedMotion()) {
      reset("none");
      return;
    }

    const cover = shell.querySelector<HTMLElement>(".playing-cover");
    if (!cover) {
      reset("none");
      return;
    }

    // Read the settled layout: the phase transform must not distort the boxes
    // the offsets are derived from. `data-measuring` is a no-transition state,
    // and because this is a layout effect nothing between setting and removing
    // it reaches the screen.
    shell.setAttribute("data-measuring", "");
    const source = document.querySelector<HTMLElement>(".playerbar .mini-cover");
    const sourceRect = source?.getBoundingClientRect() ?? null;
    const targetRect = cover.getBoundingClientRect();
    const surfaceRect = shell.getBoundingClientRect();
    shell.removeAttribute("data-measuring");

    const flip = computeCoverFlip(sourceRect, targetRect, surfaceRect);
    // The browser gate compares the painted first frame with the boxes the
    // offsets were derived from, so the measurement is observable on the
    // surface itself (`data-flip-source` / `data-flip-target`).
    if (sourceRect)
      shell.dataset.flipSource = `${Math.round(sourceRect.left)},${Math.round(sourceRect.top)},${Math.round(sourceRect.width)},${Math.round(sourceRect.height)}`;
    shell.dataset.flipTarget = `${Math.round(targetRect.left)},${Math.round(targetRect.top)},${Math.round(targetRect.width)},${Math.round(targetRect.height)}`;
    const prefix = phase === "closing" ? "--flip-exit-" : "--flip-";
    if (!flip) {
      shell.style.setProperty(`${prefix}x`, "0px");
      shell.style.setProperty(`${prefix}y`, "0px");
      shell.style.setProperty(`${prefix}scale`, "1");
      reset("none");
      return;
    }

    shell.style.setProperty(`${prefix}x`, `${flip.x}px`);
    shell.style.setProperty(`${prefix}y`, `${flip.y}px`);
    shell.style.setProperty(`${prefix}scale`, String(flip.scale));
    // The surface grows from (and collapses toward) the current-playing area.
    shell.style.setProperty("--transition-origin-x", `${flip.originX}px`);
    shell.style.setProperty("--transition-origin-y", `${flip.originY}px`);
    reset("cover");
  }, [phase, shellRef]);

  return { applied: appliedRef.current };
}
