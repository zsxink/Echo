#!/usr/bin/env node
/**
 * Renders `artifacts/immersive-tint-palette.html`: every cover in the running
 * player's cache, pushed through the *real* palette implementation, drawn the
 * way the app paints the immersion surface.
 *
 * Usage:
 *   node --experimental-strip-types scripts/diagnostics/artwork-palette-report.mjs
 *   node --experimental-strip-types scripts/diagnostics/artwork-palette-report.mjs --out /tmp/x.html
 *   node --experimental-strip-types scripts/diagnostics/artwork-palette-report.mjs --limit 12
 *
 * Why it imports the app's TypeScript directly. This report used to be a
 * one-off esbuild bundle that *inlined a copy* of `paletteFromPixels`. When the
 * scheme flipped from dark to 素白 the copy kept emitting the old near-black
 * palette and the report silently became a picture of code that no longer
 * existed — the exact failure mode the frontend-fresh gate exists to catch,
 * except nothing was checking this file. Node's type stripping runs the real
 * module with no bundler and no build step, so the report cannot drift again.
 *
 * Inputs are read-only: the Spotify-style on-disk cover cache written by the
 * app (`covers/<content_hash>/original.bin`) plus `echo.sqlite` for titles.
 * Nothing here writes outside `artifacts/`.
 */

import { execFileSync } from "node:child_process";
import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const repoRoot = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const appSupport = join(homedir(), "Library/Application Support/org.echo-player.echo");
const coversDir = join(appSupport, "covers");
const database = join(appSupport, "echo.sqlite");
const scratch = join(repoRoot, "artifacts", ".palette-scratch");

const args = process.argv.slice(2);
const flag = (name, fallback) => {
  const index = args.indexOf(`--${name}`);
  return index === -1 ? fallback : args[index + 1];
};
const outFile = resolve(flag("out", join(repoRoot, "artifacts", "immersive-tint-palette.html")));
const limit = Number(flag("limit", "0")) || Infinity;

if (!existsSync(coversDir)) {
  console.error(`no cover cache at ${coversDir} — run the app once with a populated library`);
  process.exit(1);
}

// The real implementation, not a copy of it.
const { paletteFromPixels } = await import(
  pathToFileURL(join(repoRoot, "apps/desktop/src/features/player/artworkPalette.ts")).href
);

/* --- the scheme's own numbers ---------------------------------------------
 * These mirror `artwork.css`. Kept here as literals so the report states what
 * the app *asserts*, and a change to the CSS that is not reflected here is a
 * visible inconsistency rather than a silent one. */
const GATES = {
  paperLuminance: 0.6, // e2e A15: paper must be light, `coveredLuminance < 0.6` fails
  paperInk: 7, // artworkPalette.test.ts: WCAG AAA
  mutedInkGlow: 4.5, // artworkPalette.test.ts: WCAG AA, the weakest pair on the surface
};
const THEME_FALLBACK_WEIGHT_BACKGROUND = 0.88; // color-mix(in oklab, var(--accent), var(--bg) 88%)
const THEME_FALLBACK_WEIGHT_GLOW = 0.78;

/* --- colour maths --------------------------------------------------------- */

