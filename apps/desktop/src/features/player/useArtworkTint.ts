import { useEffect } from "react";
import { loadArtworkPalette, type ArtworkPalette } from "./artworkPalette";

const properties = ["--player-tint", "--player-background", "--player-glow"] as const;

export function clearArtworkTint(): void {
  for (const property of properties) document.body.style.removeProperty(property);
}

function applyPalette(palette: ArtworkPalette | null): void {
  if (!palette) return clearArtworkTint();
  for (const [index, color] of [palette.tint, palette.background, palette.glow].entries()) {
    document.body.style.setProperty(properties[index], color);
  }
}

/** Keep the previous color while a new cover resolves; only the current track
 * may publish. The outer player owns cleanup so keyed lyric remounts do not
 * reset the color or interrupt the body's CSS transition. */
export function useArtworkTint(key: string | null, url: string | null, pending: boolean): void {
  useEffect(() => {
    let cancelled = false;
    const timeout = window.setTimeout(() => {
      if (!cancelled) applyPalette(null);
    }, 3000);
    if (key && url) {
      void loadArtworkPalette(key, url).then((palette) => {
        if (cancelled) return;
        window.clearTimeout(timeout);
        applyPalette(palette);
      });
    } else if (!pending) {
      window.clearTimeout(timeout);
      applyPalette(null);
    }
    return () => {
      cancelled = true;
      window.clearTimeout(timeout);
    };
  }, [key, url, pending]);
}
