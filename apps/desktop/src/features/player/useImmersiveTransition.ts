import { useEffect, useRef, useState } from "react";

/**
 * The immersive player's explicit visibility state machine (issue #85).
 *
 * The surface is no longer mounted on `ui.immersiveOpen` alone: it has to stay
 * rendered for one short exit animation after the overlay stack has already let
 * it go. Four phases instead of a boolean:
 *
 *   closed → opening → open → closing → closed
 *
 * React owns the lifecycle (mount, timers, body classes, focus) and the
 * stylesheet owns the visuals (`data-phase` in `player.css`). Deliberately no
 * `transition-behavior: allow-discrete` / `@starting-style`: their support
 * differs across macOS WebKit, WebView2 and WebKitGTK, and the accessibility
 * contract needs the layer gone from the stack *immediately* — which is a
 * JavaScript fact, not a CSS one.
 *
 * Reduced motion is decided here, not only in CSS: the global 1ms clamp cannot
 * cancel a delayed unmount, so the phase changes directly and no timer runs.
 */

export type ImmersivePhase = "closed" | "opening" | "open" | "closing";

/** Entry duration — mirrors `--motion-base` in `tokens.css`. */
export const IMMERSIVE_OPEN_MS = 220;
/** Exit duration — mirrors `--motion-exit` in `tokens.css`. */
export const IMMERSIVE_EXIT_MS = 180;

/**
 * jsdom implements no `matchMedia`; "no preference" is the honest default there
 * (and the desktop WebView always answers).
 */
export function prefersReducedMotion(): boolean {
  return (
    typeof window !== "undefined" &&
    typeof window.matchMedia === "function" &&
    window.matchMedia("(prefers-reduced-motion: reduce)").matches
  );
}

/**
 * Drive the phase from the authoritative `ui.immersiveOpen` flag.
 *
 * Interruptions converge on one final state:
 *  - closing → open resumes straight to `open` (the surface never unmounted, so
 *    replaying the entry would jump);
 *  - opening → closed cancels the `opening → open` frame and starts `closing`;
 *  - every timer/frame is cleared by the effect that owns it.
 */
export function useImmersiveTransition(open: boolean): {
  phase: ImmersivePhase;
  settled: boolean;
} {
  const [phase, setPhase] = useState<ImmersivePhase>(() => {
    if (!open) return "closed";
    return prefersReducedMotion() ? "open" : "opening";
  });

  // `settled` answers a narrower question than `phase`: are the entry/exit
  // transforms done, so that `getBoundingClientRect()` reflects final layout?
  // `open` only means the state machine advanced; the CSS transition still runs
  // for `IMMERSIVE_OPEN_MS`. Consumers that measure geometry (the lyrics column)
  // must wait for this flag. It is false for every phase except `open`, so a
  // closing surface can never be measured with stale `true`; re-entering `open`
  // after an interrupted exit starts a fresh wait because the reset ran while
  // the surface was in `closing`.
  const [openSettled, setOpenSettled] = useState(() => phase === "open");
  const settled = phase === "open" && openSettled;

  const openRef = useRef(open);
  openRef.current = open;
  const phaseRef = useRef(phase);
  phaseRef.current = phase;

  // Follow the authoritative flag. This only *requests* a phase; the timers and
  // frames below own the actual progression.
  useEffect(() => {
    if (open) {
      if (phaseRef.current === "closed" || phaseRef.current === "closing") {
        // A close that was interrupted keeps its node and resumes at full
        // opacity — re-running the FLIP entry would read as a second mount.
        setPhase(prefersReducedMotion() || phaseRef.current === "closing" ? "open" : "opening");
      }
      return;
    }
    if (phaseRef.current === "opening" || phaseRef.current === "open") {
      setPhase(prefersReducedMotion() ? "closed" : "closing");
    }
  }, [open]);

  // Own the two timed edges of the machine.
  useEffect(() => {
    if (phase === "opening") {
      // Two frames: the first paints the FLIP start state (the surface's own
      // entry transform and the cover's measured offset), the next lets the
      // transition run from it to `none`. A single frame can be coalesced with
      // the layout effect's writes and skip the starting position entirely.
      let second = 0;
      const first = requestAnimationFrame(() => {
        second = requestAnimationFrame(() => {
          if (openRef.current && phaseRef.current === "opening") setPhase("open");
        });
      });
      return () => {
        cancelAnimationFrame(first);
        cancelAnimationFrame(second);
      };
    }
    if (phase === "closing") {
      const timer = window.setTimeout(() => {
        if (!openRef.current && phaseRef.current === "closing") setPhase("closed");
      }, IMMERSIVE_EXIT_MS);
      return () => window.clearTimeout(timer);
    }
    return undefined;
  }, [phase]);

  // Mark the geometry stable once the entry transition has had time to finish.
  // A timer mirrors the stylesheet exactly as the exit edge already does; a
  // `transitionend` listener would need target filtering, bubbling and
  // `transitioncancel` handling for no extra accuracy here.
  useEffect(() => {
    if (phase !== "open") {
      setOpenSettled(false);
      return undefined;
    }
    if (prefersReducedMotion()) {
      setOpenSettled(true);
      return undefined;
    }
    const timer = window.setTimeout(() => setOpenSettled(true), IMMERSIVE_OPEN_MS);
    return () => window.clearTimeout(timer);
  }, [phase]);

  return { phase, settled };
}
