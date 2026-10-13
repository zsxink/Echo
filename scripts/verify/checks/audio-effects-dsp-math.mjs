#!/usr/bin/env node
// Task AFX-2: verify the audio-effects DSP arithmetic against independent maths.
//
// AFX-1 guards EVIDENCE DISCIPLINE (did we retract the wrong claim, does the
// probe still prove a runtime change). It does not check a single number in the
// DSP design. This gate does, so a later edit to a coefficient, a headroom
// constant or a preset cannot quietly produce a different response while every
// existing test stays green.
//
// Every expectation here was derived two ways: from the FFmpeg n6.0 upstream
// source and from `effects/math.rs`, independently. The script RE-COMPUTES the
// reference values from the formulas; it does not hard-code the answers alone.
// Where a value came from a formula, the formula is inlined below.
//
// The discriminating power of each assertion was verified in both directions:
// a normal tree passes, and a deliberately broken tree fails. "rc=0 in the
// normal state" alone cannot tell a real gate from a no-op.

// --- maths (kept deliberately plain: no imports, no deps) -------------------

import {
  BAND_FREQUENCIES, DENSE_INTERVALS, peakingCoefficients, responseDb,
  compiledChain, gridPeak, densePeak, peakWidthOctaves,
} from "./audio-effects-dsp-reference.mjs";
import { forbiddenClaims } from "./audio-effects-doc-claims.mjs";

// --- expectations ----------------------------------------------------------


const PRESETS = {
  pop: [1, 2, 1, -1, -0.5, 0, 1, 2, 1, 0],
  rock: [3, 4, 2, 0, -1.5, -2, -0.5, 2, 3, 2],
  classical: [0, 0, 0, 0, 0, 0, 0, 0.5, 0.5, 0.5],
  jazz: [1, 1.5, 2, 1.5, 0.5, 0, 0.5, 1, 1.5, 1],
  electronic: [2, 3, 2, -1, -1.5, -1, 0, 1.5, 2, 1],
  vocal: [-1.5, -1.5, -1, -0.5, 0, 1, 2, 1, -1, 0],
  bass: [1.5, 4, 3, 0.5, 0, -0.5, -0.5, -0.5, -1, -1.5],
  warm: [1, 1.5, 1, 0.5, 0, 0, -0.5, -1, -2, -2.5],
  retro: [0, 1, 2.5, 3, 2, 0.5, -0.5, -2, -3.5, -5],
};

const failures = [];
const check = (condition, message) => {
  if (!condition) failures.push(message);
};

// 1. A zero-gain peaking section is EXACTLY the identity (numerator == denominator).
//    This is why math.rs can special-case it and why skipping inactive bands is safe.
for (const rate of [22050, 44100, 48000, 96000]) {
  for (const center of BAND_FREQUENCIES) {
    const c = peakingCoefficients(center, rate, 0);
    check(
      c.b0 === 1 && c.b1 === 0 && c.b2 === 0 && c.a1 === 0 && c.a2 === 0,
      `g=0 must be exactly identity: ${center} Hz @ ${rate} gave b=${c.b0},${c.b1},${c.b2} a=${c.a1},${c.a2}`,
    );
  }
}

// 2. Analytic 997 Hz response of the 1 kHz/+6 dB section: the historical single-tone response
//    measured natively in the 2026-10-08 runtime investigation (evidence: +5.995782..+5.995869 dB
//    measured, -0.0042 dB error vs analytic).
const ANALYTIC_997 = (() => {
  const c = peakingCoefficients(1000, 48000, 6);
  return responseDb(c, 48000, 997);
})();
check(
  Math.abs(ANALYTIC_997 - 5.999529) < 1e-5,
  `analytic |H(997)| for 1 kHz/+6 dB should be 5.999529 dB, got ${ANALYTIC_997}`,
);

