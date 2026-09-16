import { Hct, argbFromRgb } from "@material/material-color-utilities";
import { afterEach, describe, expect, it, vi } from "vitest";

import { loadArtworkPalette, paletteFromPixels } from "./artworkPalette";

function rgba(red: number, green: number, blue: number, alpha = 255): number[] {
  return [red, green, blue, alpha];
}

function hexToRgb(hex: string): [number, number, number] {
  return [
    Number.parseInt(hex.slice(1, 3), 16),
    Number.parseInt(hex.slice(3, 5), 16),
    Number.parseInt(hex.slice(5, 7), 16),
  ];
}

function relativeLuminance([red, green, blue]: [number, number, number]): number {
  const channel = (value: number) => {
    const normalized = value / 255;
    return normalized <= 0.04045 ? normalized / 12.92 : ((normalized + 0.055) / 1.055) ** 2.4;
  };
  return 0.2126 * channel(red) + 0.7152 * channel(green) + 0.0722 * channel(blue);
}

function contrast(first: [number, number, number], second: [number, number, number]): number {
  const [lighter, darker] = [relativeLuminance(first), relativeLuminance(second)].sort(
    (a, b) => b - a,
  );
  return (lighter + 0.05) / (darker + 0.05);
}

/** The ink `artwork.css` paints inactive lyrics with, composited over whatever
 * sits behind the column — the stylesheet fades to 20% transparency. Opacity is
 * what makes muted lyrics unreadable on a light surface (the prototype's
 * 30%-transparent *white* was readable on a dark one; the same pair inverted is
 * not), so this is the pair worth pinning. */
function inkAt80PercentOver(
  ink: [number, number, number],
  background: [number, number, number],
): [number, number, number] {
  return ink.map((channel, index) => Math.round(channel * 0.8 + background[index] * 0.2)) as [
    number,
    number,
    number,
  ];
}

class TestImage {
  static instances: TestImage[] = [];
  crossOrigin: string | null = null;
  naturalWidth = 2;
  naturalHeight = 2;
  onload: ((event: Event) => void) | null = null;
  onerror: ((event: Event) => void) | null = null;
  url = "";

  constructor() {
    TestImage.instances.push(this);
  }

  set src(url: string) {
    this.url = url;
  }
}

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
  vi.useRealTimers();
  TestImage.instances = [];
});

describe("paletteFromPixels", () => {
  it("returns no palette when every pixel is transparent", () => {
    expect(
      paletteFromPixels(new Uint8ClampedArray([...rgba(50, 120, 220, 0), ...rgba(255, 0, 0, 0)])),
    ).toBeNull();
  });

  it("uses the hue of partially transparent artwork", () => {
    const palette = paletteFromPixels(new Uint8ClampedArray([...rgba(30, 100, 210, 128)]));

    expect(palette).not.toBeNull();
    const [red, green, blue] = hexToRgb(palette!.tint);
    const tint = Hct.fromInt(argbFromRgb(red, green, blue));
    expect(tint.hue).toBeGreaterThan(220);
    expect(tint.hue).toBeLessThan(310);
  });

  it("keeps a large cover color ahead of a tiny vivid accent", () => {
    const pixels = new Uint8ClampedArray([
      ...Array.from({ length: 180 }, () => rgba(22, 72, 154)).flat(),
      ...Array.from({ length: 6 }, () => rgba(255, 12, 30)).flat(),
    ]);

    const palette = paletteFromPixels(pixels);

    expect(palette).not.toBeNull();
    const [red, green, blue] = hexToRgb(palette!.tint);
    const tint = Hct.fromInt(argbFromRgb(red, green, blue));
    // The blue cover is near hue 270; a small red logo must not win selection.
    expect(tint.hue).toBeGreaterThan(220);
    expect(tint.hue).toBeLessThan(310);
  });

  it("returns one palette for one artwork, however many times it is asked", () => {
    // Three near-identical greens whose weighted scores are almost tied, so the
    // winner is decided by whatever clustering the extraction produces. It must
    // not come from a random source: `QuantizerCelebi` seeds its k-means
    // cluster assignment from `Math.random`, and on exactly this input it
    // returned four different palettes across thirty runs. That is not a
    // theoretical wobble — a cover like this (a green cover whose runner-up is
    // a dark neutral) flipped between a tinted and an almost-theme background
    // between launches, which reads in the app as "the background follows the
    // cover… sometimes". Fifty runs make a random quantiser's escape
    // improbable enough to be a reliable red.
    const pixels = new Uint8ClampedArray([
      ...Array.from({ length: 40 }, () => rgba(40, 60, 40)).flat(),
      ...Array.from({ length: 38 }, () => rgba(45, 58, 42)).flat(),
      ...Array.from({ length: 36 }, () => rgba(38, 62, 44)).flat(),
    ]);

    const first = paletteFromPixels(pixels);

    expect(first).not.toBeNull();
    for (let attempt = 0; attempt < 50; attempt += 1) {
      expect(paletteFromPixels(pixels)).toEqual(first);
    }
  });

  it("keeps grayscale artwork neutral and supplies a light, readable palette", () => {
    const pixels = new Uint8ClampedArray([
      ...Array.from({ length: 64 }, () => rgba(94, 94, 94)).flat(),
      ...Array.from({ length: 32 }, () => rgba(180, 180, 180)).flat(),
    ]);

    const palette = paletteFromPixels(pixels);

    expect(palette).not.toBeNull();
    for (const color of [palette!.tint, palette!.background, palette!.glow, palette!.on]) {
      const [red, green, blue] = hexToRgb(color);
      expect(Hct.fromInt(argbFromRgb(red, green, blue)).chroma).toBeLessThan(5);
    }
    // 素白: the surface is a light paper now, so a *dark* background is the
    // regression — the failure mode that made every cover read as 黑灰.
    expect(Hct.fromInt(argbFromRgb(...hexToRgb(palette!.background))).tone).toBeGreaterThan(80);
    // The two colours the immersion surface actually paints on top of each
    // other: ink on paper, at AAA.
    expect(contrast(hexToRgb(palette!.background), hexToRgb(palette!.on))).toBeGreaterThanOrEqual(
      7,
    );
    // And the weakest pair on the surface: an inactive lyric over the corner
    // wash, which must still clear AA.
    const glow = hexToRgb(palette!.glow);
    expect(contrast(inkAt80PercentOver(hexToRgb(palette!.on), glow), glow)).toBeGreaterThanOrEqual(
      4.5,
    );
  });
});

