// A9 extension (issue #75): the immersive lyrics offset must survive a track
// change and re-derive from the info block's rendered height.
//
// Extracted from run-e2e.mjs only to keep that harness under the 1000-line
// scale gate; the assertions are the ones the issue demanded — the track-change
// path was previously never asserted after the surface opened, so the Windows
// "lyrics paint into the song title" regression stayed green in CI.

/** Assert `--lyrics-start-offset` behavior across a track change.
 *
 * @param {{
 *   evalJs: (expr: string) => Promise<unknown>,
 *   sleep: (ms: number) => Promise<void>,
 *   fail: (msg: string) => void,
 *   firstSongTitle: string,
 * }} t
 */
export async function assertLyricsOffsetAfterTrackChange({ evalJs, sleep, fail, firstSongTitle }) {
  // A queue-entry change remounts the whole surface and the freshly laid-out
  // info block can still be short of its final height (fallback fonts,
  // song_detail landing late). The mock's `player_control next` performs a
  // real queue advance, so this drives exactly the remount path auto-advance
  // takes and re-asserts geometry. Run after the focus assertions: they sample
  // the *first* song's state.
  await evalJs(`document.querySelector('[data-testid="playerbar"] [aria-label="下一首"]').click()`);
  await sleep(400);
  const switched = await evalJs(`(() => {
    const surface = document.querySelector('[data-testid="immersive"]');
    const info = surface.querySelector('.now-playing-info');
    const lyrics = surface.querySelector('.lyrics');
    return {
      title: surface.querySelector('.now-title').textContent,
      lyricsTop: Math.round(lyrics.getBoundingClientRect().top),
      infoBottom: Math.round(info.getBoundingClientRect().bottom),
      offset: getComputedStyle(lyrics).marginTop,
    };
  })()`);
  if (switched.title === firstSongTitle)
    fail("A9: 下一首 did not advance the queue — the surface still names the first song");
  if (switched.offset === "0px")
    fail("A9: after a track change --lyrics-start-offset is unset (0px)");
  if (switched.lyricsTop < switched.infoBottom)
    fail(
      `A9: after a track change the lyrics start at ${switched.lyricsTop}px but the metadata runs to ${switched.infoBottom}px (--lyrics-start-offset: ${switched.offset})`,
    );

  // Height change *without* a signature change: title/artist/album/duration
  // all stay the same while the info block grows. The signature-triggered
  // measurement could never see this class of change (the Windows font-race
  // root cause of issue #75 is one instance of it); the offset must be
  // re-derived from the rendered height and follow it back down again.
  const grew = await evalJs(`(async () => {
    const info = document.querySelector('[data-testid="immersive"] .now-playing-info');
    const lyrics = document.querySelector('[data-testid="immersive"] .lyrics');
    info.style.paddingBottom = '48px';
    await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));
    await new Promise((r) => setTimeout(r, 150));
    const mid = {
      lyricsTop: Math.round(lyrics.getBoundingClientRect().top),
      infoBottom: Math.round(info.getBoundingClientRect().bottom),
      offset: getComputedStyle(lyrics).marginTop,
    };
    info.style.removeProperty('padding-bottom');
    await new Promise((r) => setTimeout(r, 150));
    const back = {
      lyricsTop: Math.round(lyrics.getBoundingClientRect().top),
      infoBottom: Math.round(info.getBoundingClientRect().bottom),
    };
    return { mid, back };
  })()`);
  if (grew.mid.lyricsTop < grew.mid.infoBottom)
    fail(
      `A9: the info block grew to ${grew.mid.infoBottom}px but the lyrics kept their start at ${grew.mid.lyricsTop}px (--lyrics-start-offset: ${grew.mid.offset}) — the offset is not re-derived from the rendered height`,
    );
  if (grew.back.lyricsTop - grew.back.infoBottom > 40)
    fail(
      `A9: after the injected height was removed the lyrics still start ${grew.back.lyricsTop - grew.back.infoBottom}px below the metadata bottom — the offset did not follow the height back down`,
    );
}