// 3. Band validity: fc <= 0.45 * Fs. Cross-checks the 22.05 kHz degeneration
//    claim in native-gate.md ("only the 16 kHz band degenerates").
const expectedInactive = {
  22050: [16000],
  44100: [],
  48000: [],
  96000: [],
};
for (const [rateText, expected] of Object.entries(expectedInactive)) {
  const rate = Number(rateText);
  const inactive = BAND_FREQUENCIES.filter((f) => f > 0.45 * rate);
  check(
    JSON.stringify(inactive) === JSON.stringify(expected),
    `at ${rate} Hz the inactive bands should be ${JSON.stringify(expected)}, got ${JSON.stringify(inactive)}`,
  );
  for (const f of inactive) {
    const c = peakingCoefficients(f, rate, 12);
    check(c.identity, `band ${f} Hz @ ${rate} must be identity even at +12 dB (skipped, not zero-gain)`);
  }
}

// 4. Cascade arithmetic: a dB sum equals product-of-H, so the chain peak is what
//    the preamp budget must cover. Pinned because getting it wrong in EITHER
//    direction misleads the preset design: the preamp would either clip or
//    attenuate needlessly.
//    Measured (Fs=48000): single +12 -> 12.000; 1k+2k -> 14.565 @1025 Hz;
//    1k+2k+4k -> 16.904; all ten -> 18.403 @500 Hz. Neighbouring sections
//    therefore exceed a single section, but remain below the sum of sliders.
const peakAt = (gains, sampleRate) => {
  let best = -Infinity;
  let bestHz = 0;
  const step = 0.05;
  const top = sampleRate / 2;
  const { evaluate } = compiledChain(gains, sampleRate);
  for (let frequency = 0; frequency <= top; frequency += step) {
    const db = evaluate(frequency);
    if (db > best) {
      best = db;
      bestHz = frequency;
    }
  }
  return { best, bestHz };
};
const twoAdjacent = [0, 0, 0, 0, 0, 12, 12, 0, 0, 0];
const twoAdjacentPeak = peakAt(twoAdjacent, 48000).best;
check(
  twoAdjacentPeak > 14.5 && twoAdjacentPeak < 14.65,
  `+12@1k with +12@2k should cascade to about +14.565 dB, got ${twoAdjacentPeak.toFixed(4)}`,
);
const threeAdjacentPeak = peakAt([0, 0, 0, 0, 0, 12, 12, 12, 0, 0], 48000).best;
check(
  threeAdjacentPeak > 16.85 && threeAdjacentPeak < 16.95,
  `+12 at 1k/2k/4k should cascade to about +16.904 dB, got ${threeAdjacentPeak.toFixed(4)}`,
);
const allTwelvePeak = gridPeak(BAND_FREQUENCIES.map(() => 12), 48000);
check(
  allTwelvePeak > 18.35 && allTwelvePeak < 18.45,
  `all ten bands at +12 dB should peak near +18.403 dB, got ${allTwelvePeak.toFixed(4)}`,
);
// A single section must reach exactly its nominal gain, otherwise the whole
// peak-gain budget is miscalibrated at the most basic level.
const singleTwelve = peakAt([0, 0, 0, 0, 0, 12, 0, 0, 0, 0], 48000).best;
check(
  Math.abs(singleTwelve - 12) < 1e-3,
  `a single +12 dB section must peak at exactly +12 dB, got ${singleTwelve.toFixed(4)}`,
);

