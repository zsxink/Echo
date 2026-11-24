// Issue #85 — 沉浸式播放器显隐/专注过渡 in a real Chromium.
//
// The component tests own the state machine's edges; this gate owns what jsdom
// cannot see: the first painted frame, the measured FLIP against the 常驻播放栏's
// 当前播放区, the delayed unmount window, the body-class lifecycle, focus
// restoration, reduced-motion behaviour and the absence of per-frame lyric
// height writes.
//
// Extracted from run-e2e.mjs to keep that harness under the scale gate.

/** Entry/exit windows from `useImmersiveTransition`. */
const OPEN_MS = 220;
const EXIT_MS = 180;

/**
 * @param {{
 *   cdp: { call: (method: string, params?: object) => Promise<any> },
 *   goto: (seed: string, platform?: string) => Promise<void>,
 *   evalJs: (expr: string) => Promise<any>,
 *   waitFor: (expr: string, timeout?: number) => Promise<any>,
 *   assert: (expr: string, msg: string) => Promise<void>,
 *   sleep: (ms: number) => Promise<void>,
 *   fail: (msg: string) => void,
 *   pass: (msg: string) => void,
 * }} t
 */
export async function assertImmersiveTransitionScenarios({
  cdp,
  goto,
  evalJs,
  waitFor,
  assert,
  sleep,
  fail,
  pass,
}) {
  await goto("medium");
  await cdp.call("Emulation.setDeviceMetricsOverride", {
    width: 1280,
    height: 800,
    deviceScaleFactor: 1,
    mobile: false,
  });
  await sleep(200);

  // Start playback from a row: the surface's cover and the bar's 当前播放区
  // cover must both exist for the FLIP measurement to be meaningful.
  await evalJs(`(() => {
    const row = [...document.querySelectorAll('[data-testid^="song-row-"]')][0];
    if (row) row.click();
  })()`);
  await assert(
    `document.querySelector('[data-testid="playerbar"]')?.getAttribute('data-empty') === null`,
    "A85: clicking a row did not start playback",
  );
  await waitFor(`!!document.querySelector('[data-testid="playerbar"] .mini-cover')`);

  // Record every phase / body-class / lyric-style mutation from before the
  // first click: the opening frame lasts less than one polling interval.
  await evalJs(`(() => {
    window.__echoPhaseLog = [];
    window.__echoBodyLog = [];
    window.__echoLyricStyleWrites = [];
    const phaseObserver = new MutationObserver(() => {
      const phase = document.querySelector('[data-testid="immersive"]')?.getAttribute('data-phase');
      if (phase && window.__echoPhaseLog[window.__echoPhaseLog.length - 1] !== phase)
        window.__echoPhaseLog.push(phase);
    });
    phaseObserver.observe(document.body, {
      subtree: true,
      childList: true,
      attributes: true,
      attributeFilter: ['data-phase'],
    });
    window.__echoPhaseObserver = phaseObserver;
    const bodyObserver = new MutationObserver(() => {
      window.__echoBodyLog.push(document.body.className);
    });
    bodyObserver.observe(document.body, { attributes: true, attributeFilter: ['class'] });
    window.__echoBodyObserver = bodyObserver;
    const styleObserver = new MutationObserver((mutations) => {
      for (const mutation of mutations) {
        const el = mutation.target;
        if (!(el instanceof HTMLElement)) continue;
        if (!el.matches('.lyrics, .lyrics-track')) continue;
        window.__echoLyricStyleWrites.push(el.getAttribute('style') || '');
      }
    });
    styleObserver.observe(document.body, {
      subtree: true,
      attributes: true,
      attributeFilter: ['style'],
    });
    window.__echoStyleObserver = styleObserver;
    return true;
  })()`);

  const trigger = `document.querySelector('[data-testid="now-playing-trigger"]')`;
  // Sample the cover's *painted* first entry frame: the opening state lives for
  // two frames only, so polling would miss it.
  await evalJs(`(() => {
    window.__echoFirstFlip = null;
    const sample = () => {
      if (window.__echoFirstFlip) return;
      const surface = document.querySelector('[data-testid="immersive"]');
      const cover = surface?.querySelector('.playing-cover');
      if (surface?.getAttribute('data-phase') === 'opening' && cover) {
        const r = cover.getBoundingClientRect();
        window.__echoFirstFlip = {
          left: r.left,
          top: r.top,
          width: r.width,
          height: r.height,
        };
      }
      requestAnimationFrame(sample);
    };
    requestAnimationFrame(sample);
    return true;
  })()`);
  await evalJs(`(() => { const el = ${trigger}; el.focus(); el.click(); })()`);

  const opened = await waitFor(
    `(() => {
      const el = document.querySelector('[data-testid="immersive"]');
      return el?.getAttribute('data-phase') === 'open' ? true : null;
    })()`,
    OPEN_MS * 8,
  );
  if (!opened) fail("A85: the immersive surface never settled to data-phase=open");
  // Let the 220ms entry transition finish before reading the settled cover.
  await sleep(OPEN_MS + 100);

  const opening = await evalJs(`(() => {
    const phases = window.__echoPhaseLog;
    return {
      phases,
      body: window.__echoBodyLog,
      sawOpening: phases.includes('opening'),
      sawEntering: window.__echoBodyLog.some((c) => c.includes('player-mode-entering')),
    };
  })()`);
  if (!opening.sawOpening)
    fail(
      `A85: the entry never painted an opening phase (phase log: ${opening.phases.join(" → ")})`,
    );
  if (!opening.sawEntering) fail("A85: body.player-mode-entering never existed during the entry");

  // The FLIP target must be measured from the bar's cover, not hard-coded. The
  // recorded boxes are what the offsets were derived from; comparing them with
  // the *settled* layout would fail on legitimate late font/metadata reflow,
  // so the start position is judged on the sampled opening frame instead.
  const flip = await evalJs(`(() => {
    const surface = document.querySelector('[data-testid="immersive"]');
    const source = document.querySelector('[data-testid="playerbar"] .mini-cover');
    const cover = surface?.querySelector('.playing-cover');
    if (!surface || !source || !cover) return null;
    const s = source.getBoundingClientRect();
    return {
      marker: surface.dataset.flip,
      x: Number.parseFloat(surface.style.getPropertyValue('--flip-x')),
      y: Number.parseFloat(surface.style.getPropertyValue('--flip-y')),
      scale: Number.parseFloat(surface.style.getPropertyValue('--flip-scale')),
      recordedSource: surface.dataset.flipSource,
      recordedTarget: surface.dataset.flipTarget,
      first: window.__echoFirstFlip,
      source: { left: s.left, top: s.top, width: s.width, height: s.height },
      transform: getComputedStyle(cover).transform,
      opacity: Number.parseFloat(getComputedStyle(surface).opacity),
      phase: surface.getAttribute('data-phase'),
    };
  })()`);
  if (!flip) fail("A85: the surface, its cover or the 当前播放区 cover disappeared after opening");
  if (flip.marker !== "cover")
    fail(`A85: the entry did not measure a cover FLIP (marker: ${flip.marker})`);
  if (!flip.first) fail("A85: no opening frame was painted with the measured FLIP");
  if (!flip.recordedSource || !flip.recordedTarget)
    fail("A85: the FLIP measurement did not publish its source/target boxes");

  const sourceRect = flip.recordedSource.split(",").map(Number);
  const targetRect = flip.recordedTarget.split(",").map(Number);
  const sourceCentre = [sourceRect[0] + sourceRect[2] / 2, sourceRect[1] + sourceRect[3] / 2];
  const targetCentre = [targetRect[0] + targetRect[2] / 2, targetRect[1] + targetRect[3] / 2];
  const expectedScale =
    Math.round(Math.max(0.2, Math.min(1.6, sourceRect[2] / targetRect[2])) * 1000) / 1000;
  if (Math.abs(flip.x - (sourceCentre[0] - targetCentre[0])) > 2)
    fail(`A85: --flip-x ${flip.x} is not the measured centre delta`);
  if (Math.abs(flip.y - (sourceCentre[1] - targetCentre[1])) > 2)
    fail(`A85: --flip-y ${flip.y} is not the measured centre delta`);
  if (Math.abs(flip.scale - expectedScale) > 0.01)
    fail(`A85: --flip-scale ${flip.scale} does not match the clamped source/target ratio`);
  // The first painted frame really sits on the 当前播放区 cover (centre) at the
  // measured scale — this is the assertion the visual spec makes.
  const firstCentre = [
    flip.first.left + flip.first.width / 2,
    flip.first.top + flip.first.height / 2,
  ];
  // The first painted frame sits on the current-playing cover's centre. The
  // entry also gives the *surface* a ≤12px downward offset (by design), which
  // carries the cover with it, so the tolerance is that offset rather than 0.
  if (
    Math.abs(firstCentre[0] - sourceCentre[0]) > 3 ||
    Math.abs(firstCentre[1] - sourceCentre[1]) > 12
  )
    fail(
      `A85: the first entry frame is centred at (${firstCentre.map(Math.round).join(", ")}), not on the 当前播放区 cover (${sourceCentre.map(Math.round).join(", ")})`,
    );
  if (Math.abs(flip.first.width - targetRect[2] * expectedScale) > 3)
    fail(
      `A85: the first entry frame is ${Math.round(flip.first.width)}px wide, not ${Math.round(targetRect[2] * expectedScale)}px`,
    );
  if (flip.transform !== "none")
    fail(`A85: the settled cover still carries a transform (${flip.transform})`);
  if (flip.phase !== "open" || Math.abs(flip.opacity - 1) > 0.01)
    fail("A85: the settled surface is not fully opaque/open");

  // No per-frame lyric height writes: the column is not animated by height.
  const lyrics = await evalJs(`(() => {
    const el = document.querySelector('[data-testid="immersive"] .lyrics');
    return {
      writes: window.__echoLyricStyleWrites,
      inlineHeight: el?.style.height ?? "",
      inlineTransform: el?.style.transform ?? "",
    };
  })()`);
  if (lyrics.inlineHeight)
    fail(`A85: the lyrics column carries an inline height (${lyrics.inlineHeight})`);
  if (lyrics.inlineTransform) fail("A85: the lyrics column was moved with an inline transform");
  if (lyrics.writes.some((style) => /(^|;)\s*height\s*:/.test(style)))
    fail("A85: a transition pass wrote the lyrics column's height");

  // ---- 切歌 in place: same phase, no replayed entry -----------------------
  const beforeSwitch = await evalJs(`(() => {
    const surface = document.querySelector('[data-testid="immersive"]');
    return {
      title: surface.querySelector('.now-title')?.textContent ?? '',
      x: Number.parseFloat(surface.style.getPropertyValue('--flip-x')),
      y: Number.parseFloat(surface.style.getPropertyValue('--flip-y')),
      scale: Number.parseFloat(surface.style.getPropertyValue('--flip-scale')),
      openings: window.__echoPhaseLog.filter((p) => p === 'opening').length,
    };
  })()`);
  await evalJs(`document.querySelector('[data-testid="playerbar"] [aria-label="下一首"]').click()`);
  const advanced = await waitFor(`(() => {
    const title =
      document.querySelector('[data-testid="immersive"] .now-title')?.textContent ?? '';
    return title && title !== ${JSON.stringify(beforeSwitch.title)} ? title : null;
  })()`);
  if (!advanced) fail("A85: 下一首 did not change the title inside the open surface");
  await sleep(300);
  const afterSwitch = await evalJs(`(() => {
    const surface = document.querySelector('[data-testid="immersive"]');
    return {
      phase: surface?.getAttribute('data-phase') ?? '',
      x: Number.parseFloat(surface?.style.getPropertyValue('--flip-x') ?? 'NaN'),
      y: Number.parseFloat(surface?.style.getPropertyValue('--flip-y') ?? 'NaN'),
      scale: Number.parseFloat(surface?.style.getPropertyValue('--flip-scale') ?? 'NaN'),
      openings: window.__echoPhaseLog.filter((p) => p === 'opening').length,
      coverTransform: getComputedStyle(surface.querySelector('.playing-cover')).transform,
    };
  })()`);
  if (afterSwitch.phase !== "open")
    fail(`A85: a track change left the surface in "${afterSwitch.phase}", not open`);
  if (
    afterSwitch.x !== beforeSwitch.x ||
    afterSwitch.y !== beforeSwitch.y ||
    afterSwitch.scale !== beforeSwitch.scale
  )
    fail("A85: a track change re-ran the cover FLIP");
  if (afterSwitch.openings !== beforeSwitch.openings)
    fail("A85: a track change replayed the entry phase");
  if (afterSwitch.coverTransform !== "none")
    fail("A85: the cover is not settled after a track change");

  // ---- exit: delayed unmount, inert surface, restored focus ----------------
  await evalJs(`document.querySelector('[data-testid="close-player-mode"]').click()`);
  await sleep(30);
  const during = await evalJs(`(() => {
    const el = document.querySelector('[data-testid="immersive"]');
    if (!el) return null;
    return {
      phase: el.getAttribute('data-phase'),
      ariaHidden: el.getAttribute('aria-hidden'),
      inert: el.hasAttribute('inert'),
      body: document.body.className,
      focused: document.activeElement?.getAttribute('data-testid') ?? '',
    };
  })()`);
  if (!during) fail("A85: the surface was gone 30ms into a 180ms exit");
  if (during.phase !== "closing") fail(`A85: exit phase is "${during.phase}", not closing`);
  if (during.ariaHidden !== "true" || !during.inert)
    fail("A85: the exiting surface is still exposed to assistive technology / pointer input");
  if (!during.body.includes("player-mode-open") || !during.body.includes("player-mode-closing"))
    fail(`A85: body classes during the exit are "${during.body}"`);
  if (during.body.includes("player-mode-entering"))
    fail("A85: player-mode-entering survived into the exit");
  if (during.focused !== "now-playing-trigger")
    fail(
      `A85: focus was not restored to the opening control during the exit (active: ${during.focused || "none"})`,
    );

  // It must stay for the exit window and only then leave.
  await sleep(70);
  const stillThere = await evalJs(`!!document.querySelector('[data-testid="immersive"]')`);
  if (!stillThere) fail("A85: the surface unmounted before the exit animation could finish");
  await waitFor(`!document.querySelector('[data-testid="immersive"]')`, EXIT_MS * 10);
  const after = await evalJs(`(() => ({
    surface: !!document.querySelector('[data-testid="immersive"]'),
    body: document.body.className,
    phases: window.__echoPhaseLog,
  }))()`);
  if (after.surface) fail("A85: the surface never unmounted after the exit");
  for (const cls of [
    "player-mode-open",
    "player-mode-entering",
    "player-mode-closing",
    "lyrics-focus-open",
  ]) {
    if (after.body.includes(cls)) fail(`A85: "${cls}" survived the exit ("${after.body}")`);
  }
  // The closed phase unmounts the surface, so its `data-phase` is not
  // observable; the log ending at `closing` plus a gone surface and clean body
  // is the convergence check.
  const lastPhase = after.phases[after.phases.length - 1];
  if (lastPhase !== "closed" && lastPhase !== "closing")
    fail(`A85: the phase log did not converge to closed (${after.phases.join(" → ")})`);

  // ---- prefers-reduced-motion: immediate, no delayed unmount --------------
  await cdp.call("Emulation.setEmulatedMedia", {
    features: [{ name: "prefers-reduced-motion", value: "reduce" }],
  });
  await sleep(100);
  await evalJs(`window.__echoPhaseLog = []`);
  await evalJs(`(() => { const el = ${trigger}; el.focus(); el.click(); })()`);
  const reducedOpen = await waitFor(
    `(() => {
      const el = document.querySelector('[data-testid="immersive"]');
      return el?.getAttribute('data-phase') === 'open' ? true : null;
    })()`,
    1000,
  );
  if (!reducedOpen) fail("A85: reduced motion never reached an open surface");
  const reducedPhases = await evalJs(`window.__echoPhaseLog`);
  if (reducedPhases.includes("opening")) fail("A85: reduced motion still painted an opening phase");

  const spin = await evalJs(`(() => {
    const ring = document.querySelector('[data-testid="immersive"] .playing-cover.is-playing .ring');
    if (!ring) return null;
    const cs = getComputedStyle(ring);
    return { name: cs.animationName, duration: cs.animationDuration };
  })()`);
  if (!spin || spin.name !== "record-spin")
    fail("A85: reduced motion stopped the 黑胶 record rotation (the documented exception)");

  await evalJs(`document.querySelector('[data-testid="close-player-mode"]').click()`);
  await sleep(30);
  const reducedGone = await evalJs(`!document.querySelector('[data-testid="immersive"]')`);
  if (!reducedGone) fail("A85: reduced motion still delayed the unmount");
  if (await evalJs(`document.body.classList.contains('player-mode-open')`))
    fail("A85: reduced motion left the immersion body state behind");

  // Restore the media setting; the surrounding A9/A13 scenarios must not
  // inherit the reduced-motion emulation.
  await cdp.call("Emulation.setEmulatedMedia", { media: "", features: [] });
  await sleep(50);
  await evalJs(`window.__echoPhaseLog = []`);
  await evalJs(`(() => { const el = ${trigger}; el.focus(); el.click(); })()`);
  const restored = await waitFor(
    `(() => {
      const el = document.querySelector('[data-testid="immersive"]');
      return el?.getAttribute('data-phase') === 'open' ? true : null;
    })()`,
    OPEN_MS * 8,
  );
  if (!restored) fail("A85: the surface did not reopen after clearing emulated media");
  const restoredPhases = await evalJs(`window.__echoPhaseLog`);
  if (!restoredPhases.includes("opening"))
    fail("A85: clearing emulated media did not restore the transition");
  await evalJs(`document.querySelector('[data-testid="close-player-mode"]').click()`);
  await waitFor(`!document.querySelector('[data-testid="immersive"]')`, EXIT_MS * 10);

  // ---- layout widths: the measurement is not a desktop-only constant ------
  // ≤760px hides the 当前播放区 in immersive mode, so the FLIP must degrade to
  // the plain fade; 761–980px and ≥981px are both measurable. No width may
  // introduce horizontal overflow.
  const layouts = [
    [390, 844, "none"],
    [880, 800, "cover"],
    [1280, 800, "cover"],
  ];
  for (const [width, height, expected] of layouts) {
    await cdp.call("Emulation.setDeviceMetricsOverride", {
      width,
      height,
      deviceScaleFactor: 1,
      mobile: false,
    });
    await sleep(250);
    await evalJs(`(() => { const el = ${trigger}; el.focus(); el.click(); })()`);
    const layoutOpen = await waitFor(
      `(() => {
        const el = document.querySelector('[data-testid="immersive"]');
        return el?.getAttribute('data-phase') === 'open' ? true : null;
      })()`,
      OPEN_MS * 8,
    );
    if (!layoutOpen) fail(`A85: the surface did not open at ${width}x${height}`);
    await sleep(OPEN_MS + 60);
    const layout = await evalJs(`(() => {
      const surface = document.querySelector('[data-testid="immersive"]');
      return {
        marker: surface.dataset.flip,
        overflow: document.documentElement.scrollWidth > window.innerWidth + 1,
        cover: !!surface.querySelector('.playing-cover'),
        phase: surface.getAttribute('data-phase'),
      };
    })()`);
    if (layout.phase !== "open") fail(`A85: ${width}px surface did not settle open`);
    if (!layout.cover) fail(`A85: ${width}px surface rendered no cover`);
    if (layout.marker !== expected)
      fail(
        `A85: at ${width}px the FLIP marker is "${layout.marker}", expected "${expected}" (the 当前播放区 must be measurable exactly when it is visible)`,
      );
    if (layout.overflow)
      fail(`A85: ${width}px layout overflows horizontally with the surface open`);
    await evalJs(`document.querySelector('[data-testid="close-player-mode"]').click()`);
    await waitFor(`!document.querySelector('[data-testid="immersive"]')`, EXIT_MS * 10);
  }

  await cdp.call("Emulation.clearDeviceMetricsOverride");
  pass(
    "A85: 沉浸式播放器从当前播放区 FLIP 入场、退场延迟卸载且不可交互并恢复焦点、body 类与状态收敛、减少动画即时完成且唱片继续旋转",
  );
}