const srgbToLinear = (c) => (c <= 0.04045 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
const linearToSrgb = (c) => (c <= 0.0031308 ? c * 12.92 : 1.055 * c ** (1 / 2.4) - 0.055);

function rgbToOklab([r, g, b]) {
  const [lr, lg, lb] = [r, g, b].map(srgbToLinear);
  const l = Math.cbrt(0.4122214708 * lr + 0.5363325363 * lg + 0.0514459929 * lb);
  const m = Math.cbrt(0.2119034982 * lr + 0.6806995451 * lg + 0.1073969566 * lb);
  const s = Math.cbrt(0.0883024619 * lr + 0.2817188376 * lg + 0.6299787005 * lb);
  return [
    0.2104542553 * l + 0.793617785 * m - 0.0040720468 * s,
    1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s,
    0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s,
  ];
}

function oklabToRgb([L, a, b]) {
  const l = (L + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const m = (L - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const s = (L - 0.0894841775 * a - 1.291485548 * b) ** 3;
  return [
    linearToSrgb(4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s),
    linearToSrgb(-1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s),
    linearToSrgb(-0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s),
  ].map((value) => Math.max(0, Math.min(1, value)));
}

/** oklch() as written in `tokens.css`, with alpha folded onto `over`. */
const oklch = (lightness, chroma, hue) => {
  const radians = (hue * Math.PI) / 180;
  return [lightness, chroma * Math.cos(radians), chroma * Math.sin(radians)];
};
const mixOklab = (first, second, weightSecond) =>
  first.map((value, index) => value * (1 - weightSecond) + second[index] * weightSecond);

const hexToRgb = (hex) => {
  const value = Number.parseInt(hex.slice(1), 16);
  return [(value >> 16) & 0xff, (value >> 8) & 0xff, value & 0xff].map((v) => v / 255);
};
const rgbToHex = (rgb) =>
  `#${rgb
    .map((v) => Math.round(v * 255).toString(16).padStart(2, "0"))
    .join("")}`;

const relativeLuminance = ([r, g, b]) =>
  0.2126 * srgbToLinear(r) + 0.7152 * srgbToLinear(g) + 0.0722 * srgbToLinear(b);

function contrastRatio(a, b) {
  const [high, low] = [relativeLuminance(a), relativeLuminance(b)].sort((x, y) => y - x);
  return (high + 0.05) / (low + 0.05);
}

/** `color-mix(in oklab, ink, transparent 20%)` is ink at 80% alpha; the browser
 * then composites it in sRGB. Reproduce that, because the whole point of the
 * number is which pixels the lyrics actually render as. */
const compositeOver = (ink, background, alpha) =>
  ink.map((value, index) => value * alpha + background[index] * (1 - alpha));

const distance = (a, b) => Math.hypot(a[0] - b[0], a[1] - b[1], a[2] - b[2]) * 100;

/* --- inputs --------------------------------------------------------------- */

const themeFallback = {
  background: rgbToHex(
    oklabToRgb(
      mixOklab(
        oklch(0.6528, 0.2219, 19.38), // tokens.css --accent, coral-rose
        oklch(1, 0, 89.88), // tokens.css --bg, paper
        THEME_FALLBACK_WEIGHT_BACKGROUND,
      ),
    ),
  ),
  glow: rgbToHex(
    oklabToRgb(
      mixOklab(
        oklch(0.6528, 0.2219, 19.38),
        oklch(1, 0, 89.88),
        THEME_FALLBACK_WEIGHT_GLOW,
      ),
    ),
  ),
  on: rgbToHex(oklabToRgb(oklch(0.2134, 0, 89.88))), // tokens.css --fg
};

/** `content_hash` -> "title · artist", so a card says which song it is. */
function songLabels() {
  if (!existsSync(database)) return new Map();
  try {
    const json = execFileSync(
      "sqlite3",
      [
        "-json",
        database,
        `select c.content_hash as hash, s.title as title, s.artist as artist
           from songs s join cover_assets c on c.asset_key = 'cv1-' || s.cover_hash
          where s.cover_hash is not null
          group by c.content_hash`,
      ],
      { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] },
    );
    return new Map(
      JSON.parse(json || "[]").map((row) => [row.hash, [row.title, row.artist].filter(Boolean).join(" · ")]),
    );
  } catch {
    return new Map();
  }
}

/* --- decoding covers, with no binary of our own ---------------------------
 * The app decodes through a canvas at 64px on the longest side. `sips` reaches
 * the same scale and can emit an uncompressed BMP, which is a header and a
 * pixel array — cheaper to parse correctly than to keep a compiled helper
 * alive in /tmp for.
 *
 * Two shapes come back: 24bpp `BI_RGB` for opaque artwork, and 32bpp
 * `BI_BITFIELDS` under a `BITMAPV5HEADER` for artwork with an alpha channel.
 * The bitfield masks sit right after the 40-byte `BITMAPINFOHEADER` portion of
 * the V5 header — at file offset 54, *not* after the full 124-byte header,
 * which is where the pixel data starts. Reading them at the wrong offset is
 * how a decoder silently returns garbage instead of failing. */
const DEFAULT_MASKS = { red: 0x00ff0000, green: 0x0000ff00, blue: 0x000000ff, alpha: 0xff000000 };

function channelExtractor(mask) {
  if (!mask) return () => 255;
  let shift = 0;
  while (((mask >> shift) & 1) === 0 && shift < 32) shift += 1;
  const max = mask >>> shift;
  return (word) => {
    const value = (word & mask) >>> shift;
    return Math.round((value / max) * 255);
  };
}

function readBmpAsRgba(file) {
  const bitmap = readFileSync(file);
  const dataOffset = bitmap.readUInt32LE(10);
  const width = bitmap.readInt32LE(18);
  const signedHeight = bitmap.readInt32LE(22);
  const bitDepth = bitmap.readUInt16LE(28);
  const compression = bitmap.readUInt32LE(30);
  if ((compression !== 0 && compression !== 3) || (bitDepth !== 24 && bitDepth !== 32)) {
    throw new Error(`unsupported BMP: ${bitDepth}bpp, compression ${compression}`);
  }
  const bytesPerPixel = bitDepth / 8;
  const readPixel = bitmap.readUInt32LE.bind(bitmap);
  let red;
  let green;
  let blue;
  let alpha;
  if (compression === 0) {
    // Declared order for BI_RGB is BGR, little-endian in the file.
    blue = (word) => (word >> 0) & 0xff;
    green = (word) => (word >> 8) & 0xff;
    red = (word) => (word >> 16) & 0xff;
    alpha = () => 255;
  } else {
    const readMask = (index) => {
      const mask = bitmap.readUInt32LE(54 + index * 4);
      return mask || [DEFAULT_MASKS.red, DEFAULT_MASKS.green, DEFAULT_MASKS.blue, DEFAULT_MASKS.alpha][index];
    };
    red = channelExtractor(readMask(0));
    green = channelExtractor(readMask(1));
    blue = channelExtractor(readMask(2));
    alpha = channelExtractor(bitmap.readUInt32LE(66) ? readMask(3) : 0);
  }
  const height = Math.abs(signedHeight);
  const stride = Math.floor((bitDepth * width + 31) / 32) * 4;
  const rgba = new Uint8ClampedArray(width * height * 4);
  for (let y = 0; y < height; y += 1) {
    // A negative height means the rows are already top-down.
    const sourceRow = dataOffset + (signedHeight < 0 ? y : height - 1 - y) * stride;
    for (let x = 0; x < width; x += 1) {
      const source = sourceRow + x * bytesPerPixel;
      const word =
        bytesPerPixel === 4
          ? readPixel(source)
          : bitmap[source] | (bitmap[source + 1] << 8) | (bitmap[source + 2] << 16);
      const target = (y * width + x) * 4;
      rgba[target] = red(word);
      rgba[target + 1] = green(word);
      rgba[target + 2] = blue(word);
      rgba[target + 3] = alpha(word);
    }
  }
  return rgba;
}

/* --- gather --------------------------------------------------------------- */

mkdirSync(scratch, { recursive: true });
const labels = songLabels();
const entries = readdirSync(coversDir).sort().slice(0, limit);
const rows = [];
const skipped = [];

for (const hash of entries) {
  const original = join(coversDir, hash, "original.bin");
  if (!existsSync(original)) {
    skipped.push([hash, "no original.bin"]);
    continue;
  }
  const pixels = join(scratch, `${hash}.bmp`);
  const thumb = join(scratch, `${hash}.jpg`);
  try {
    execFileSync("sips", ["-s", "format", "bmp", "-Z", "64", original, "--out", pixels], {
      stdio: "ignore",
    });
    execFileSync("sips", ["-s", "format", "jpeg", "-Z", "240", original, "--out", thumb], {
      stdio: "ignore",
    });
  } catch {
    skipped.push([hash, "sips could not decode"]);
    continue;
  }

  const palette = paletteFromPixels(readBmpAsRgba(pixels));
  if (!palette) {
    skipped.push([hash, "no palette (fully transparent)"]);
    continue;
  }

  const paper = hexToRgb(palette.background);
  const ink = hexToRgb(palette.on);
  const glow = hexToRgb(palette.glow);
  const muted = compositeOver(ink, glow, 0.8);
  const ring = mixOklab(hexToRgb(palette.tint), paper, 0.58);

  rows.push({
    hash: hash.slice(0, 10),
    label: labels.get(hash) ?? hash.slice(0, 10),
    thumb: `data:image/jpeg;base64,${readFileSync(thumb).toString("base64")}`,
    palette,
    mutedHex: rgbToHex(muted),
    ringHex: rgbToHex(ring),
    paperLuminance: relativeLuminance(paper),
    paperInk: contrastRatio(paper, ink),
    mutedInkGlow: contrastRatio(muted, glow),
    deltaFallback: distance(paper, hexToRgb(themeFallback.background)),
  });
}

rmSync(scratch, { recursive: true, force: true });

if (!rows.length) {
  console.error("no covers produced a palette — nothing to report");
  process.exit(1);
}

/* --- summarise ------------------------------------------------------------ */

const worst = {
  paperLuminance: rows.reduce((a, b) => (a.paperLuminance < b.paperLuminance ? a : b)),
  paperInk: rows.reduce((a, b) => (a.paperInk < b.paperInk ? a : b)),
  mutedInkGlow: rows.reduce((a, b) => (a.mutedInkGlow < b.mutedInkGlow ? a : b)),
  deltaFallback: rows.reduce((a, b) => (a.deltaFallback < b.deltaFallback ? a : b)),
};
const failures = [
  worst.paperLuminance.paperLuminance >= GATES.paperLuminance
    ? null
    : `最低纸面亮度 ${worst.paperLuminance.paperLuminance.toFixed(3)} < ${GATES.paperLuminance}`,
  worst.paperInk.paperInk >= GATES.paperInk
    ? null
    : `最差纸/墨对比 ${worst.paperInk.paperInk.toFixed(2)}:1 < ${GATES.paperInk}:1`,
  worst.mutedInkGlow.mutedInkGlow >= GATES.mutedInkGlow
    ? null
    : `最差弱化字/晕对比 ${worst.mutedInkGlow.mutedInkGlow.toFixed(2)}:1 < ${GATES.mutedInkGlow}:1`,
].filter(Boolean);

/* --- render --------------------------------------------------------------- */

const stat = (label, value, tone = "") =>
  `<div${tone ? ` class="${tone}"` : ""}><dt>${label}</dt><dd>${value}</dd></div>`;

const swatch = (name, hex, note) =>
  `<div class="swatch"><i style="background:${hex}"></i><span>${name}<code>${hex}</code>${
    note ? `<em>${note}</em>` : ""
  }</span></div>`;

const card = (row) => {
  const { palette } = row;
  return `
  <article class="card">
    <div class="cover"><img src="${row.thumb}" alt=""></div>
    <div class="meta">
      <h3>${row.label}</h3>
      <code>${row.hash}</code>
    </div>
    <div class="preview" style="background:${palette.background}">
      <div class="glow" style="background:radial-gradient(ellipse at 18% 38%, ${
        palette.glow
      }, transparent 72%)"></div>
      <div class="record"><i style="background:${row.ringHex}"></i></div>
      <div class="surface-text">
        <p class="title" style="color:${palette.on}">夜曲</p>
        <p class="meta-line" style="color:${row.mutedHex}">周杰伦 · 十一月的萧邦</p>
        <p class="lyric" style="color:${palette.on}">一群嗜血的蚂蚁 被腐肉所吸引</p>
        <p class="lyric muted" style="color:${row.mutedHex}">我面无表情 看孤独的风景</p>
      </div>
    </div>
    <div class="swatches">
      ${swatch("tint", palette.tint, "唱片标签、封面色")}
      ${swatch("background", palette.background, "沉浸面基底")}
      ${swatch("glow", palette.glow, "角部晕光")}
      ${swatch("on", palette.on, "标题 / 歌词墨色")}
    </div>
    <dl class="stats">
      ${stat("纸面亮度", row.paperLuminance.toFixed(3))}
      ${stat("纸 / 墨对比", `${row.paperInk.toFixed(2)}:1`)}
      ${stat("弱化字 / 晕对比", `${row.mutedInkGlow.toFixed(2)}:1`)}
      ${stat("与主题回退的感知差", row.deltaFallback.toFixed(1))}
    </dl>
  </article>`;
};

const html = `<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<title>Echo 沉浸式封面取色 · 素白方案实测</title>
<style>
  :root { color-scheme: light; }
  * { box-sizing: border-box; }
  body { margin: 0; padding: 40px 32px 64px; background: #faf8f7; color: #241f1e;
         font: 14px/1.6 -apple-system, "SF Pro SC", "PingFang SC", system-ui, sans-serif; }
  header, .panel, .fallback, .grid { max-width: 1180px; margin-inline: auto; }
  header { margin-bottom: 28px; }
  h1 { margin: 0 0 8px; font-size: 26px; letter-spacing: -.01em; }
  .sub { margin: 0; color: #6b5f5d; }
  .panel { margin-bottom: 24px; padding: 20px 24px; border-radius: 14px;
           background: #fff; border: 1px solid #eadfdd; }
  .panel h2 { margin: 0 0 12px; font-size: 15px; }
  .panel p { margin: 0 0 8px; color: #4a4241; }
  .panel p:last-child { margin-bottom: 0; }
  .verdict { display: inline-flex; align-items: center; gap: 8px; padding: 6px 14px;
             border-radius: 999px; font-weight: 600; font-size: 13px; }
  .verdict.pass { background: #e7f5e9; color: #17612c; }
  .verdict.fail { background: #fdeaea; color: #8f1d1d; }
  .fails { margin: 12px 0 0; padding-left: 20px; color: #8f1d1d; }
  .worst { display: grid; grid-template-columns: repeat(auto-fit, minmax(190px, 1fr));
           gap: 16px; margin-top: 16px; padding-top: 16px; border-top: 1px solid #f0e7e5; }
  .worst dt { font-size: 11.5px; color: #8b7d7a; }
  .worst dd { margin: 3px 0 0; font-size: 19px; font-variant-numeric: tabular-nums; }
  .worst small { display: block; margin-top: 2px; font-size: 11.5px; color: #8b7d7a; }
  .fallback { display: grid; grid-template-columns: 260px 1fr; gap: 20px; align-items: center;
              margin-bottom: 32px; padding: 20px 24px; border-radius: 14px; background: #fff;
              border: 1px dashed #d9c9c6; }
  .fallback .preview { height: 120px; }
  .fallback h2 { margin: 0 0 6px; font-size: 15px; }
  .fallback p { margin: 0 0 4px; color: #4a4241; }
  .grid { display: grid; gap: 20px; grid-template-columns: repeat(auto-fill, minmax(360px, 1fr)); }
  .card { display: grid; grid-template-columns: 96px 1fr; grid-template-areas:
          "cover meta" "preview preview" "swatches swatches" "stats stats";
          gap: 14px; padding: 18px; border-radius: 14px; background: #fff; border: 1px solid #eadfdd; }
  .cover { grid-area: cover; }
  .cover img { width: 96px; height: 96px; border-radius: 8px; object-fit: cover; display: block;
               border: 1px solid #e6dbd9; }
  .meta { grid-area: meta; align-self: center; min-width: 0; }
  .meta h3 { margin: 0 0 4px; font-size: 15px; overflow-wrap: anywhere; }
  .meta code { font-size: 11.5px; color: #8b7d7a; font-family: ui-monospace, SFMono-Regular, monospace; }
  .preview { grid-area: preview; position: relative; height: 168px; border-radius: 10px;
             overflow: hidden; padding: 16px 18px; }
  .glow { position: absolute; inset: 0; }
  .record { position: absolute; right: -26px; bottom: -34px; width: 132px; height: 132px;
            border-radius: 50%; background: radial-gradient(circle at 34% 30%, #1c1e21, #08090a 62%);
            border: 1px solid rgba(255,255,255,.06); box-shadow: 0 10px 26px rgba(0,0,0,.22); }
  .record i { position: absolute; left: 50%; top: 50%; width: 42px; height: 42px;
              margin: -21px 0 0 -21px; border-radius: 50%; display: block; }
  .surface-text { position: relative; max-width: 62%; }
  .surface-text p { margin: 0 0 6px; }
  .title { font-size: 16px; font-weight: 600; letter-spacing: -.01em; }
  .meta-line { font-size: 11.5px; }
  .lyric { font-size: 13px; }
  .lyric.muted { margin-top: 10px; }
  .swatches { grid-area: swatches; display: grid; gap: 8px; }
  .swatch { display: flex; align-items: center; gap: 10px; font-size: 12px; color: #5a4f4d; }
  .swatch i { width: 28px; height: 28px; border-radius: 6px; border: 1px solid rgba(0,0,0,.10); flex: none; }
  .swatch span { display: flex; align-items: baseline; gap: 8px; flex-wrap: wrap; }
  .swatch code { font-family: ui-monospace, SFMono-Regular, monospace; color: #241f1e; }
  .swatch em { font-style: normal; color: #9d8f8c; }
  .stats { grid-area: stats; display: flex; flex-wrap: wrap; gap: 20px; margin: 2px 0 0;
           padding-top: 12px; border-top: 1px solid #f0e7e5; }
  .stats dt { font-size: 11.5px; color: #8b7d7a; }
  .stats dd { margin: 2px 0 0; font-size: 15px; font-variant-numeric: tabular-nums; }
  footer { max-width: 1180px; margin: 40px auto 0; color: #8b7d7a; font-size: 12px; }
  footer code { font-family: ui-monospace, SFMono-Regular, monospace; }
</style>
</head>
<body>
<header>
  <h1>沉浸式封面取色 · 素白方案实测</h1>
  <p class="sub">用 <code>paletteFromPixels</code> 的<b>当前实现</b>（直接 import 应用源码，非拷贝）喂进播放器实际缓存的
  <b>${rows.length}</b> 张封面，按应用真实的绘制方式重建沉浸面。</p>
</header>

<section class="panel">
  <h2>门禁结论</h2>
  <p><span class="verdict ${failures.length ? "fail" : "pass"}">${
    failures.length ? "✗ FAIL" : "✓ PASS"
  }</span>${
    failures.length
      ? ""
      : ` &nbsp;${rows.length} 张封面的每一对颜色都在阈值之内，与 <code>artworkPalette.test.ts</code> / <code>e2e A15</code> 的断言一致。`
  }</p>
  ${failures.length ? `<ul class="fails">${failures.map((f) => `<li>${f}</li>`).join("")}</ul>` : ""}
  <dl class="worst">
    <div>
      <dt>最低纸面亮度（要求 ≥ ${GATES.paperLuminance}）</dt>
      <dd>${worst.paperLuminance.paperLuminance.toFixed(3)}</dd>
      <small>${worst.paperLuminance.label} · ${worst.paperLuminance.palette.background}</small>
    </div>
    <div>
      <dt>最差纸 / 墨对比（要求 ≥ ${GATES.paperInk}:1 · AAA）</dt>
      <dd>${worst.paperInk.paperInk.toFixed(2)}:1</dd>
      <small>${worst.paperInk.label} · 墨 ${worst.paperInk.palette.on}</small>
    </div>
    <div>
      <dt>最差弱化字 / 晕对比（要求 ≥ ${GATES.mutedInkGlow}:1 · AA）</dt>
      <dd>${worst.mutedInkGlow.mutedInkGlow.toFixed(2)}:1</dd>
      <small>${worst.mutedInkGlow.label} · ${worst.mutedInkGlow.mutedHex} on ${
        worst.mutedInkGlow.palette.glow
      }</small>
    </div>
    <div>
      <dt>与主题回退的最小感知差</dt>
      <dd>${worst.deltaFallback.deltaFallback.toFixed(1)}</dd>
      <small>${worst.deltaFallback.label} · 越接近 0 越像"没取到封面"</small>
    </div>
  </dl>
</section>

<section class="panel">
  <h2>怎么读每张卡片</h2>
  <p><b>纸面亮度</b>：素白方案要求基底足够亮，<code>&lt; ${GATES.paperLuminance}</code> 即判失败（与 e2e A15 同一条线）。</p>
  <p><b>纸 / 墨对比</b>：标题与当前歌词行用的 <code>--player-on</code> 压在 <code>--player-background</code> 上。</p>
  <p><b>弱化字 / 晕对比</b>：非当前歌词行是 80% 的墨压在该处的角部晕光上，这是整个表面上最弱的一对，也是 AA 的真正瓶颈。</p>
  <p><b>与主题回退的感知差</b>（oklab 距离 ×100）：越大说明"背景真的跟着封面走"；接近 0 就说明这首的基底和"没有封面时的主题纸色"几乎一样。</p>
  <p>采样口径与运行时一致：封面缩到最长边 64px 后取像素。<code>sips</code> 解出的 BMP 不带 alpha，因此本报告不覆盖半透明 PNG 的 alpha 加权路径。</p>
</section>

<section class="fallback">
  <div class="preview" style="background:${themeFallback.background}">
    <div class="glow" style="background:radial-gradient(ellipse at 18% 38%, ${
      themeFallback.glow
    }, transparent 72%)"></div>
    <div class="surface-text"><p class="title" style="color:${
      themeFallback.on
    }">主题回退</p><p class="meta-line" style="color:${
      themeFallback.on
    }">没有封面 / 取色失败</p></div>
  </div>
  <div>
    <h2>主题回退（没有封面 / 取色失败时）</h2>
    <p><code>${themeFallback.background}</code> · 晕 <code>${themeFallback.glow}</code> · 墨 <code>${themeFallback.on}</code></p>
    <p>由 <code>color-mix(in oklab, var(--accent), var(--bg) ${Math.round(
      THEME_FALLBACK_WEIGHT_BACKGROUND * 100,
    )}%)</code> 从珊瑚主色算得 —— 与封面取色<b>同一亮度档</b>，所以"有封面"和"没封面"只差色相。</p>
  </div>
</section>

<section class="grid">
${rows.map(card).join("\n")}
</section>

<footer>
  <p>由 <code>node --experimental-strip-types scripts/diagnostics/artwork-palette-report.mjs</code> 生成
  · 输入 <code>~/Library/Application Support/org.echo-player.echo/{covers,echo.sqlite}</code>
  · 只读，除 <code>artifacts/</code> 外不写任何文件。</p>
  ${skipped.length ? `<p>跳过 ${skipped.length} 项：${skipped.map(([h, why]) => `${h.slice(0, 10)} (${why})`).join("、")}</p>` : ""}
</footer>
</body>
</html>`;

mkdirSync(dirname(outFile), { recursive: true });
writeFileSync(outFile, html);

console.log(`wrote ${outFile}`);
console.log(`covers: ${rows.length}${skipped.length ? `, skipped: ${skipped.length}` : ""}`);
console.log(
  `worst paper luminance ${worst.paperLuminance.paperLuminance.toFixed(3)} (>= ${GATES.paperLuminance})`,
);
console.log(`worst paper/ink ${worst.paperInk.paperInk.toFixed(2)}:1 (>= ${GATES.paperInk}:1)`);
console.log(
  `worst muted-ink/glow ${worst.mutedInkGlow.mutedInkGlow.toFixed(2)}:1 (>= ${GATES.mutedInkGlow}:1)`,
);
console.log(`verdict: ${failures.length ? `FAIL — ${failures.join("; ")}` : "PASS"}`);
