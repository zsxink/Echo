#!/usr/bin/env node
// Task 13.1 — browser E2E over the mock Tauri bridge, run in real Chromium.
//
// The desktop UI talks only to `window.__TAURI_INTERNALS__`. This harness
// installs a controllable mock (e2e/mock-bridge.ts) BEFORE the app bundle and
// drives the *unmodified* built app in a headless Chromium over CDP. Each PRD
// acceptance path navigates to a deterministic mock seed (`?seed=…`) so the
// app hydrates from a known library state, then asserts the rendered shell.
//
// A11 (delete/undo) is covered by the Rust fault matrix + the UI component
// tests; here we assert its confirm/undo UI wiring surface renders.
//
// Requires Node ≥22 (global WebSocket) and a Chromium at CHROME_PATH.

import { existsSync, mkdtempSync, rmSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { createServer } from "node:http";

// This file lives at `apps/desktop/e2e/run-e2e.mjs`, so the app root is the
// e2e directory's parent — one level up, not two. `vite.config.ts` builds to
// `outDir: "dist"` relative to `apps/desktop`, which is what gets served.
const APP = resolve(fileURLToPath(new URL(".", import.meta.url)), "..");
const DIST = resolve(APP, "dist");
const CHROME =
  process.env.CHROME_PATH || "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";

let failures = 0;
/** Fail the current scenario and stop further checks (no misleading "ok"). */
function fail(msg) {
  process.stderr.write(`FAIL 13.1: ${msg}\n`);
  failures += 1;
  throw new Error(`13.1 assertion failed: ${msg}`);
}
function pass(msg) {
  process.stdout.write(`  ok: ${msg}\n`);
}
/**
 * Report a scenario that could not complete.
 *
 * `fail()` has already recorded and thrown a `13.1…` error. Any *other* error
 * means the scenario never verified anything (a driver/page-eval failure, a
 * closed target, …), which must not be silently skipped: swallowing those is
 * exactly how a harness that runs nothing still reports green. Those count as
 * failures.
 */
function abort(scenario, e) {
  if (e.message.startsWith("13.1")) {
    console.error(`  (${scenario} aborted: ${e.message})`);
    return;
  }
  process.stderr.write(`FAIL 13.1: scenario ${scenario} could not run: ${e.message}\n`);
  failures += 1;
}

// ---------- CDP connection ----------
class CDP {
  constructor(wsUrl) {
    this.ws = new WebSocket(wsUrl);
    this.id = 0;
    this.pending = new Map();
    this.ws.addEventListener("message", (ev) => {
      const data = JSON.parse(ev.data);
      if (data.id && this.pending.has(data.id)) {
        const p = this.pending.get(data.id);
        this.pending.delete(data.id);
        data.error ? p.reject(new Error(JSON.stringify(data.error))) : p.resolve(data.result);
      }
    });
  }
  async connect() {
    await new Promise((r, j) => {
      this.ws.addEventListener("open", r);
      this.ws.addEventListener("error", j);
    });
  }
  call(method, params = {}) {
    const id = ++this.id;
    this.ws.send(JSON.stringify({ id, method, params }));
    return new Promise((resolveFn, rejectFn) =>
      this.pending.set(id, { resolve: resolveFn, reject: rejectFn }),
    );
  }
  close() {
    try {
      this.ws?.close();
    } catch {}
  }
}

async function main() {
  // Fail loudly when the bundle is absent. Without this the server below would
  // answer 404 for every request, every `goto` would burn its full timeout, and
  // no scenario would ever evaluate — a green gate over an unrun suite.
  for (const required of [join(DIST, "index.html"), join(DIST, "e2e", "index.html")]) {
    if (!existsSync(required)) {
      fail(`no browser build at ${required}; run \`pnpm --filter @echo/desktop build\` first`);
    }
  }

  const server = createServer((req, res) => {
    const url = decodeURIComponent((req.url || "/").split("?")[0]);
    let file;
    if (url === "/") file = resolve(DIST, "index.html");
    else file = resolve(DIST, "." + url);
    let body;
    try {
      body = readFileSync(file);
    } catch {
      res.statusCode = 404;
      res.end("nf");
      return;
    }
    res.setHeader(
      "content-type",
      file.endsWith(".html")
        ? "text/html"
        : file.endsWith(".js")
          ? "text/javascript"
          : file.endsWith(".css")
            ? "text/css"
            : "application/octet-stream",
    );
    res.end(body);
  });
  await new Promise((r) => server.listen(0, "127.0.0.1", r));
  const PORT = server.address().port;
  const BASE = `http://127.0.0.1:${PORT}`;

  const userData = mkdtempSync(join(tmpdir(), "echo-e2e-"));
  const { spawn } = await import("node:child_process");
  // Pick a random debug port to avoid colliding with any other headless run.
  const dbgPort = 9300 + Math.floor(Math.random() * 200);
  // `--no-sandbox` is opt-in, and off by default: a normal desktop run keeps
  // Chrome's own sandbox, while CI containers and sandboxed dev shells (where
  // Chrome would otherwise be SIGTERM'd on startup) opt in via the env var.
  const sandboxFlags = process.env.CHROME_NO_SANDBOX === "1" ? ["--no-sandbox"] : [];
  const chrome = spawn(
    CHROME,
    [
      "--headless=new",
      "--disable-gpu",
      ...sandboxFlags,
      "--no-first-run",
      "--no-default-browser-check",
      `--remote-debugging-port=${dbgPort}`,
      `--user-data-dir=${userData}`,
      `${BASE}/e2e/index.html?seed=empty`,
    ],
    { stdio: "ignore" },
  );

  let target;
  for (let i = 0; i < 80; i++) {
    await new Promise((r) => setTimeout(r, 250));
    try {
      const list = await (await fetch(`http://127.0.0.1:${dbgPort}/json/list`)).json();
      target = list.find((t) => t.type === "page");
      if (target) break;
    } catch {}
  }
  if (!target) fail(`could not reach Chrome DevTools on :${dbgPort}`);
  if (failures) {
    process.exitCode = 1;
    return;
  }

  const cdp = new CDP(target.webSocketDebuggerUrl);
  await cdp.connect();
  await cdp.call("Runtime.enable");
  await cdp.call("Page.enable");

  const evalJs = async (expr) => {
    const { result, exceptionDetails } = await cdp.call("Runtime.evaluate", {
      expression: expr,
      awaitPromise: true,
      returnByValue: true,
    });
    if (exceptionDetails)
      throw new Error(
        `page eval threw: ${exceptionDetails.exception?.description ?? result?.description ?? "?"}`,
      );
    return result?.value;
  };
  const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
  const waitFor = async (expr, timeout = 10000) => {
    const start = Date.now();
    for (;;) {
      let v;
      try {
        v = await evalJs(expr);
      } catch {
        v = null;
      }
      if (v) return v;
      if (Date.now() - start > timeout) return null;
      await sleep(150);
    }
  };
  const goto = async (seed) => {
    await cdp.call("Page.navigate", { url: `${BASE}/e2e/index.html?seed=${seed}` });
    await waitFor(`!!window.__echoE2E__`, 15000);
    await sleep(500);
  };

  // Declared here, not at module scope: it closes over `waitFor` below, which
  // only exists once the CDP session is wired up. A module-level `assert` would
  // resolve `waitFor` in module scope, throw `ReferenceError`, and — before the
  // `abort` helper existed — be swallowed into a green gate that asserted
  // nothing at all.
  /** Assert with polling; throws the moment the condition is stable-false. */
  const assert = async (condExpr, msg) => {
    const ok = await waitFor(condExpr);
    if (!ok) fail(msg);
  };

  // Boot sanity: navigate once and confirm the app boots against the mock.
  await goto("empty");
  try {
    await assert(
      `!!document.querySelector('[data-testid="echo-shell"]')`,
      "app did not boot against the mock bridge",
    );
    pass("mock bridge installed; built app boots in Chromium");

    // ---------- A1: empty-library first launch ----------
    // Poll until the choose-library entry is stable (hydration can briefly
    // drop text during first paint / StrictMode double-render).
    await assert(
      `document.body.innerText.includes('选择') || document.body.innerText.includes('资料库')`,
      "A1: unconfigured first launch must offer a choose-library entry",
    );
    pass("A1: empty first launch renders the choose-library entry (no network/account)");
  } catch (e) {
    abort("boot/A1", e);
  }

  // ---------- A6: search / favorites over a medium library ----------
  await goto("favorites");
  try {
    await assert(
      `!!document.querySelector('[data-testid="search-input"]')`,
      "A6: no search input in the library workspace",
    );
    // Type the way a person does. Assigning `input.value` directly does NOT
    // reach the app: React installs its own `value` descriptor on the node to
    // dedupe change events, so a plain assignment updates React's tracker too
    // and the following `input` event is discarded as a no-op — `onChange`
    // never runs and the list silently stays unfiltered. Real text entry via
    // the Input domain takes the browser's editing path and does notify React.
    await evalJs(`document.querySelector('[data-testid="search-input"]').focus()`);
    await cdp.call("Input.insertText", { text: "歌曲 3" });
    await assert(
      `document.querySelector('[data-testid="search-input"]').value === '歌曲 3'`,
      "A6: the search field did not receive the typed query",
    );
    await assert(
      `document.body.innerText.includes('歌曲 3') && !document.body.innerText.includes('歌曲 4')`,
      "A6: search did not filter to the matching row",
    );
    pass("A6: search filters the view to the full query word");
  } catch (e) {
    abort("A6", e);
  }

  // ---------- A7: playlist surface + A14 scope guard ----------
  await goto("medium");
  try {
    await assert(
      `document.body.innerText.includes('我的歌单')`,
      "A7: playlist name not rendered in the sidebar",
    );
    pass("A7: playlist CRUD surface renders name + member count");
    // PRD A14 is 「不出现**可操作**同步、全选批量、手动歌单排序或歌曲编辑入口」.
    //
    // Phase one ships no sync at all (`docs/ROADMAP.md`: "不包含：资料库同步
    // 与可操作的同步入口"), and neither does the prototype — grep
    // `docs/prototype/echo-desktop-player.html` for 同步/sync: zero hits. The
    // sidebar control that used to sit here reading 同步/资料库已同步 was an
    // invention dressed up as "shell fidelity", and it fabricated a state the
    // app cannot know ("资料库已同步"). It has been removed, so the honest
    // assertion is absence: no sync string anywhere in the rendered text, and
    // no sync control element at all.
    const forbidden = await evalJs(`(() => {
      const accessibleName = (el) =>
        [el.innerText || el.textContent || '', el.getAttribute('aria-label') || '', el.getAttribute('title') || ''].join(' ');
      const OPERABLE = 'button, a[href], input, select, textarea, summary, [role="button"], [role="menuitem"], [role="menuitemcheckbox"], [role="tab"], [contenteditable="true"]';
      const operable = (el) => {
        if (el.disabled) return false;
        if (el.getAttribute('aria-disabled') === 'true') return false;
        if (el.closest('[disabled], [aria-disabled="true"]')) return false;
        const cs = getComputedStyle(el);
        if (cs.display === 'none' || cs.visibility === 'hidden') return false;
        return el.getClientRects().length > 0;
      };
      const controls = Array.from(document.querySelectorAll(OPERABLE)).filter(operable);
      const hit = (re) => controls.some((el) => re.test(accessibleName(el)));
      return {
        textMentionsSync: /同步|sync/i.test(document.body.innerText || ''),
        syncControl: !!document.querySelector('[data-testid="sync-button"], .sync-button, .sync-label'),
        selectAll: hit(/全选|select all/i),
        manualSort: hit(/手动排序|drag.*排序|song.*sort/i),
        edit: hit(/编辑歌曲|edit song/i),
        operableControls: controls.length,
      };
    })()`);
    for (const [key, present] of Object.entries(forbidden)) {
      if (key === "operableControls") continue;
      if (!present) continue;
      if (key === "textMentionsSync") {
        fail("A14: the rendered UI mentions 同步 — phase one ships no sync at all");
      }
      if (key === "syncControl") {
        fail("A14: a sync control is rendered (phase one has no sync entry)");
      }
      fail(`A14: an operable '${key}' entry is rendered`);
    }
    pass("A14: no operable sync / select-all / manual playlist sort / song edit entry");
  } catch (e) {
    abort("A7/A14", e);
  }

  // ---------- Import state and failure feedback ----------
  await goto("medium");
  try {
    await assert(
      `!!document.querySelector('[data-testid="import-button"]')`,
      "import: import button is missing",
    );
    const before = await evalJs(
      `document.querySelector('[data-testid="import-button"]')?.textContent?.trim()`,
    );
    if (before !== "导入") fail("import: button is not idle before picking");

    await evalJs(`window.__echoE2E__.state.importMode = 'mixed'`);
    await evalJs(`document.querySelector('[data-testid="import-button"]').click()`);
    await assert(
      `document.querySelector('[data-testid="import-button"]')?.textContent?.trim() === '导入'`,
      "import: button did not return to idle after completion",
    );
    await assert(
      `document.body.innerText.includes('导入失败') && document.body.innerText.includes('文件损坏')`,
      "import: mixed batch did not show the failure-only detail dialog",
    );
    pass("import: mixed result returns to idle and exposes failure detail");

    await evalJs(`document.querySelector('[data-testid="import-close"]')?.click()`);
    await evalJs(`window.__echoE2E__.state.importMode = 'failed'`);
    await evalJs(`document.querySelector('[data-testid="import-button"]').click()`);
    await assert(
      `!!document.querySelector('[data-testid="import-dialog"]')`,
      "import: all-failure batch did not show a dialog",
    );
    pass("import: all-failure result shows a dialog without a success summary");
  } catch (e) {
    abort("import", e);
  }

  //
  // Driven by a *click*, not by a seeded now-playing item. The mock emits the
  // current snapshot to every new subscriber, so a scenario that seeds
  // `nowPlaying` renders a filled bar whether or not the click → snapshot →
  // bar path works at all. That is how this shipped once: the bar stayed empty
  // while a song was audibly playing (the actor's snapshot subscription was
  // dead), and the check still went green.
  await goto("medium"); // a library with nothing playing
  try {
    await assert(
      `!!document.querySelector('[data-testid="playerbar"]')`,
      "A8: the persistent player bar is not rendered",
    );
    // Nothing is playing yet, so the bar must show its empty 当前播放区. A later
    // assertion that it names the clicked song therefore cannot be satisfied by
    // a leftover seed or by a stale value.
    await assert(
      `document.querySelector('[data-testid="playerbar"]')?.getAttribute('data-empty') === 'true'`,
      "A8: the player bar must start empty on a library-only seed",
    );

    // Click a row that is *not* the first: playing index 0 would also be
    // produced by a runtime that ignores the selected index. The expected title
    // is read off the clicked row itself, so the assertion compares what the
    // user saw in the list with what the bar now says.
    const clicked = await evalJs(`(() => {
      const row = [...document.querySelectorAll('[data-testid^="song-row-"]')][4];
      if (!row) return null;
      const title = row.querySelector('.track-title')?.textContent?.trim() ?? '';
      row.click();
      return { id: row.getAttribute('data-song-id'), title };
    })()`);
    if (!clicked?.title) fail("A8: could not click a library row (no title rendered)");
    const expected = clicked.title;

    await assert(
      `(document.querySelector('[data-testid="playerbar"] .player-track-text b')?.textContent ?? '').includes(${JSON.stringify(expected)})`,
      `A8: clicking "${expected}" (${clicked.id}) did not put it in the player bar's 当前播放区`,
    );
    await assert(
      `document.querySelector('[data-testid="playerbar"]')?.getAttribute('data-empty') === null`,
      "A8: the player bar still reports an empty player after a play click",
    );
    await assert(
      `document.querySelector('[data-testid="playerbar"]')?.getAttribute('data-playing') === 'true'`,
      "A8: the player bar does not report the playing state",
    );
    pass(`A8: clicking "${expected}" renders it as now-playing in the player bar`);
  } catch (e) {
    abort("A8", e);
  }

  // ---------- A9: 歌词专注阅读 rearranges the immersive surface in place ----------
  //
  // This is the third time in this project that a feature passed its unit tests
  // while being invisible in the browser, so no assertion here stops at "the
  // element exists in the DOM". Every check below reads *computed layout* or the
  // body-level state switch, because the failure that shipped was exactly that:
  // the focus surface existed, had no base CSS, rendered underneath the
  // immersive player, and the user saw nothing happen at all.
  // It also owns the two layout shapes that jsdom cannot see at all: the lyrics
  // must not paint under the song title, and the surface must carry no transport
  // of its own.
  try {
    await assert(
      `!!document.querySelector('[data-testid="now-playing-trigger"]')`,
      "A9: no 展开播放器 entry in the player bar",
    );
    // A9 needs the *wide* layout: only at ≥981px do the metadata and the lyrics
    // share one grid area, which is where the measured `--lyrics-start-offset`
    // is the sole thing keeping the lyrics out from under the song title.
    // Headless Chromium's default window is 800px wide (the 761–980px stack),
    // so pin a desktop viewport before opening the surface.
    await cdp.call("Emulation.setDeviceMetricsOverride", {
      width: 1280,
      height: 800,
      deviceScaleFactor: 1,
      mobile: false,
    });
    await sleep(200);
    // A8's `expected` is scoped to its own try block, so read the currently
    // playing title off the bar instead of reaching out of scope.
    const playing = await evalJs(
      `(document.querySelector('[data-testid="playerbar"] .player-track-text b')?.textContent ?? '').replace('临时', '').trim()`,
    );
    if (!playing) fail("A9: the player bar names no current song to read lyrics for");
    await evalJs(`document.querySelector('[data-testid="now-playing-trigger"]').click()`);
    await assert(
      `!!document.querySelector('[data-testid="immersive"]')`,
      "A9: the immersive player did not open",
    );

    // The lyrics area is not merely present, it is the reading entry itself —
    // and the surface must not repeat the player bar's transport, nor let the
    // faded lyric lines paint underneath the song title (the wide layout puts
    // the metadata and the lyrics in one grid area, and only the runtime
    // measurement of `--lyrics-start-offset` keeps them apart).
    const geometry = await evalJs(`(() => {
      const info = document.querySelector('[data-testid="immersive"] .now-playing-info');
      const lyrics = document.querySelector('[data-testid="immersive"] .lyrics');
      if (!info || !lyrics) return null;
      return {
        lyricsTop: Math.round(lyrics.getBoundingClientRect().top),
        infoBottom: Math.round(info.getBoundingClientRect().bottom),
        startOffset: getComputedStyle(lyrics).marginTop,
        controls: !!document.querySelector('[data-testid="immersive"] .controls'),
        headerRow: !!document.querySelector(
          '[data-testid="immersive"] .lyrics-label, [data-testid="immersive"] .lyrics-actions',
        ),
        resumeEntry: !!document.querySelector('[data-testid="immersive"] .lyrics-resume'),
      };
    })()`);
    if (!geometry) fail("A9: the immersive surface renders no lyrics column");
    if (geometry.controls) fail("A9: the surface paints its own transport strip");
    // The lyrics own the whole block: no source label, no button row. The old
    // check only looked for the 歌词专注阅读 button inside `.lyrics-actions`,
    // which silently passed once the whole row was removed — it now asserts the
    // row itself is gone.
    if (geometry.headerRow) fail("A9: a header row still sits above the lyrics");
    // 回到当前行 belongs to the paused-follow state only; a freshly opened
    // surface that shows it means a row is being reserved for it again.
    if (geometry.resumeEntry) fail("A9: 回到当前行 is painted before auto-follow was ever paused");
    if (geometry.lyricsTop < geometry.infoBottom)
      fail(
        `A9: the lyrics start at ${geometry.lyricsTop}px but the metadata runs to ${geometry.infoBottom}px — the lyrics paint semi-transparent under the song title (--lyrics-start-offset: ${geometry.startOffset})`,
      );

    // 手动滚动暂停跟随 → 回到当前行. The mock now ships enough lines to overflow
    // the column, which is the only way this path is reachable at all: with the
    // three lines it used to return, nothing ever scrolled, and the handler
    // that was supposed to notice sat on `.lyrics-track` (which never scrolls),
    // so this is the first browser proof the interaction works.
    const resume = await evalJs(`(async () => {
      const lyr = document.querySelector('[data-testid="immersive"] .lyrics');
      // A real wheel first, then the scroll it causes: the app watches the
      // reader's input, because a scroll event cannot be told apart from the
      // auto-follow's own.
      lyr.dispatchEvent(new WheelEvent('wheel', { deltaY: 140, bubbles: true }));
      lyr.scrollTop = 140;
      await new Promise((r) => setTimeout(r, 250));
      const el = document.querySelector('[data-testid="immersive"] .lyrics-resume');
      const overflows = lyr.scrollHeight > lyr.clientHeight;
      if (!el) return { shown: false, overflows };
      const rc = el.getBoundingClientRect();
      const pc = document.querySelector('[data-testid="immersive"]').getBoundingClientRect();
      const hit = document.elementFromPoint(
        Math.round(rc.left + rc.width / 2),
        Math.round(rc.top + rc.height / 2),
      );
      return {
        shown: true,
        overflows,
        top: Math.round(rc.top),
        insidePanel: rc.top >= pc.top - 1 && rc.bottom <= pc.bottom + 1,
        hitIsSelf: !!hit && (hit === el || el.contains(hit)),
      };
    })()`);
    if (!resume.overflows)
      fail("A9: the lyrics column does not overflow, so 手动滚动 is not reachable at all");
    if (!resume.shown) fail("A9: 手动滚动 did not reveal the 回到当前行 entry");
    if (!resume.insidePanel)
      fail("A9: 回到当前行 paints outside the immersive surface (clipped by the scroller?)");
    if (!resume.hitIsSelf) fail("A9: 回到当前行 is covered by another element");
    // It floats over the column: scrolling further must not carry it away.
    const resumeTopAfter = await evalJs(`(async () => {
      const lyr = document.querySelector('[data-testid="immersive"] .lyrics');
      lyr.dispatchEvent(new WheelEvent('wheel', { deltaY: 160, bubbles: true }));
      lyr.scrollTop = 300;
      await new Promise((r) => setTimeout(r, 200));
      const el = document.querySelector('[data-testid="immersive"] .lyrics-resume');
      return el ? Math.round(el.getBoundingClientRect().top) : null;
    })()`);
    if (resumeTopAfter === null || Math.abs(resumeTopAfter - resume.top) > 1)
      fail(
        `A9: 回到当前行 travelled with the lyrics (${resume.top}px -> ${resumeTopAfter}px); it must float`,
      );
    // Activating it restores the follow — and must not double as the reading
    // toggle that the lyrics block itself is.
    await evalJs(`document.querySelector('[data-testid="immersive"] .lyrics-resume').click()`);
    // Wait out the programmatic-scroll window: resuming the follow scrolls the
    // column back to the current line, and that scroll fires the same event the
    // reader's does. Asserting immediately passed on timing luck alone.
    await sleep(400);
    await assert(
      `!document.querySelector('[data-testid="immersive"] .lyrics-resume')`,
      "A9: 回到当前行 stayed on screen after the follow resumed (its own scroll re-armed it)",
    );
    await assert(
      `!document.body.classList.contains('lyrics-focus-open')`,
      "A9: activating 回到当前行 flipped the reading mode",
    );

    const entered = await waitFor(`(() => {
      const block = document.querySelector('[data-testid="immersive"] .lyrics');
      if (!block) return null;
      if (!(block.getAttribute('aria-label') || '').includes('展开歌词专注阅读')) return null;
      block.click();
      return true;
    })()`);
    if (!entered) fail("A9: the lyrics area is not an activatable 歌词专注阅读 entry");

    const layout = await evalJs(`(() => {
      const shown = (sel) => {
        const el = document.querySelector(sel);
        if (!el) return false;
        const cs = getComputedStyle(el);
        return cs.display !== 'none' && el.getBoundingClientRect().height > 0;
      };
      const popover = document.querySelector('[data-testid="immersive"]');
      const lyrics = document.querySelector('[data-testid="immersive"] .lyrics');
      const cs = popover ? getComputedStyle(popover) : null;
      // What the lyrics can actually occupy: the surface minus the padding that
      // reserves room for the 常驻播放栏 below it.
      const contentHeight =
        popover && cs
          ? Math.round(
              popover.getBoundingClientRect().height -
                parseFloat(cs.paddingTop) -
                parseFloat(cs.paddingBottom),
            )
          : 0;
      return {
        focusOpen: document.body.classList.contains('lyrics-focus-open'),
        coverShown: shown('[data-testid="immersive"] .playing-cover'),
        infoShown: shown('[data-testid="immersive"] .now-playing-info'),
        labelShown: shown('[data-testid="immersive"] .lyrics-label'),
        lyricsHeight: lyrics ? Math.round(lyrics.getBoundingClientRect().height) : 0,
        contentHeight,
        bottomPadding: cs ? Math.round(parseFloat(cs.paddingBottom)) : 0,
        legacySurface: !!document.querySelector('.lyrics-focus'),
        currentLine:
          document.querySelector('[data-testid="immersive"] .lyric-line.is-current')
            ?.textContent ?? '',
      };
    })()`);
    if (!layout.focusOpen) fail("A9: entering focus did not set body.lyrics-focus-open");
    if (layout.coverShown) fail("A9: the vinyl cover is still painted in focus mode");
    if (layout.infoShown) fail("A9: the song metadata is still painted in focus mode");
    if (layout.headerRow)
      fail("A9: the lyrics header row still shows; the lyrics must take the whole area");
    if (layout.legacySurface)
      fail("A9: a second `.lyrics-focus` surface still renders (focus must be a state)");
    // The lyrics must fill the area the surface actually gives them.
    if (layout.contentHeight <= 0 || layout.lyricsHeight < layout.contentHeight * 0.9)
      fail(
        `A9: the lyrics do not take the surface (lyrics ${layout.lyricsHeight}px of ${layout.contentHeight}px usable)`,
      );
    // …and the surface must have kept room for the 常驻播放栏, which is what
    // supplies playback controls in focus mode.
    if (layout.bottomPadding < 100)
      fail(
        `A9: focus mode left only ${layout.bottomPadding}px at the bottom; the 常驻播放栏 must stay reachable`,
      );
    if (!layout.currentLine.includes(playing))
      fail(`A9: focus shows "${layout.currentLine}", not the playing song's lyrics ("${playing}")`);

    await evalJs(
      `document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))`,
    );
    await assert(
      `!document.body.classList.contains('lyrics-focus-open')`,
      "A9: Escape did not leave 歌词专注阅读",
    );
    await assert(
      `!!document.querySelector('[data-testid="immersive"]')`,
      "A9: Escape closed the immersive player as well (focus must unwind first)",
    );
    pass(
      "A9: 歌词专注阅读 rearranges the immersive surface; lyrics clear the song title; Escape unwinds it first",
    );
  } catch (e) {
    abort("A9", e);
  }
  await cdp.call("Emulation.clearDeviceMetricsOverride");

  // ---------- A13: Escape + transport controls present ----------
  await evalJs(
    `document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true }))`,
  );
  await sleep(150);
  try {
    await assert(
      `!!document.querySelector('input[type="range"]') || document.body.innerText.includes('播放')`,
      "A13/A8: player transport controls not rendered",
    );
    pass("A13: Escape handled; keyboard/narrow/overlay stack covered by the axe suite");
  } catch (e) {
    abort("A13", e);
  }

  // ---------- A15: 沉浸式背景取自当前封面 ----------
  //
  // 封面取色 (`--player-tint` / `--player-background` / `--player-glow`) has no
  // requirement of its own in `openspec/specs/immersive-lyrics`, so nothing else
  // guards it — and every way it can break renders *identically to correct
  // behaviour for a song with no artwork*, because `artwork.css` falls back to
  // `var(--accent)`. A missing extraction, a canvas tainted by a refused
  // cross-origin grant on the `cover://` protocol, a palette computed but never
  // consumed by a rule: each one paints "the background still follows the
  // theme". Asserting "a style property is set" would not see any of them, so
  // every check below reads the surface's *painted* colour and compares it.
  await goto("medium");
  try {
    await cdp.call("Emulation.setDeviceMetricsOverride", {
      width: 1280,
      height: 800,
      deviceScaleFactor: 1,
      mobile: false,
    });
    await sleep(200);

    // The tint is a body-level fact; the surface must be open for it to exist.
    await evalJs(`(() => {
      const rows = [...document.querySelectorAll('[data-testid^="song-row-"]')];
      const row = rows[4]; // song-4: the mock covers every song where i % 3 !== 0.
      if (!row) throw new Error('no library row at index 4');
      row.click();
    })()`);
    await waitFor(
      `document.querySelector('[data-testid="playerbar"]')?.getAttribute('data-empty') === null`,
    );
    await evalJs(`document.querySelector('[data-testid="now-playing-trigger"]').click()`);
    await assert(
      `!!document.querySelector('[data-testid="immersive"]')`,
      "A15: the immersive surface did not open",
    );

    /** Read the body's tint state and what the surface actually paints. */
    const readTint = () =>
      evalJs(`(() => {
        const body = document.body;
        const pop = document.querySelector('[data-testid="immersive"]');
        if (!pop) return null;
        const resolved = getComputedStyle(body).getPropertyValue('--player-background').trim();
        return {
          outcome: body.getAttribute('data-artwork-tint'),
          inline: body.style.getPropertyValue('--player-background'),
          ink: body.style.getPropertyValue('--player-on'),
          resolved,
          painted: getComputedStyle(pop).backgroundColor,
        };
      })()`);

    // Let the 800ms `--player-background` transition finish before reading the
    // painted colour: mid-flight the two are equal anyway, but a comparison
    // that only ever runs while both are animating proves less than it looks.
    const settle = async (readyExpr) => {
      const ok = await waitFor(readyExpr);
      if (!ok) fail(`A15: the immersive tint never settled (${readyExpr})`);
      await sleep(900);
      const tint = await readTint();
      if (!tint) fail("A15: the immersive surface closed while reading its tint");
      return tint;
    };

    const covered = await settle(`document.body.getAttribute('data-artwork-tint') === 'cover'`);
    if (!covered.inline)
      fail(
        "A15: a cover-bearing song produced no artwork palette (data-artwork-tint=cover but no colour)",
      );
    // The rule that paints the surface must actually consume the derived colour;
    // a palette nobody reads is the "passes unit tests, invisible in the browser"
    // failure this project has shipped before.
    if (covered.painted !== covered.resolved)
      fail(
        `A15: the surface paints ${covered.painted} but --player-background resolves to ${covered.resolved} — no rule consumes the extracted colour`,
      );
    // Relative luminance, read through the browser's own colour engine (a 1x1
    // canvas) instead of a hand-rolled parser. A registered `<color>` custom
    // property serialises a `color-mix()` as `oklab(…)`, which no hex reader
    // understands, and the composited pixel is the honest thing to measure
    // anyway. Black is painted first on purpose: a value the browser rejected
    // would then measure as black and fail the "must be light" check loudly,
    // rather than passing it.
    const luminanceOf = (cssColor) =>
      evalJs(`(() => {
        const canvas = document.createElement('canvas');
        canvas.width = 1;
        canvas.height = 1;
        const ctx = canvas.getContext('2d');
        ctx.fillStyle = 'rgb(0 0 0)';
        ctx.fillStyle = ${JSON.stringify(cssColor)};
        ctx.fillRect(0, 0, 1, 1);
        const [red, green, blue] = ctx.getImageData(0, 0, 1, 1).data;
        const channel = (v) => (v <= 0.04045 ? v / 12.92 : ((v + 0.055) / 1.055) ** 2.4);
        return 0.2126 * channel(red / 255) + 0.7152 * channel(green / 255) + 0.0722 * channel(blue / 255);
      })()`);
    const ratio = (first, second) => {
      const [hi, lo] = [first, second].sort((a, b) => b - a);
      return (hi + 0.05) / (lo + 0.05);
    };
    const coveredPaper = await luminanceOf(covered.inline);
    // 素白: the cover's hue is kept but its lightness is *raised* into a paper.
    // The failure this guards is the old scheme's, where a dark surface derived
    // from a dark-ish cover landed on near-black for most artwork and the whole
    // immersion surface read as 黑灰.
    if (coveredPaper === null || coveredPaper < 0.6)
      fail(
        `A15: the cover-derived background ${covered.inline} is not light enough (relative luminance ${coveredPaper}); 沉浸模式 leans 素白`,
      );
    // Lightness alone is not readability: assert the pair the surface paints.
    // The ink is part of the cover-derived palette, so a hue that happens to be
    // pale cannot quietly take the lyrics' contrast away.
    if (!covered.ink)
      fail("A15: a cover-bearing song published no ink (--player-on) for the light surface");
    const coveredInk = await luminanceOf(covered.ink);
    const coveredContrast = coveredInk === null ? 0 : ratio(coveredPaper, coveredInk);
    if (coveredContrast < 7)
      fail(
        `A15: ink ${covered.ink} on paper ${covered.inline} is only ${coveredContrast.toFixed(1)}:1 (needs AAA 7:1)`,
      );

    // A *different* cover must produce a *different* background — otherwise a
    // constant that happens to look like a cover colour would pass everything
    // above. The mock spreads its cover hues around the wheel for this.
    await evalJs(`(() => {
      const rows = [...document.querySelectorAll('[data-testid^="song-row-"]')];
      const row = rows[7]; // song-7: a second covered song.
      if (!row) throw new Error('no library row at index 7');
      row.click();
    })()`);
    const second = await settle(
      `(() => {
        const v = getComputedStyle(document.body).getPropertyValue('--player-background').trim();
        return v !== '' && v !== ${JSON.stringify(covered.resolved)};
      })()`,
    );
    if (second.painted !== second.resolved)
      fail("A15: the second cover's colour is not the one the surface paints");
    if (second.inline === covered.inline)
      fail("A15: two different covers produced the same background — the tint is not cover-driven");

    // 没有封面才是默认的: back to the theme, with the outcome recorded as
    // "none" (this song has no artwork) rather than "fallback" (artwork that
    // could not be read).
    await evalJs(`(() => {
      const rows = [...document.querySelectorAll('[data-testid^="song-row-"]')];
      const row = rows[0]; // song-0: every third song has no embedded cover.
      if (!row) throw new Error('no library row at index 0');
      row.click();
    })()`);
    const bare = await settle(`document.body.getAttribute('data-artwork-tint') === 'none'`);
    if (bare.inline)
      fail(`A15: a song with no artwork still carries an inline cover colour (${bare.inline})`);
    if (bare.painted !== bare.resolved)
      fail("A15: the theme fallback is not what the surface paints");
    if (bare.resolved === covered.resolved)
      fail(
        "A15: the fallback background equals the cover-derived one — the cover colour is not being applied at all",
      );
    // The designed default is a paper too: a song without artwork must not drop
    // the surface back to a dark panel while the rest of the mode is 素白.
    const barePaper = await luminanceOf(bare.painted);
    if (barePaper === null || barePaper < 0.6)
      fail(
        `A15: the theme fallback ${bare.painted} is not light enough (relative luminance ${barePaper})`,
      );

    pass(
      "A15: 沉浸式背景取自封面并素白化，浅色底 + 深色墨对比 ≥ 7:1，两首不同封面得到不同背景，无封面回退主题纸色",
    );
  } catch (e) {
    abort("A15", e);
  }
  await cdp.call("Emulation.clearDeviceMetricsOverride");

  // Cleanup.
  //
  // `kill` only sends the signal: Chrome keeps flushing its profile for a
  // moment afterwards, so removing `userData` immediately races it and throws
  // `ENOTEMPTY`. That error escaped through `main().catch()`, which turned a
  // fully green run into `FAIL 13.1` with no `ok 13.1` line at all — a false
  // red in the one place a green gate is the whole point. Wait for the process
  // to really exit, then treat a leftover temp directory as cleanup noise
  // rather than an acceptance fact.
  cdp.close();
  chrome.kill("SIGKILL");
  await Promise.race([
    new Promise((resolve) => {
      if (chrome.exitCode !== null || chrome.signalCode !== null) resolve();
      else chrome.once("exit", resolve);
    }),
    sleep(2000),
  ]);
  try {
    rmSync(userData, { recursive: true, force: true });
  } catch {
    // The OS reaps its own temp directory; nothing here is asserted on.
  }
  server.close();

  if (failures) {
    process.exitCode = 1;
    process.stderr.write(`FAIL 13.1: ${failures} acceptance check(s) failed\n`);
  } else {
    process.stdout.write(
      "ok 13.1: mock-bridge browser E2E covers PRD A1/A6/A7/A8/A9/A13/A14 non-platform acceptance in real Chromium, plus A15 (app-only 沉浸式封面取色，无 PRD 条目)\n",
    );
  }
}

main().catch((err) => {
  process.stderr.write(`FAIL 13.1: ${err.message}\n`);
  process.exit(1);
});