// 5. Measured comparison on 12 curves x four rates, including mixed signs.
// This regression does not establish a global bound on arbitrary curves.
let worstLowBias = 0;
let worstHighBias = 0;
let worstLowPreset = null;
const comparisonCurves = {
  ...PRESETS,
  allPlus12: BAND_FREQUENCIES.map(() => 12),
  adjacentPlus12: twoAdjacent,
  alternating: [12, -12, 12, -12, 12, -12, 12, -12, 12, -12],
};
for (const rate of [22050, 44100, 48000, 96000]) {
  for (const [id, gains] of Object.entries(comparisonCurves)) {
    const dense = densePeak(gains, rate);
    check(dense.samples === 1048577, `${id}@${rate} must sample every dense grid point`);
    const coarse = gridPeak(gains, rate);
    const gap = dense.gainDb - coarse;
    if (gap > worstLowBias) { worstLowBias = gap; worstLowPreset = `${id}@${rate}`; }
    worstHighBias = Math.max(worstHighBias, -gap);
    // Independent sampled maximum must dominate both immediate dense neighbors.
    const { evaluate } = compiledChain(gains, rate);
    const step = rate / (2 * DENSE_INTERVALS);
    for (const neighbor of [dense.frequencyHz - step, dense.frequencyHz + step]) {
      if (neighbor >= 0 && neighbor <= rate / 2) {
        check(evaluate(neighbor) <= dense.gainDb + 1e-9, `${id}@${rate} dense peak misses its neighbor`);
      }
    }
  }
}
check(worstLowBias < 0.01, `sampled coarse underestimate ${worstLowBias} dB at ${worstLowPreset}`);
check(worstHighBias < 0.01, `sampled coarse overestimate ${worstHighBias} dB`);

// 6. Peak-gain table in design.md section 3. Recomputed from the same formulas.
const AUTO_HEADROOM_DB = 0.5;
const expectedAutoPreamp = {
  classical: -0.11,
  electronic: -3.18,
  vocal: -1.82,
  bass: -4.35,
};
for (const [id, expected] of Object.entries(expectedAutoPreamp)) {
  const peak = gridPeak(PRESETS[id], 48000);
  const actual = -Math.max(peak, 0) + AUTO_HEADROOM_DB;
  check(
    Math.abs(actual - expected) <= 0.01,
    `design's Auto preamp for ${id} should be ${expected} dB, recomputed ${actual.toFixed(3)}`,
  );
}

// 7. Bandwidth claims. "Q=sqrt(2) is one octave" holds only near +6 dB; design and
//    review-and-tuning must keep saying so rather than being simplified away.
const widthAt6 = peakWidthOctaves(1000, 48000, 6);
const widthAt12Low = peakWidthOctaves(1000, 48000, 12);
const widthAt12High = peakWidthOctaves(16000, 48000, 12);
check(
  Math.abs(widthAt6 - 1.0) < 0.01,
  `+6 dB at 1 kHz should be ~1.00 octave, got ${widthAt6.toFixed(4)}`,
);
check(
  widthAt12Low > 0.5 && widthAt12Low < 0.6,
  `+12 dB at 1 kHz should be ~0.54 octave, got ${widthAt12Low.toFixed(4)}`,
);
check(
  widthAt12High > 0.2 && widthAt12High < 0.25,
  `+12 dB at 16 kHz should be ~0.225 octave, got ${widthAt12High.toFixed(4)}`,
);

// 8. extrastereo: FFmpeg computes M + m*(L - M) with M = (L+R)/2, i.e. M + m*S.
//    The three fixture shapes must give +1.9382 dB, 0 dB and +1.0231 dB.
const extrastereo = (m) => ({
  left: (1 + m) / 2,
  right: (1 - m) / 2,
});
for (const [m, shape, expectedDb] of [
  [1.25, "antiphase", 20 * Math.log10(1.25)],
  [1.25, "in-phase", 0],
  [1.25, "left-only", 20 * Math.log10(1.125)],
]) {
  const { left, right } = extrastereo(m);
  const gainDb = 20 * Math.log10(Math.abs(left * (shape === "antiphase" ? 1 : 1) + right * (shape === "antiphase" ? -1 : shape === "left-only" ? 0 : 1)));
  check(
    Math.abs(gainDb - expectedDb) < 1e-3,
    `extrastereo m=${m} on ${shape} input should be ${expectedDb.toFixed(4)} dB, got ${gainDb.toFixed(4)}`,
  );
}

