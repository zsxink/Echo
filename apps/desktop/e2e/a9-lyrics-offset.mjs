// A9 extension (issue #75): the immersive lyrics offset must survive a track
// change and re-derive from the info block's rendered height.
//
// Extracted from run-e2e.mjs only to keep that harness under the 1000-line
// scale gate; the assertions are the ones the issue demanded — the track-change
// path was previously never asserted after the surface opened, so the Windows
// "lyrics paint into the song title" regression stayed green in CI.
//
// Issue #90 changed *how* these gates fail. Asserting the written value
// (`=== "0px"`) blessed any collapse that landed on a different number: a
// measurement taken while the metadata block has no box produces a margin of the
// gap itself, which is not zero and does overlap. Every check here is on the
// invariant the offset exists to hold — the lyrics start a full gap below the
// metadata — and each waits for the measurement to land before judging it,
// because one read samples the WebView's intermediate pass (see
// `assertLyricsGapSettles`).

/** The gap the wide layout keeps between the metadata block and the lyrics. */
export const LYRICS_GAP = 20;

/** Slack for the rounded margin and device-pixel snapping. */
const TOLERANCE_PX = 1;

/**
 * How long the measurement is allowed to take to land. The normal case is a
 * frame or two (the write, then the layout that applies it); the room beyond
 * that is for a WebView that applies it late, not for a measurement that never
 * arrives — a collapsed or missing one never reaches the gap at any bound.
 */
const SETTLE_TIMEOUT_MS = 3000;

/** Re-read delay: past the double-rAF settle and the font/observer passes. */
const SETTLE_RECHECK_MS = 300;

/** How much margin may still be left over once an injected height is undone. */
const STALE_MARGIN_PX = 40;

/**
 * Geometry the offset exists to control, read off the open immersive surface.
 * An expression string, so the same read can run inside a poll.
 */
const GEOMETRY_EXPR = `(() => {
  const surface = document.querySelector('[data-testid="immersive"]');
  if (!surface) return null;
  const info = surface.querySelector('.now-playing-info');
  const lyrics = surface.querySelector('.lyrics');
  if (!info || !lyrics) return null;
  return {
    title: surface.querySelector('.now-title')?.textContent ?? '',
    lyricsTop: Math.round(lyrics.getBoundingClientRect().top),
    infoBottom: Math.round(info.getBoundingClientRect().bottom),
    offset: getComputedStyle(lyrics).marginTop,
    inline: lyrics.style.marginTop || "(none)",
    infoDisplay: getComputedStyle(info).display,
  };
})()`;

/** The same read, answered only once the gap holds. */
const SETTLED_EXPR = `(() => {
  const box = ${GEOMETRY_EXPR};
  if (!box) return null;
  return box.lyricsTop >= box.infoBottom + ${LYRICS_GAP} - ${TOLERANCE_PX} ? box : null;
})()`;

/**
 * Fail unless the lyrics clear the metadata bottom by the full gap. The A9 gates
 * assert this invariant rather than the value that produced it: a collapsed
 * measurement is a *different number*, not zero (issue #90).
 *
 * The inline declaration and the metadata's `display` ride along in the message:
 * "no inline margin at all" and "a margin the WebView has not applied yet" are
 * different bugs and must not read as one failure.
 *
 * @param {string} when  what the surface was doing, for the message
 * @param {{ lyricsTop: number, infoBottom: number, offset: string }} box
 * @param {(msg: string) => void} fail
 */
export function assertLyricsClearGap(when, box, fail) {
  if (box.lyricsTop < box.infoBottom + LYRICS_GAP - TOLERANCE_PX)
    fail(
      `A9: ${when}: the lyrics start at ${box.lyricsTop}px but the metadata runs to ${box.infoBottom}px (margin-top: ${box.offset}; inline: ${box.inline}; info display: ${box.infoDisplay}; ${LYRICS_GAP}px gap required)`,
    );
}

/**
 * Wait for the measurement to reach the gap, then re-check that it stayed, and
 * return the re-checked box.
 *
 * A freshly written inline margin is not instantly style: driving this gate, the
 * probe read `margin-top: 109px` on the lyrics element while `getComputedStyle`
 * still answered `0px` and the box had not moved, for about two frames after the
 * surface mounted. Judging a single read therefore fails on a pass the spec
 * explicitly allows to exist as long as it does not stay ("任何测量中间态都不得
 * 作为最终值长期留在界面上"). The bound keeps this a gate and not a wait: a
 * collapsed measurement never reaches the gap. The re-check is the mirror image
 * — an offset that lands correctly and is then walked back by a later, blinder
 * pass.
 *
 * @param {string} when
 * @param {{ evalJs: Function, waitFor: Function, sleep: Function, fail: Function }} t
 */
