// A9 extension (issue #90): 歌词专注阅读 is the one state where the metadata
// block has **no rendered box at all** (`responsive.css` hides it at every
// width), which is precisely the input the lyrics-offset measurement must not
// turn into a result. The focus round trip therefore re-asserts the geometry on
// the way back out, not only that the surface rearranged.
//
// Extracted from run-e2e.mjs to keep that harness under the 1000-line scale
// gate, same as a9-lyrics-offset.mjs — which also owns the shared gap invariant.

import { assertLyricsGapSettles } from "./a9-lyrics-offset.mjs";

/**
 * Enter 歌词专注阅读, assert the surface rearranged, leave it, and assert the
 * lyrics still clear the song metadata.
 *
 * @param {{
 *   evalJs: (expr: string) => Promise<any>,
 *   waitFor: (expr: string, timeout?: number) => Promise<boolean>,
 *   assert: (condExpr: string, msg: string) => Promise<void>,
 *   sleep: (ms: number) => Promise<void>,
 *   fail: (msg: string) => void,
 *   playing: string,
 * }} t
 */
export async function assertLyricsFocusRoundTrip({
  evalJs,
  waitFor,
  assert,
  sleep,
  fail,
  playing,
}) {
  const entered = await waitFor(`(() => {
    const block = document.querySelector('[data-testid="immersive"] .lyrics');
    if (!block) return null;
    if (!(block.getAttribute('aria-label') || '').includes('展开歌词专注阅读')) return null;
    block.click();
    return true;
  })()`);
  if (!entered) fail("A9: the lyrics area is not an activatable 歌词专注阅读 entry");

  // Wait for the mode to be *painted*, not merely requested: `lyrics-focus-open`
  // lands on the body in a passive effect (after commit) and this WebView needs a
  // frame or two more before the rule shows up in computed style — the same settle
  // the offset gate waits for. Reading in between samples the non-focus surface
  // and reports the padding and the metadata as wrong.
  const painted = await waitFor(`(() => {
    if (!document.body.classList.contains('lyrics-focus-open')) return null;
    const popover = document.querySelector('[data-testid="immersive"]');
    const info = document.querySelector('[data-testid="immersive"] .now-playing-info');
    if (!popover || !info) return null;
    if (getComputedStyle(info).display !== 'none') return null;
    if (parseFloat(getComputedStyle(popover).paddingBottom) < 100) return null;
    return true;
  })()`);
  if (!painted)
    fail(
      "A9: 歌词专注阅读 never gave the surface to the lyrics (the metadata stayed painted, or the 常驻播放栏 lost its room)",
    );

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
  if (layout.labelShown) fail("A9: a lyrics label is painted in focus mode");
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

  // Focus owns the column's margin (`margin: 0 auto`), and the measurement has
  // to stand down while it does: an inline margin written from a metadata block
  // that is `display: none` beats that rule and drags the focused column down.
  const inFocus = await evalJs(`(() => {
    const lyrics = document.querySelector('[data-testid="immersive"] .lyrics');
    if (!lyrics) return null;
    return {
      top: Math.round(lyrics.getBoundingClientRect().top),
      inlineMargin: /(^|;)\\s*margin-top/.test(lyrics.getAttribute('style') || ''),
    };
  })()`);
  if (!inFocus) fail("A9: the lyrics column disappeared in focus mode");
  if (inFocus.inlineMargin)
    fail(
      "A9: the lyrics kept an inline margin-top through 歌词专注阅读 — the measurement did not stand down",
    );

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

  // Coming back out is where issue #90 landed: the metadata block goes from no
  // box at all to a real one, and a measurement that ran in between reads zeros
  // and clamps them into a margin of the gap itself.
  await assertLyricsGapSettles("leaving 歌词专注阅读", { evalJs, waitFor, sleep, fail });
}