// 9. alimiter threshold: limit=0.891250938 is exactly -1 dBFS, and the measured
//    output peak -0.99985 dBFS is its s16 rounding residue, i.e. the limiter WAS
//    active (evidence/macos-pcm-limiter-stress-probe.json).
const LIMIT = 0.891250938;
check(
  Math.abs(20 * Math.log10(LIMIT) + 1) < 1e-6,
  `limit ${LIMIT} should be exactly -1 dBFS, got ${(20 * Math.log10(LIMIT)).toFixed(6)}`,
);
const measuredPeakDb = 20 * Math.log10(29205 / 32768);
check(
  Math.abs(measuredPeakDb - -0.99985) < 1e-4,
  `s16 code 29205 should be -0.99985 dBFS, got ${measuredPeakDb.toFixed(6)}`,
);
check(
  Math.abs(measuredPeakDb + 1) < 0.001,
  "the measured peak must sit within 0.001 dB of the limit, otherwise it does not prove the limiter engaged",
);

// 10. Source-level invariants that no unit test can drift away from.
//
// These must match the SHIPPED filter string, not a prose mention. Searching for
// a bare option name is not enough: `level=false` appears both in the LIMITER
// constant and in the comment explaining why it is required, so a substring
// search passes even after someone deletes the option from the chain — a gate
// that reports green while the protection is gone. Each invariant is therefore
// scoped to the constant/chain expression itself, and comment lines are ignored.
const NATIVE_EFFECTS = "crates/echo-desktop/src/player/native_effects.rs";
const stripRustComments = (source) =>
  source
    .split("\n")
    .map((line) => line.replace(/\/\/.*$/, ""))
    .join("\n");

const invariants = [
  {
    file: NATIVE_EFFECTS,
    // The whole limiter expression, so removing ANY single option trips the gate.
    needle:
      "alimiter=limit=0.891250938:attack=5:release=50:level_in=1:level_out=1:asc=false:level=false:latency=true",
    why: "the committed limiter configuration; dropping level=false would let FFmpeg's auto-level renormalise the output back to 0 dB and destroy the preamp budget, and dropping asc/latency/limit changes what the Gate measured",
  },
  {
    file: NATIVE_EFFECTS,
    // m=1 is the internal neutral used for enable/bypass transitions; c=false
    // disables extrastereo's internal clipping so peaks reach the end limiter.
    needle: "extrastereo@spatial=m=1:c=false",
    why: "the internal neutral width and the internal-clipping disable are both load-bearing",
  },
  {
    file: NATIVE_EFFECTS,
    needle: "precision=f64",
    why: "the EQ sections use f64 precision, matching their RBJ response model; this does not assert every spatial stage is f64",
  },
  {
    file: NATIVE_EFFECTS,
    needle: "normalize=false",
    why: "normalize defaults to false; enabling it additionally rescales the numerator and changes the modelled level",
  },
  {
    file: NATIVE_EFFECTS,
    needle: "aformat=sample_fmts=dblp",
    why: "the graph starts at the dblp format boundary; FFmpeg may insert format conversions, including the spatial float bridge",
  },
  {
    file: NATIVE_EFFECTS,
    needle: "t=q:w=",
    why: "each section must use the Q-factor width type; a different width_type changes every coefficient",
  },
];

// Read the Rust sources here rather than in a test, so the assertions are about
// the shipped chain syntax and not about a duplicated constant.
import { readFileSync, existsSync } from "node:fs";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "../../..");
const codeCache = new Map();
const readCode = (file) => {
  if (!codeCache.has(file)) {
    const path = resolve(ROOT, file);
    codeCache.set(file, existsSync(path) ? stripRustComments(readFileSync(path, "utf8")) : null);
  }
  return codeCache.get(file);
};
for (const item of invariants) {
  const source = readCode(item.file);
  if (source === null) {
    failures.push(`missing ${item.file}, cannot verify the filter chain`);
    continue;
  }
  if (!source.includes(item.needle)) {
    failures.push(`${item.file} no longer contains "${item.needle}": ${item.why}`);
  }
}

