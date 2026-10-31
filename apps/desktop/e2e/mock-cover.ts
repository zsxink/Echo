/**
 * The artwork URL the mock answers a `cover://` request with.
 *
 * The real shell registers the `cover://` URI scheme and the WebView paints the
 * asset bytes directly. A browser loaded from a plain http origin cannot resolve
 * a custom scheme, so the mock answers the *same* call with an inline SVG whose
 * colour is derived from the key: the key still round-trips through the app
 * unchanged, and both artwork paths stay observable in the preview.
 */
export function coverAssetUrl(key: string): string {
  // Spread the hue around the whole wheel. The obvious `% 360` inside the fold
  // leaves every `cv1-mockN` key on the same value up to its last character
  // (the keys share a prefix), which parked every mock cover in a single
  // 10-degree band. The immersive tint is verified by *comparing* the
  // background two different covers produce, so a fixture that cannot express
  // two different covers would make that assertion vacuous.
  const hue =
    ([...key].reduce((acc, ch) => (acc * 31 + ch.charCodeAt(0)) % 1_000_003, 7) * 137) % 360;
  const svg =
    `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">` +
    `<defs><linearGradient id="g" x1="0" y1="0" x2="1" y2="1">` +
    `<stop offset="0" stop-color="hsl(${hue} 58% 62%)"/>` +
    `<stop offset="1" stop-color="hsl(${(hue + 46) % 360} 54% 34%)"/>` +
    `</linearGradient></defs>` +
    `<rect width="64" height="64" fill="url(#g)"/>` +
    `<circle cx="32" cy="32" r="11" fill="none" stroke="rgba(255,255,255,.55)" stroke-width="1.6"/>` +
    `<circle cx="32" cy="32" r="2.6" fill="rgba(255,255,255,.82)"/>` +
    `</svg>`;
  return `data:image/svg+xml;utf8,${encodeURIComponent(svg)}`;
}
