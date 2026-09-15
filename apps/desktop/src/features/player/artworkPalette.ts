import { Hct, QuantizerCelebi, argbFromRgb, hexFromArgb } from "@material/material-color-utilities";

export interface ArtworkPalette {
  readonly tint: string;
  readonly background: string;
  readonly glow: string;
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
  const colors = [...QuantizerCelebi.quantize(pixels, 16)].map(([argb, count]) => ({
    color: Hct.fromInt(argb),
    count,
  }));
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
  return {
    tint: tone(primary, 65, 40),
    background: tone(primary, 12, 24),
    glow: tone(secondary, 22, 30),
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