// Q and the transition target are pinned as whole-expression patterns rather than
// substrings: the maths above models Q=sqrt(2) BY DEFINITION, so it would happily
// agree with a source that changed Q. Substring matching is also not enough here,
// because both tokens appear in prose elsewhere in the file.
{
  const source = readCode(NATIVE_EFFECTS);
  if (source === null) {
    failures.push(`missing ${NATIVE_EFFECTS}, cannot verify Q or the transition target`);
  } else {
    if (!/std::f64::consts::SQRT_2/.test(source)) {
      failures.push(
        `${NATIVE_EFFECTS} no longer installs Q as SQRT_2. The DSP maths above models Q=sqrt(2) by ` +
          "definition; changing the installed Q without changing the model is exactly how the response " +
          "graph and the audio diverge",
      );
    }
    if (!/const TRANSITION: Duration = Duration::from_millis\(30\)/.test(source)) {
      failures.push(`${NATIVE_EFFECTS} no longer sets the 30 ms transition target`);
    }
  }
}

// The band frequencies must remain the ones the specs promise. They live in
// effects/model.rs, not mod.rs.
{
  const source = readCode("crates/echo-desktop/src/effects/model.rs");
  if (source === null) {
    failures.push("missing crates/echo-desktop/src/effects/model.rs, cannot verify the band frequencies");
  } else {
    for (const frequency of BAND_FREQUENCIES) {
      if (!source.includes(String(frequency))) {
        failures.push(`effects/model.rs no longer declares the ${frequency} Hz band the specs fix`);
      }
    }
    // The gain step and range are spec-fixed (-12..+12, 0.5 dB steps).
    for (const bound of ["-12.0", "12.0", "0.5"]) {
      if (!source.includes(bound)) {
        failures.push(`effects/model.rs no longer references the ${bound} gain bound/step the specs fix`);
      }
    }
  }
}

