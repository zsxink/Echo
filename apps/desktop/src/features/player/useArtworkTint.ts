import { useEffect } from "react";

import { loadArtworkPalette, type ArtworkPalette } from "./artworkPalette";

const properties = ["--player-tint", "--player-background", "--player-glow"] as const;

/**
 * Why the immersive surface ended up without a cover colour, published on
 * `<body data-artwork-tint>`.
 *
 * All three states paint identically — `artwork.css` falls back to
 * `var(--accent)`, so no cover colour *is* the theme colour — which means a
 * broken extraction is indistinguishable from a song that simply has no
 * embedded artwork. That is not a theoretical gap: the extraction reads
 * pixels through a canvas, so a refused cross-origin grant (`cover://` is a
 * different origin from the `tauri://localhost` document) makes the image
 * load fail, the palette come back `null`, and the background stay on the
 * theme — the exact rendering of "封面取色没生效", with nothing to see
 * anywhere. Recording the outcome turns that silent degradation into a fact
 * the browser suite can assert and a person can read off DevTools.
 *
 * `<body>` already carries this kind of observability contract for the shell
 * (`data-playing`, `data-empty`, `data-focus` — all asserted in
 * `e2e/run-e2e.mjs`); this is the same contract for the tint.
 */
export type ArtworkTintOutcome = "cover" | "none" | "fallback";

/**
 * Drop the artwork colours and forget the outcome — the immersive surface is
 * closed, so there is no tint state to report. (The keyed track surface calls
 * this on unmount and the outer player calls it when the mode closes.)
 */
export function clearArtworkTint(): void {
  for (const property of properties) document.body.style.removeProperty(property);
  delete document.body.dataset.artworkTint;
}

/**
 * Fall back to the theme, recording which kind of fallback this is:
 * `"none"` for a song with no embedded artwork (the designed default),
 * `"fallback"` for artwork that was there but produced no palette.
 */
function fallBackToTheme(outcome: Exclude<ArtworkTintOutcome, "cover">): void {
  clearArtworkTint();
  document.body.dataset.artworkTint = outcome;
}

function applyPalette(
  palette: ArtworkPalette | null,
  outcome: Exclude<ArtworkTintOutcome, "cover">,
): void {
  if (!palette) return fallBackToTheme(outcome);
  for (const [index, color] of [palette.tint, palette.background, palette.glow].entries()) {
    document.body.style.setProperty(properties[index], color);
  }
  document.body.dataset.artworkTint = "cover";
}

/** Keep the previous color while a new cover resolves; only the current track
 * may publish. The outer player owns cleanup so keyed lyric remounts do not
 * reset the color or interrupt the body's CSS transition.
 *
 * `pending` means "this song's artwork may still arrive" — the metadata or the
 * cover key has not landed yet. The colour is deliberately *not* cleared in
 * that window (the previous track's tint would flash back to the theme on
 * every song change), so a cover that never resolves is reported as
 * `"fallback"` by the timeout rather than as `"none"`. */
export function useArtworkTint(key: string | null, url: string | null, pending: boolean): void {
  useEffect(() => {
    let cancelled = false;
    const timeout = window.setTimeout(() => {
      if (!cancelled) applyPalette(null, "fallback");
    }, 3000);
    if (key && url) {
      void loadArtworkPalette(key, url).then((palette) => {
        if (cancelled) return;
        window.clearTimeout(timeout);
        // A `null` palette here is artwork that *could not be read* (decode
        // error, tainted canvas, refused origin, timeout), never a song
        // without artwork — that case does not reach this branch.
        applyPalette(palette, "fallback");
      });
    } else if (!pending) {
      window.clearTimeout(timeout);
      applyPalette(null, "none");
    }
    return () => {
      cancelled = true;
      window.clearTimeout(timeout);
    };
  }, [key, url, pending]);
}
