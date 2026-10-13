// Independent RBJ numeric reference for AFX-2; no native-output claims.
const TAU = 2 * Math.PI;
const SQRT2 = Math.sqrt(2);

/** RBJ peaking, normalised by a0. Matches math.rs::Biquad::peaking and
 *  FFmpeg n6.0 af_biquads.c:842-847 (alpha = sin(w0)/(2*width) for width_type=q). */
export function peakingCoefficients(centerHz, sampleRate, gainDb) {
  if (gainDb === 0 || sampleRate === 0 || centerHz > 0.45 * sampleRate) {
    return { b0: 1, b1: 0, b2: 0, a1: 0, a2: 0, identity: true };
  }
  const amplitude = 10 ** (gainDb / 40);
  const omega = (TAU * centerHz) / sampleRate;
  const alpha = Math.sin(omega) / (2 * SQRT2);
  const a0 = 1 + alpha / amplitude;
  return {
    b0: (1 + alpha * amplitude) / a0,
    b1: (-2 * Math.cos(omega)) / a0,
    b2: (1 - alpha * amplitude) / a0,
    a1: (-2 * Math.cos(omega)) / a0,
    a2: (1 - alpha / amplitude) / a0,
    identity: false,
  };
}

/** |H| in dB at a frequency, per math.rs::Biquad::gain_db. */
export function responseDb(c, sampleRate, frequencyHz) {
  const cosine2 = 2 * Math.cos((TAU * frequencyHz) / sampleRate) ** 2 - 1;
  const sine2 = 2 * Math.sin((TAU * frequencyHz) / sampleRate) * Math.cos((TAU * frequencyHz) / sampleRate);
  const nr = c.b2 * cosine2 + c.b1 * Math.cos((TAU * frequencyHz) / sampleRate) + c.b0;
  const ni = c.b2 * sine2 + c.b1 * Math.sin((TAU * frequencyHz) / sampleRate);
  const dr = c.a2 * cosine2 + c.a1 * Math.cos((TAU * frequencyHz) / sampleRate) + 1;
  const di = c.a2 * sine2 + c.a1 * Math.sin((TAU * frequencyHz) / sampleRate);
  return 10 * Math.log10((nr * nr + ni * ni) / (dr * dr + di * di));
}

/** Product of section power responses, with one shared trig evaluation. */
export function compiledChain(gains, sampleRate) {
  const coefficients = BAND_FREQUENCIES
    .map((center, index) => peakingCoefficients(center, sampleRate, gains[index]))
    .filter((section) => !section.identity);
  const evaluateTrig = (sine, cosine) => {
    const cosine2 = 2 * cosine * cosine - 1;
    const sine2 = 2 * sine * cosine;
    let power = 1;
    for (const c of coefficients) {
      const nr = c.b2 * cosine2 + c.b1 * cosine + c.b0;
      const ni = c.b2 * sine2 + c.b1 * sine;
      const dr = c.a2 * cosine2 + c.a1 * cosine + 1;
      const di = c.a2 * sine2 + c.a1 * sine;
      power *= (nr * nr + ni * ni) / (dr * dr + di * di);
    }
    return 10 * Math.log10(power);
  };
  return {
    evaluateTrig,
    evaluate: (frequency) => {
      const omega = TAU * frequency / sampleRate;
      return evaluateTrig(Math.sin(omega), Math.cos(omega));
    },
  };
}

export function chainDb(gains, sampleRate, frequencyHz) {
  return compiledChain(gains, sampleRate).evaluate(frequencyHz);
}

/** Peak search mirroring math.rs::peak_gain: 32769-point grid plus ternary
 *  refinement on every sampled local maximum. */
export function gridPeak(gains, sampleRate) {
  const INTERVALS = 32768;
  const step = sampleRate / (2 * INTERVALS);
  const { evaluate } = compiledChain(gains, sampleRate);
  let maximum = Math.max(evaluate(0), evaluate(sampleRate / 2));
  for (const center of BAND_FREQUENCIES) {
    if (center <= 0.45 * sampleRate) maximum = Math.max(maximum, evaluate(center));
  }
  const refine = (low, high) => {
    let lo = low;
    let hi = high;
    for (let i = 0; i < 48; i += 1) {
      const left = lo + (hi - lo) / 3;
      const right = hi - (hi - lo) / 3;
      if (evaluate(left) < evaluate(right)) lo = left;
      else hi = right;
    }
    return evaluate((lo + hi) / 2);
  };
  let before = evaluate(0);
  let current = evaluate(step);
  for (let index = 2; index <= INTERVALS; index += 1) {
    const frequency = index * step;
    const after = evaluate(frequency);
    maximum = Math.max(maximum, current);
    if (current > before && current >= after) {
      maximum = Math.max(maximum, refine(frequency - 2 * step, frequency));
    }
    before = current;
    current = after;
  }
  return Math.max(maximum, current);
}

/** Independent uniform grid: exactly 2^20 intervals / 1,048,577 samples.
 *  This is a numerical comparison on listed curves, never a global upper bound.
 *  Normalized-frequency trig is shared across sample rates and cases. */
export const DENSE_INTERVALS = 1 << 20;
let denseTrig;
export function densePeak(gains, sampleRate) {
  if (!denseTrig) {
    const sine = new Float64Array(DENSE_INTERVALS + 1);
    const cosine = new Float64Array(DENSE_INTERVALS + 1);
    for (let index = 0; index <= DENSE_INTERVALS; index += 1) {
      const omega = Math.PI * index / DENSE_INTERVALS;
      sine[index] = Math.sin(omega);
      cosine[index] = Math.cos(omega);
    }
    denseTrig = { sine, cosine };
  }
  const { evaluateTrig } = compiledChain(gains, sampleRate);
  let maximum = -Infinity;
  let bestIndex = 0;
  for (let index = 0; index <= DENSE_INTERVALS; index += 1) {
    const value = evaluateTrig(denseTrig.sine[index], denseTrig.cosine[index]);
    if (value > maximum) {
      maximum = value;
      bestIndex = index;
    }
  }
  return { gainDb: maximum, frequencyHz: bestIndex * sampleRate / (2 * DENSE_INTERVALS), samples: DENSE_INTERVALS + 1 };
}

/** Edges relative to a peak/valley by a specified dB distance.
 *  Cuts use the reciprocal RBJ response, so their width mirrors boosts.
 *  A distance at least as large as |gain| has no finite pair of edges. */
export function peakWidthOctaves(centerHz, sampleRate, gainDb, distanceDb = 10 * Math.log10(2)) {
  if (Math.abs(gainDb) <= distanceDb || centerHz > 0.45 * sampleRate) return null;
  const section = peakingCoefficients(centerHz, sampleRate, gainDb);
  const sign = Math.sign(gainDb);
  const target = Math.abs(gainDb) - distanceDb;
  const evaluate = (frequency) => sign * responseDb(section, sampleRate, frequency) - target;
  const root = (low, high) => {
    let lo = low;
    let hi = high;
    let lowValue = evaluate(lo);
    if (lowValue * evaluate(hi) > 0) throw new Error("band edge is not bracketed");
    for (let index = 0; index < 80; index += 1) {
      const mid = (lo + hi) / 2;
      const value = evaluate(mid);
      if (value * lowValue > 0) { lo = mid; lowValue = value; }
      else hi = mid;
    }
    return (lo + hi) / 2;
  };
  const lower = root(0, centerHz);
  const upper = root(centerHz, sampleRate / 2);
  return Math.log2(upper / lower);
}

export const BAND_FREQUENCIES = [31.25, 62.5, 125, 250, 500, 1000, 2000, 4000, 8000, 16000];