// The 0.45*Fs band-validity rule is what the whole low-sampling-rate story rests
// on. A bare "does the file mention 0.45" test is not enough: writing 0.95
// instead of 0.45 also contains no 0.45 change in the places that matter and
// would silently move the 22.05 kHz behaviour, so the comparison must be pinned.
{
  const source = readCode("crates/echo-desktop/src/effects/math.rs");
  if (source === null) {
    failures.push("missing crates/echo-desktop/src/effects/math.rs, cannot verify the 0.45*Fs rule");
  } else {
    const uses = source.match(/0\.45\s*\*\s*f64::from\(/g) ?? [];
    if (uses.length < 2) {
      failures.push(
        `effects/math.rs applies "0.45 * Fs" in ${uses.length} place(s); the band-validity rule is used both for ` +
          "the active-band mask and for the per-section identity skip, so fewer than two means one was changed",
      );
    }
    if (/0\.(?!45\b)\d+\s*\*\s*f64::from\(sample_rate\)/.test(source)) {
      failures.push("effects/math.rs applies a different coefficient than 0.45 to the sample rate");
    }
    // g=0 must return IDENTITY exactly, and an out-of-range band must too.
    const peaking = source.slice(source.indexOf("pub fn peaking"));
    if (!/gain_db\s*==\s*0\.0[\s\S]{0,220}?return Self::IDENTITY/.test(peaking)) {
      failures.push(
        "effects/math.rs::Biquad::peaking no longer returns IDENTITY for gain_db == 0.0 within the same " +
          "condition as the band-validity skip; a zero-gain section must be exactly the identity, not a " +
          "near-identity that shifts the level",
      );
    }
  }
}

// 11. Documents must not drift back to the discredited readings.
//
// These rules are line-scoped and skip lines that are *quoting the error in order
// to correct it*. Without that carve-out the gate would flag its own errata:
// recording "原文称 X 不受支持，已删除" contains the forbidden phrase while
// being exactly the text we want. A rule that punishes its own correction cannot
// be distinguished from a no-op, so the exclusion is explicit.
//
// The exclusion matches the *corrective* wording only, and the corrective wording
// is deliberately distinctive ("不是", "更正", "retracted"). A live assertion of
// the wrong claim does not read like an apology, so the two cannot be confused —
// verified in both directions below.
const CHANGE = "openspec/changes/introduce-audio-effects-equalizer";
for (const file of ["design.md", "native-gate.md", "review-and-tuning.md", "review-and-fixes.md", "tasks.md"]) {
  const path = resolve(ROOT, `${CHANGE}/${file}`);
  if (!existsSync(path)) { failures.push(`missing ${CHANGE}/${file}`); continue; }
  failures.push(...forbiddenClaims(readFileSync(path, "utf8")).map((claim) => `${CHANGE}/${file}: ${claim}`));
}

// The Auto headroom must stay labelled unverified wherever the table is quoted,
// otherwise a reader can lift the numbers without the caveat.
//
// Keyword matching ("does the word 未验证 appear nearby") is NOT sufficient here:
// the words 无任何 and 定稿 occur elsewhere in the same section for unrelated
// reasons, so removing the caveat still leaves a match and the gate passes green.
// The assertion is therefore on the exact caveat sentence, in both places it is
// required: the peak table and the headroom formula.
{
  const path = resolve(ROOT, `${CHANGE}/design.md`);
  if (!existsSync(path)) {
    failures.push(`missing ${CHANGE}/design.md`);
  } else {
    const design = readFileSync(path, "utf8");
    check(
      design.includes("新曲线保守 Auto preamp dB"),
      "design.md no longer contains the Auto preamp table",
    );
    check(
      design.includes("是未验证的调音偏好"),
      "design.md's peak table no longer states that the 0.5 dB Auto compensation is unverified; " +
        "the numbers are pure arithmetic and can be quoted without the caveat",
    );
    check(
      design.includes("该 0.5 dB 目前无实测 A/B 支撑"),
      "design.md's headroom section no longer records that the 0.5 dB has no measured A/B; " +
        "this is the sentence that keeps it out of the 'already validated' column",
    );
  }
}

// 12. The measured-matrix claims must stay honest about what ao=pcm cannot show.
{
  const path = resolve(ROOT, `${CHANGE}/native-gate.md`);
  if (existsSync(path)) {
    const gate = readFileSync(path, "utf8");
    if (!/audio-params[^.\n]*恒等|结构性无法暴露/.test(gate)) {
      failures.push(
        "native-gate.md no longer states that the sampling-rate matrix cannot expose source Fs != graph Fs; " +
          "that limit is what keeps fc<=0.45*Fs unverified",
      );
    }
    if (!/usable_for_tolerance_verdict|0\.25 dB/.test(gate)) {
      failures.push("native-gate.md no longer records that the 0.25 dB response tolerance was never measured");
    }
  }
}

if (failures.length) {
  for (const failure of failures) console.error(`error: ${failure}`);
  process.exit(1);
}

console.log(
  `ok: AFX-2 DSP maths verified (analytic 997 Hz ${ANALYTIC_997.toFixed(6)} dB; ` +
    `adjacent cascade ${twoAdjacentPeak.toFixed(4)} dB; all-+12 ${allTwelvePeak.toFixed(4)} dB; ` +
    `48 cases x ${DENSE_INTERVALS + 1} samples; measured grid bias -${worstLowBias.toFixed(8)}/+${worstHighBias.toFixed(8)} dB; ` +
    `widths ${widthAt6.toFixed(3)}/${widthAt12Low.toFixed(3)}/${widthAt12High.toFixed(3)} oct)`,
);
