import { useLayoutEffect, type RefObject } from "react";

/** The breakpoint at which `player.css` puts the meta and the lyrics in one area. */
const WIDE_LAYOUT_QUERY = "(min-width: 981px)";

/** Breathing room between the metadata block and the first lyric line. */
const LYRICS_GAP_PX = 20;

/** Sub-pixel slack for the post-write geometry check (rounded margins, DPR). */
const GEOMETRY_TOLERANCE_PX = 1;

/** How many frames a rejected pass is re-driven before it is left alone. */
const MAX_REDRIVE_PASSES = 3;

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
 * line rewrapping. On WebView2 those events routinely land *after* a one-shot
 * measurement, and the lyrics paint into the song-title block (issue #75;
 * macOS measured late enough to hide it). So the metadata block's rendered
 * height is the single source of truth, re-measured through every channel that
 * can change it: a `ResizeObserver` on the info block, a double-rAF settle pass
 * after the (re)mount, `document.fonts.ready`, the window `resize` listener and
 * the media query's own `change` event.
 *
 * Written as a **plain inline `margin-top`**, not through the custom property
 * the prototype uses. The e2e A9 gate caught the property route losing
 * intermittently on Windows: the inline `--lyrics-start-offset` sat at its
 * correct value while `margin-top: var(--lyrics-start-offset, 0px)` computed its
 * `0px` fallback for the element's whole lifetime (issue #90).
 *
 * The value is the metadata block's **rendered height plus the gap**, and the
 * offset already on the column is removed **arithmetically** rather than by
 * clearing it and reading again. Clearing does not survive to the next
 * synchronous layout read on WebView2 — the read answers from the pre-clear
 * layout for a frame or two — so a clear-then-read pass measured its own
 * previous offset and wrote out the collapsed difference (issue #90, A9 "info
 * block grew": 48px written where 137px was needed). The metadata box is the
 * one this hook never moves, so its height cannot be stale about the hook's
 * own write.
 *
 * Every pass is still judged by the **geometry it produced**, never by the value
 * it wrote. A `display: none` metadata block answers `getBoundingClientRect`
 * with all zeros, and that reading must not become a margin — a clamped 0/20px
 * runs the first lyric lines straight through the song title, and a write-back
 * check that only compares the value to itself confirms it as good. Nothing was
 * measured there, so a pass with no rendered box, a pass whose reading says the
 * two boxes are not sharing an area after all, and a write that did not take all
 * keep the previous margin and ask again next frame. jsdom has neither layout
 * nor these APIs — the fallbacks drop channels one by one and `isWideLayout`
 * answers "not wide", which is safe: nothing there can measure.
 */
export function useLyricsStartOffset(
  shellRef: RefObject<HTMLDivElement | null>,
  infoSignature: string,
  focusOpen: boolean,
) {
  useLayoutEffect(() => {
    const shell = shellRef.current;
    const info = shell?.querySelector<HTMLElement>(".now-playing-info");
    const lyrics = shell?.querySelector<HTMLElement>(".lyrics");
    if (!shell || !info || !lyrics) return;

    let redrivePasses = 0;
    // Shared frame handle for every deferred pass (the settle below and the
    // retries), so cleanup cancels them all.
    let frame = 0;

    const redrive = () => {
      if (redrivePasses >= MAX_REDRIVE_PASSES) return;
      redrivePasses += 1;
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(apply);
    };

    const hasRenderedBox = (element: HTMLElement) => {
      const rect = element.getBoundingClientRect();
      return rect.width > 0 && rect.height > 0;
    };

    const apply = () => {
      frame = 0;
      // ≤980px stacks record / info / lyrics in one column and 歌词专注阅读 hands
      // the whole surface to the lyrics. Neither has a shared area to separate,
      // and each owns the column's `margin-top` for itself (`responsive.css`
      // gives the focused column `margin: 0 auto`) — an inline value left here
      // would beat that rule, so it has to come off rather than linger.
      if (focusOpen || !isWideLayout()) {
        lyrics.style.marginTop = "";
        return;
      }
      if (!hasRenderedBox(info) || !hasRenderedBox(lyrics)) {
        redrive();
        return;
      }
      const infoRect = info.getBoundingClientRect();
      const lyricsRect = lyrics.getBoundingClientRect();
      // The column's un-offset position: the margin it currently carries is
      // taken back out of the measured top. Cleared-and-re-read is not the same
      // thing: WebView2 (and any Chromium under
      // `Emulation.setDeviceMetricsOverride`) answers the synchronous rect read
      // that follows the clear from the *pre-clear* layout, so that pass
      // measured its own previous offset —
      // `infoBottom − (areaTop + previous) + gap` — and wrote the collapsed
      // number out. That is the metadata/lyrics overlap of issue #90, caught by
      // the A9 "info block grew" step as a 48px margin where 137px was needed.
      const applied = Number.parseFloat(lyrics.style.marginTop) || 0;
      const naturalTop = lyricsRect.top - applied;
      // The column already starts below the metadata with room to spare: the two
      // boxes are not sharing one grid area, so their distance is not an offset.
      // Nothing was measured here; the previous margin stays.
      if (naturalTop >= infoRect.bottom + LYRICS_GAP_PX - GEOMETRY_TOLERANCE_PX) {
        redrivePasses = 0;
        return;
      }
      // The metadata block's own height plus the gap. Both boxes are grid items
      // in the same area with no margin on the metadata, so its rendered height
      // *is* the distance the column has to give up — and the metadata box is the
      // one this hook never moves, so this read cannot be stale about a margin
      // the hook wrote a frame earlier.
      lyrics.style.marginTop = `${Math.round(infoRect.height + LYRICS_GAP_PX)}px`;
      // Judge the outcome, not the write: a dropped write and a box that moved
      // right after the measurement both read back exactly what was written. A
      // read that is merely *stale* here costs a frame or two of re-driving —
      // never the value, which depends only on the metadata box above.
      const aligned =
        lyrics.getBoundingClientRect().top >=
        info.getBoundingClientRect().bottom + LYRICS_GAP_PX - GEOMETRY_TOLERANCE_PX;
      if (aligned) redrivePasses = 0;
      else redrive();
    };

    apply();

    // The first layout of a remount can still carry fallback-font metrics;
    // two frames in, the real font has typically swapped. Idempotent with the
    // initial pass — whichever sees the final geometry wins.
    const scheduleSettle = () => {
      cancelAnimationFrame(frame);
      frame = requestAnimationFrame(() => {
        frame = requestAnimationFrame(apply);
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
    // The breakpoint itself, not only the window's size. `resize` is not the
    // last word on a crossing: a classic scrollbar appearing or going (a longer
    // library list, the queue panel) shrinks and restores the layout viewport
    // that both `@media` and `matchMedia` answer to, without resizing the
    // window. The pass that saw "not wide" cleared the margin, and until this
    // listener nothing ever asked the question again.
    const wideQuery =
      typeof window.matchMedia === "function" ? window.matchMedia(WIDE_LAYOUT_QUERY) : null;
    wideQuery?.addEventListener("change", apply);
    return () => {
      disposed = true;
      cancelAnimationFrame(frame);
      observer?.disconnect();
      window.removeEventListener("resize", apply);
      wideQuery?.removeEventListener("change", apply);
    };
  }, [shellRef, infoSignature, focusOpen]);
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