describe("loadArtworkPalette", () => {
  function installCanvas(pixels: Uint8ClampedArray): void {
    vi.spyOn(HTMLCanvasElement.prototype, "getContext").mockReturnValue({
      drawImage: vi.fn(),
      getImageData: () => ({ data: pixels }),
    } as unknown as CanvasRenderingContext2D);
    vi.stubGlobal("Image", TestImage);
  }

  it("shares an in-flight extraction and caches only a successful palette", async () => {
    installCanvas(new Uint8ClampedArray([...rgba(23, 90, 150), ...rgba(23, 90, 150)]));
    const key = "palette-test-success";
    const first = loadArtworkPalette(key, "cover://first");
    const second = loadArtworkPalette(key, "cover://ignored-after-share");

    expect(first).toBe(second);
    expect(TestImage.instances).toHaveLength(1);
    expect(TestImage.instances[0].crossOrigin).toBe("anonymous");
    TestImage.instances[0].onload?.(new Event("load"));
    await expect(first).resolves.toMatchObject({
      background: expect.stringMatching(/^#[0-9a-f]{6}$/i),
    });

    await expect(loadArtworkPalette(key, "cover://cached")).resolves.not.toBeNull();
    expect(TestImage.instances).toHaveLength(1);
  });

  it("does not cache a failed image, so that key can be retried", async () => {
    installCanvas(new Uint8ClampedArray([...rgba(20, 20, 20)]));
    const key = "palette-test-retry-after-error";
    const failed = loadArtworkPalette(key, "cover://broken");
    TestImage.instances[0].onerror?.(new Event("error"));
    await expect(failed).resolves.toBeNull();

    const retry = loadArtworkPalette(key, "cover://recovered");
    expect(TestImage.instances).toHaveLength(2);
    TestImage.instances[1].onload?.(new Event("load"));
    await expect(retry).resolves.not.toBeNull();
  });

  it("settles a stalled image and releases its in-flight entry for retry", async () => {
    vi.useFakeTimers();
    installCanvas(new Uint8ClampedArray([...rgba(20, 20, 20)]));
    const key = "palette-test-timeout-retry";
    const stalled = loadArtworkPalette(key, "cover://stalled");
    await vi.advanceTimersByTimeAsync(3000);
    await expect(stalled).resolves.toBeNull();

    const retry = loadArtworkPalette(key, "cover://retry");
    expect(TestImage.instances).toHaveLength(2);
    TestImage.instances[1].onload?.(new Event("load"));
    await expect(retry).resolves.not.toBeNull();
  });
});
