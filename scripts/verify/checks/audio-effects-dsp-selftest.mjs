#!/usr/bin/env node
import assert from "node:assert/strict";
import { forbiddenClaims } from "./audio-effects-doc-claims.mjs";
import { compiledChain, gridPeak, peakWidthOctaves, peakingCoefficients, responseDb } from "./audio-effects-dsp-reference.mjs";

try {
  // Deliberate false claims must fail even if unrelated correction wording exists.
  for (const claim of [
    "误差很小，级联真实峰值仅 +0.607 dB。",
    "不是明显失真；级联峰值只有 +0.607 dB。",
    "上段已更正。\n级联峰值仅 +0.607 dB。",
    "`c` 是 center coefficient。",
    "30 ms 连续过渡。",
    "`spatial:m` 更由结构体检查证实不受支持。",
    "cut 谷宽 3.32 octave。",
  ]) assert.ok(forbiddenClaims(claim).length, `false claim escaped: ${claim}`);
  for (const correction of [
    "历史 +0.607 dB 来自错误范围，已撤回。",
    "旧 +0.607 dB 是扫描范围错误。",
    "`c` 不是 center coefficient。",
    "此前把它当作中心系数是误读。",
    "「30 ms 连续过渡」实为分轮参数插值。",
    "原文「`spatial:m` 更由结构体检查证实不受支持」，已删除。",
    "历史 cut 谷宽 3.32 octave 来自不适用的搜索括区，应删除。",
  ]) assert.equal(forbiddenClaims(correction).length, 0, `correction rejected: ${correction}`);

  for (const rate of [22050, 44100, 48000, 96000]) {
    for (const center of [125, 1000, 8000]) {
      for (const gain of [6, 12]) {
        const boost = peakingCoefficients(center, rate, gain);
        const cut = peakingCoefficients(center, rate, -gain);
        for (const f of [20, center / 2, center, center * 1.2, rate * 0.49]) {
          assert.ok(Math.abs(responseDb(boost, rate, f) + responseDb(cut, rate, f)) < 1e-8, "RBJ cut must invert boost");
        }
        const width = peakWidthOctaves(center, rate, gain);
        assert.ok(Math.abs(width - peakWidthOctaves(center, rate, -gain)) < 1e-8, "cut and boost widths must mirror");
      }
    }
  }
  assert.equal(peakWidthOctaves(1000, 48000, 0.5), null, "below-threshold width is undefined");
  assert.equal(peakWidthOctaves(1000, 48000, -3), null);
  console.log("ok: DSP gate rejects false claims; edge roots invert cuts and reject undefined widths");

  // Independent golden-section solve with broad brackets rather than the model's
  // 32768-grid detection and ternary refinement. Verify both sides of the extrema.
  for (const [gains, bracket, expected] of [
    [[0, 0, 0, 0, 0, 12, 12, 0, 0, 0], [900, 1200], 14.564616258],
    [Array(10).fill(12), [350, 650], 18.402800135],
  ]) {
    const { evaluate } = compiledChain(gains, 48000);
    let [left, right] = bracket;
    const ratio = (Math.sqrt(5) - 1) / 2;
    for (let index = 0; index < 80; index += 1) {
      const a = right - ratio * (right - left);
      const b = left + ratio * (right - left);
      if (evaluate(a) < evaluate(b)) left = a;
      else right = b;
    }
    const frequency = (left + right) / 2;
    const peak = evaluate(frequency);
    assert.ok(Math.abs(peak - expected) < 1e-7, "independent extremum regression");
    assert.ok(Math.abs(gridPeak(gains, 48000) - peak) < 1e-7, "grid refinement misses independently solved extremum");
    assert.ok(evaluate(frequency - 0.1) < peak && evaluate(frequency + 0.1) < peak);
  }
  console.log("ok: independent golden-section extrema and both neighbors match the refined grid");
} catch (error) {
  console.error(error);
  process.exitCode = 1;
}
