import { Hct, QuantizerWu, argbFromRgb, hexFromArgb } from "@material/material-color-utilities";

export interface ArtworkPalette {
  /** The record label's ink — the cover colour, kept saturated enough to read
   * against the black vinyl it sits on. */
  readonly tint: string;
  /** The immersion surface itself: a near-white paper carrying the cover's hue. */
  readonly background: string;
  /** A deeper wash of the secondary hue, for the surface's corner gradient. */
  readonly glow: string;
  /** Content colour for the immersion surface — title, meta, lyrics. The
   * surface is *light*, so this is a dark tone of the cover's own hue; the
   * theme's `--accent-on` (white) would be invisible on it. */
  readonly on: string;
}

/**
 * The cover's dominant swatches, each with the pixel weight behind it.
 *
 * `QuantizerCelebi` is the obvious choice and the wrong one: it refines Wu's
 * boxes with a k-means pass that seeds the initial cluster *assignment* from
 * `Math.random`, so the very same cover returns a different palette on
 * different calls. Cover art whose primaries sit close together — a green
 * cover whose runner-up is a dark neutral — then flips between a tinted and a
 * near-theme background from one launch to the next, which reads in the app as
 * "the background follows the cover… sometimes". Wu's pass splits the RGB cube
 * by pixel weight and calls no random source at all, so identical pixels give
 * an identical palette — and the immersive colour for a song is reproducible.
 *
 * Wu returns colours without populations, so each pixel is assigned to its
 * nearest swatch to recover the area weighting the selection below needs.
 */
function dominantSwatches(pixels: number[], maxColors: number): { argb: number; count: number }[] {
  const swatches = new QuantizerWu().quantize(pixels, maxColors);
  const counts = new Array<number>(swatches.length).fill(0);
  for (const pixel of pixels) {
    const red = (pixel >> 16) & 0xff;
    const green = (pixel >> 8) & 0xff;
    const blue = pixel & 0xff;
    let nearest = 0;
    let nearestDistance = Infinity;
    for (let index = 0; index < swatches.length; index += 1) {
      const distance =
        (red - ((swatches[index] >> 16) & 0xff)) ** 2 +
        (green - ((swatches[index] >> 8) & 0xff)) ** 2 +
        (blue - (swatches[index] & 0xff)) ** 2;
      if (distance < nearestDistance) {
        nearestDistance = distance;
        nearest = index;
      }
    }
    counts[nearest] += 1;
  }
  return swatches.map((argb, index) => ({ argb, count: counts[index] }));
}

/** Population leads; chroma can break a tie but a small bright logo must not
 * replace the cover itself. Neutral artwork deliberately bypasses theme-color
 * scoring, whose default fallback would introduce a color absent from it. */
export function paletteFromPixels(bytes: Uint8ClampedArray): ArtworkPalette | null {
  const pixels: number[] = [];
  for (let i = 0; i + 3 < bytes.length; i += 4) {
    const alpha = bytes[i + 3];
    if (alpha === 0) continue;
    // Weight coverage without compositing against an arbitrary theme color.
    // A translucent PNG retains its hue; fully transparent RGB is ignored.
    const weight = Math.max(1, Math.round((alpha / 255) * 4));
    const color = argbFromRgb(bytes[i], bytes[i + 1], bytes[i + 2]);
    for (let sample = 0; sample < weight; sample += 1) pixels.push(color);
  }
  if (!pixels.length) return null;
  const colors = dominantSwatches(pixels, 16).map(({ argb, count }) => ({
    color: Hct.fromInt(argb),
    count,
  }));
  if (!colors.length) return null;
  colors.sort(
    (a, b) =>
      b.count * (1 + Math.min(b.color.chroma, 48) / 96) -
      a.count * (1 + Math.min(a.color.chroma, 48) / 96),
  );
  const primary = colors[0].color;
  const neutral = colors.reduce((sum, c) => sum + c.count * c.color.chroma, 0) / pixels.length < 6;
  const secondary =
    colors.find(
      (c) =>
        c.count >= pixels.length * 0.15 &&
        c.color.chroma >= 6 &&
        Math.min(Math.abs(c.color.hue - primary.hue), 360 - Math.abs(c.color.hue - primary.hue)) >
          25,
    )?.color ?? primary;
  const tone = (color: Hct, lightness: number, cap: number) =>
    hexFromArgb(Hct.from(color.hue, neutral ? 0 : Math.min(color.chroma, cap), lightness).toInt());
  // 素白, not 深色: the immersion surface is a light paper that carries the
  // cover's hue, because a dark surface derived from a dark-ish cover lands on
  // near-black for most artwork and the tint stops being readable as colour at
  // all. Lightness stays high and chroma is loosened so the hue is actually
  // visible; `on` supplies the dark ink for it. Every pair here keeps WCAG AAA
  // (>7:1) contrast, which the unit suite and the browser suite both assert.
  return {
    tint: tone(primary, 65, 40),
    background: tone(primary, 92, 16),
    glow: tone(secondary, 84, 24),
    on: tone(primary, 24, 12),
  };
}

const cache = new Map<string, ArtworkPalette>();
const inFlight = new Map<string, Promise<ArtworkPalette | null>>();
const MAX_CACHE = 128;

export function loadArtworkPalette(key: string, url: string): Promise<ArtworkPalette | null> {
  const cached = cache.get(key);
  if (cached) {
    cache.delete(key);
    cache.set(key, cached);
    return Promise.resolve(cached);
  }
  const pending = inFlight.get(key);
  if (pending) return pending;
  const result = new Promise<ArtworkPalette | null>((resolve) => {
    const image = new Image();
    let finished = false;
    const finish = (palette: ArtworkPalette | null) => {
      if (finished) return;
      finished = true;
      clearTimeout(timeout);
      image.onload = null;
      image.onerror = null;
      resolve(palette);
    };
    const timeout = setTimeout(() => finish(null), 3000);
    image.crossOrigin = "anonymous";
    image.onload = () => {
      try {
        if (!image.naturalWidth || !image.naturalHeight) return finish(null);
        const canvas = document.createElement("canvas");
        const scale = Math.min(1, 64 / Math.max(image.naturalWidth, image.naturalHeight));
        canvas.width = Math.max(1, Math.round(image.naturalWidth * scale));
        canvas.height = Math.max(1, Math.round(image.naturalHeight * scale));
        const ctx = canvas.getContext("2d", { willReadFrequently: true });
        if (!ctx) return finish(null);
        ctx.drawImage(image, 0, 0, canvas.width, canvas.height);
        finish(paletteFromPixels(ctx.getImageData(0, 0, canvas.width, canvas.height).data));
      } catch {
        // Decode errors and a tainted canvas are ordinary no-artwork fallbacks.
        finish(null);
      }
    };
    image.onerror = () => finish(null);
    image.src = url;
  }).then((palette) => {
    inFlight.delete(key);
    if (palette) {
      cache.set(key, palette);
      if (cache.size > MAX_CACHE) cache.delete(cache.keys().next().value!);
    }
    return palette;
  });
  inFlight.set(key, result);
  return result;
}
