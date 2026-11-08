// Issue #133: the picked-cover preview must be *visible* in a real WebView.
//
// Extracted from run-e2e.mjs to keep that harness under the 1000-line scale
// gate (same as a9-* / a76-*). The original defect was a runtime surprise —
// the packaged CSP blocked `blob:` — so a jsdom assertion on `src` is not
// enough: this module checks the shipped CSP on the Node side and decodes the
// returned `data:` JPEG in Chromium.

import { readFileSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const APP = resolve(fileURLToPath(new URL(".", import.meta.url)), "..");

/**
 * Assert the packaged CSP and the rendered picked-cover preview.
 *
 * @param {{
 *   goto: (seed: string) => Promise<void>,
 *   assert: (condExpr: string, msg: string) => Promise<void>,
 *   evalJs: (expr: string) => Promise<unknown>,
 *   pass: (msg: string) => void,
 *   fail: (msg: string) => void,
 * }} t
 */
export async function assertCoverPreviewScenarios({ goto, assert, evalJs, pass, fail }) {
  const csp = JSON.parse(readFileSync(join(APP, "src-tauri", "tauri.conf.json"), "utf8")).app
    .security.csp;
  const imgSrc = csp
    .split(";")
    .map((part) => part.trim())
    .find((part) => part.startsWith("img-src "));
  if (!imgSrc || !/\bdata:/.test(imgSrc) || /\bblob:/.test(imgSrc)) {
    fail("#133: packaged img-src must allow data: without widening to blob:");
  }
  pass("#133: packaged CSP allows data: preview images and stays blob:-free");

  await goto("medium");
  const opened = await evalJs(`(() => {
    // Match the actual sidebar item, not an ancestor whose innerText also
    // happens to contain the playlist name (e.g. the shell container).
    const entry = Array.from(document.querySelectorAll('button.playlist-item'))
      .find((el) => (el.innerText || '').includes('我的歌单'));
    if (!entry) return false;
    entry.click();
    return true;
  })()`);
  if (!opened) fail("#133: sidebar playlist entry is missing");
  await assert(
    `!!document.querySelector('[data-testid="playlist-view"]')`,
    "#133: playlist view did not open",
  );

  const edited = await evalJs(`(() => {
    const button = Array.from(document.querySelectorAll('button'))
      .find((el) => (el.innerText || '').includes('编辑歌单'));
    if (!button) return false;
    button.click();
    return true;
  })()`);
  if (!edited) fail("#133: 编辑歌单 entry is missing");
  await assert(
    `!!document.querySelector('.playlist-name-dialog input[type="file"]')`,
    "#133: edit dialog did not open",
  );

  await evalJs(`(() => {
    const input = document.querySelector('.playlist-name-dialog input[type="file"]');
    const file = new File([new Uint8Array([1, 2, 3, 4])], 'cover.png', { type: 'image/png' });
    const transfer = new DataTransfer();
    transfer.items.add(file);
    input.files = transfer.files;
    input.dispatchEvent(new Event('change', { bubbles: true }));
    return true;
  })()`);
  await assert(
    `(() => {
      const img = document.querySelector('.playlist-name-dialog img');
      return !!img && img.src.startsWith('data:image/jpeg;base64,') && img.naturalWidth > 0;
    })()`,
    "#133: picked cover preview did not render as a bounded data: JPEG",
  );
  await assert(
    `(() => {
      const payload = window.__echoE2E__.state.coverPreviewPayloads[0];
      return !!payload && typeof payload.bytes === 'string' &&
        !Array.isArray(payload.bytes) && payload.mime === 'image/png';
    })()`,
    "#133: preview_cover did not receive a base64 wire payload",
  );
  pass("#133: picked cover preview renders a decoded data: JPEG in Chromium");
}