export async function assertLyricsGapSettles(when, { evalJs, waitFor, sleep, fail }) {
  const settled = await waitFor(SETTLED_EXPR, SETTLE_TIMEOUT_MS);
  if (!settled) {
    const last = await evalJs(GEOMETRY_EXPR);
    if (!last) fail(`A9: ${when}: the immersive surface renders no lyrics or metadata box`);
    assertLyricsClearGap(
      `${when} — the offset never reached the gap within ${SETTLE_TIMEOUT_MS}ms`,
      last,
      fail,
    );
    return last;
  }
  await sleep(SETTLE_RECHECK_MS);
  const stayed = await evalJs(GEOMETRY_EXPR);
  assertLyricsClearGap(`${when}, once the measurement had settled`, stayed, fail);
  return stayed;
}

/** Assert the measured lyrics margin across a track change and a late height change.
 *
 * @param {{
 *   evalJs: Function,
 *   waitFor: Function,
 *   sleep: Function,
 *   fail: (msg: string) => void,
 *   firstSongTitle: string,
 * }} t
 */
export async function assertLyricsOffsetAfterTrackChange(t) {
  const { evalJs, waitFor, fail, firstSongTitle } = t;
  // A queue-entry change remounts the whole surface and the freshly laid-out
  // info block can still be short of its final height (fallback fonts,
  // song_detail landing late). The mock's `player_control next` performs a
  // real queue advance, so this drives exactly the remount path auto-advance
  // takes and re-asserts geometry. Run after the focus assertions: they sample
  // the *first* song's state.
  await evalJs(`document.querySelector('[data-testid="playerbar"] [aria-label="下一首"]').click()`);
  const advanced = await waitFor(
    `(() => {
      const title =
        document.querySelector('[data-testid="immersive"] .now-title')?.textContent ?? '';
      return title && title !== ${JSON.stringify(firstSongTitle)} ? title : null;
    })()`,
    SETTLE_TIMEOUT_MS,
  );
  if (!advanced)
    fail("A9: 下一首 did not advance the queue — the surface still names the first song");

  const switched = await assertLyricsGapSettles("after a track change", t);
  // The wide layout never measures a non-positive offset: the two boxes share
  // one grid area, so an empty or zero margin means the measurement stood down
  // rather than aligned. That is a distinct failure from a collapsed value.
  if (!(parseFloat(switched.offset) > 0))
    fail(
      `A9: after a track change the lyrics margin is "${switched.offset}" — nothing pushed them below the metadata`,
    );

  // Height change *without* a signature change: title/artist/album/duration all
  // stay the same while the info block grows. The signature-triggered
  // measurement could never see this class of change (the Windows font-race root
  // cause of issue #75 is one instance of it); the offset must be re-derived
  // from the rendered height and follow it back down again.
  await evalJs(
    `document.querySelector('[data-testid="immersive"] .now-playing-info').style.paddingBottom = '48px'`,
  );
  await assertLyricsGapSettles("after the info block grew", t);

  await evalJs(
    `document.querySelector('[data-testid="immersive"] .now-playing-info').style.removeProperty('padding-bottom')`,
  );
  // The gap alone cannot prove the follow-back: a margin left at the grown value
  // still clears the shrunken metadata, so this waits on the leftover instead.
  const followed = await waitFor(
    `(() => {
      const box = ${GEOMETRY_EXPR};
      if (!box) return null;
      return box.lyricsTop - box.infoBottom <= ${STALE_MARGIN_PX} ? box : null;
    })()`,
    SETTLE_TIMEOUT_MS,
  );
  if (!followed) {
    const last = await evalJs(GEOMETRY_EXPR);
    fail(
      `A9: after the injected height was removed the lyrics still start ${last.lyricsTop - last.infoBottom}px below the metadata bottom (margin-top: ${last.offset}) — the offset did not follow the height back down`,
    );
  }
  assertLyricsClearGap("after the injected height was removed", followed, fail);
}
