// Issue #76: the Windows caption trio (最小化 / 最大化-还原 / 关闭).
//
// Extracted from run-e2e.mjs only to keep that harness under the 1000-line
// scale gate. Both branches of the platform gate are asserted through the
// `?platform=` forcing seam: `windows` renders the self-drawn buttons in the
// top-right strip and drives the window plugin (recorded by the mock), while
// the unset (web) platform renders nothing — macOS/Linux keep their native
// chrome.

/**
 * Assert the Windows window-controls branch and the non-Windows branch.
 *
 * @param {{
 *   goto: (seed: string) => Promise<void>,
 *   assert: (condExpr: string, msg: string) => Promise<void>,
 *   evalJs: (expr: string) => Promise<unknown>,
 *   pass: (msg: string) => void,
 * }} t
 */
export async function assertWindowControlsScenarios({ goto, assert, evalJs, pass }) {
  await goto("empty&platform=windows");
  await assert(
    `!!document.querySelector('[data-testid="window-controls"]')`,
    "#76: no window controls on the forced-Windows platform",
  );
  // The controls sit inside the top-right safe-area strip.
  await assert(
    `getComputedStyle(document.querySelector('[data-testid="window-controls"]')).position === 'fixed'`,
    "#76: window controls are not the fixed top-right caption strip",
  );
  // Minimize + close reach the window plugin (recorded, no real window).
  await evalJs(`document.querySelector('[data-testid="window-minimize"]').click()`);
  await evalJs(`document.querySelector('[data-testid="window-close"]').click()`);
  await assert(
    `window.__echoE2E__.state.windowActions.join(',') === 'minimize,close'`,
    "#76: minimize/close clicks did not reach the window plugin",
  );
  // Toggle flips the mock's maximized phase twice; the glyph must follow
  // the *window* state (via tauri://resize), reading 最大化 → 还原 → 最大化.
  await assert(
    `document.querySelector('[data-testid="window-toggle-maximize"]').getAttribute('aria-label') === '最大化'`,
    "#76: caption does not start in the 最大化 phase",
  );
  await evalJs(`document.querySelector('[data-testid="window-toggle-maximize"]').click()`);
  await assert(
    `document.querySelector('[data-testid="window-toggle-maximize"]').getAttribute('aria-label') === '还原'`,
    "#76: maximize toggle did not switch the glyph to 还原",
  );
  await evalJs(`document.querySelector('[data-testid="window-toggle-maximize"]').click()`);
  await assert(
    `document.querySelector('[data-testid="window-toggle-maximize"]').getAttribute('aria-label') === '最大化'`,
    "#76: second toggle did not restore the 最大化 glyph",
  );
  pass(
    "#76: Windows caption buttons render, drive minimize/close/toggle and track the window state",
  );

  await goto("empty");
  await assert(
    `!document.querySelector('[data-testid="window-controls"]')`,
    "#76: window controls leaked onto a non-Windows platform",
  );
  pass("#76: no caption buttons without the Windows platform (macOS/Linux/web stay native)");
}
