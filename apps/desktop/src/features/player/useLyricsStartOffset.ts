import { useLayoutEffect, type RefObject } from "react";

/** The breakpoint at which `player.css` puts the meta and the lyrics in one area. */
const WIDE_LAYOUT_QUERY = "(min-width: 981px)";

/** Breathing room between the metadata block and the first lyric line. */
const LYRICS_GAP_PX = 20;

/**
 * Prototype parity for the wide layout: `@media (min-width: 981px)` gives
 * `.now-playing-info` and `.lyrics` the **same** grid area (`"record lyrics"`,
 * see `player.css`), so the lyrics only stop covering the song title because
 * the prototype measures the metadata block and writes `--lyrics-start-offset`
 * (`alignDesktopLyricsRegion`, echo-desktop-player.html:1758-1763). The offset
 * cannot live in the stylesheet: it depends on the rendered height of the
 * metadata — title wrapping, artist/album line, 临时 import row.
 *
 * The offset is **observed, not triggered**: a signature of
 * title/artist/album/duration misses every height change that does not alter
 * those four strings — `song_detail` landing after the remount a track change
 * causes, CJK fallback-font metrics swapping in once webfonts load, a meta
 * line rewrapping. On WebView2 those events routinely land *after* the
 * one-shot measurement, and the lyrics painted into the song-title block
 * (issue #75; macOS measured late enough to hide it). So the metadata block's
 * rendered height is the single source of truth, re-measured through every
 * channel that can change it: a `ResizeObserver` on the info block, a
 * double-rAF settle pass after the (re)mount, `document.fonts.ready`, and the
 * window `resize` listener. jsdom has neither layout nor these APIs — the
 * fallbacks drop channels one by one and `isWideLayout` answers "not wide",
 * which is safe: nothing there can measure.
 */
export function useLyricsStartOffset(
  shellRef: RefObject<HTMLDivElement | null>,
  infoSignature: string,
) {
  useLayoutEffect(() => {
    const shell = shellRef.current;
    const info = shell?.querySelector<HTMLElement>(".now-playing-info");
    const lyrics = shell?.querySelector<HTMLElement>(".lyrics");
    if (!shell || !info || !lyrics) return;

    const apply = () => {
      // Measure from the un-offset state: the property moves the very box it is
      // measured against, so it has to come off first (prototype does the same).
      lyrics.style.removeProperty("--lyrics-start-offset");
      // ≤980px stacks record / info / lyrics in one column, so there is no
      // shared grid area to separate.
      if (!isWideLayout()) return;
      const offset = Math.max(
        0,
        info.getBoundingClientRect().bottom - lyrics.getBoundingClientRect().top + LYRICS_GAP_PX,
      );
      lyrics.style.setProperty("--lyrics-start-offset", `${Math.round(offset)}px`);
    };

    apply();

    // The first layout of a remount can still carry fallback-font metrics;
    // two frames in, the real font has typically swapped. Idempotent with the
    // initial pass — whichever sees the final geometry wins.
    let settleFrame = 0;
    const scheduleSettle = () => {
      cancelAnimationFrame(settleFrame);
      settleFrame = requestAnimationFrame(() => {
        settleFrame = requestAnimationFrame(apply);
      });
    };
    scheduleSettle();

    const observer =
      typeof ResizeObserver === "function" ? new ResizeObserver(() => apply()) : null;
    // Observe the border box — the box `getBoundingClientRect` measures. The
    // default content-box would stay silent through padding changes, and the
    // offset formula reads the border box's bottom.
    observer?.observe(info, { box: "border-box" });

    // `fonts.ready` may resolve long after this surface unmounted (and jsdom
    // may never settle it at all); a late `apply()` would write a property onto
    // a DOM node nobody owns anymore, so the callback checks the flag.
    let disposed = false;
    Promise.resolve(document.fonts?.ready).then(
      () => {
        if (!disposed) apply();
      },
      () => {
        /* a rejected fonts promise must not surface as an unhandled error */
      },
    );

    window.addEventListener("resize", apply);
    return () => {
      disposed = true;
      cancelAnimationFrame(settleFrame);
      observer?.disconnect();
      window.removeEventListener("resize", apply);
    };
  }, [shellRef, infoSignature]);
}

/**
 * jsdom implements no `matchMedia` at all (the unit suite would throw), and it
 * could not answer a width query anyway — it has no layout. The desktop WebView
 * always has it. "Not wide" is the safe answer wherever the query is missing:
 * the offset is a measurement, and nothing here can measure.
 */
export function isWideLayout(): boolean {
  return typeof window.matchMedia === "function" && window.matchMedia(WIDE_LAYOUT_QUERY).matches;
}
