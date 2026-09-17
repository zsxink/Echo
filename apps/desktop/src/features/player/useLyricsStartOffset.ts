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
 * Without the measurement the custom property falls back to `0px`, and the
 * faded lyric lines paint *underneath* the title (`.now-playing-info` is
 * `z-index: 1; pointer-events: none`), which reads as translucent lyrics
 * behind the song name. jsdom has no layout, so both halves of that are
 * asserted on rendered geometry in `e2e/run-e2e.mjs` instead.
 *
 * `infoSignature` carries every input that changes the metadata's height; the
 * listener covers width changes (which rewrap the title).
 */
export function useLyricsStartOffset(
  shellRef: RefObject<HTMLDivElement | null>,
  infoSignature: string,
) {
  useLayoutEffect(() => {
    const shell = shellRef.current;
    const apply = () => {
      const info = shell?.querySelector<HTMLElement>(".now-playing-info");
      const lyrics = shell?.querySelector<HTMLElement>(".lyrics");
      if (!info || !lyrics) return;
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
    window.addEventListener("resize", apply);
    return () => window.removeEventListener("resize", apply);
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
