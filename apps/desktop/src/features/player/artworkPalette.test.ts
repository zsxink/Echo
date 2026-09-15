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

function whiteAt70PercentOver(background: [number, number, number]): [number, number, number] {
  return background.map((channel) => Math.round(255 * 0.7 + channel * 0.3)) as [
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

  it("keeps grayscale artwork neutral and supplies a dark, readable palette", () => {
    const pixels = new Uint8ClampedArray([
      ...Array.from({ length: 64 }, () => rgba(94, 94, 94)).flat(),
      ...Array.from({ length: 32 }, () => rgba(180, 180, 180)).flat(),
    ]);

    const palette = paletteFromPixels(pixels);

    expect(palette).not.toBeNull();
    for (const color of [palette!.tint, palette!.background, palette!.glow]) {
      const [red, green, blue] = hexToRgb(color);
      expect(Hct.fromInt(argbFromRgb(red, green, blue)).chroma).toBeLessThan(5);
    }
    expect(Hct.fromInt(argbFromRgb(...hexToRgb(palette!.background))).tone).toBeLessThan(18);
    const glow = hexToRgb(palette!.glow);
    expect(contrast(whiteAt70PercentOver(glow), glow)).toBeGreaterThanOrEqual(4.5);
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
