//! f64 RBJ peaking response and conservative headroom calculations. These
//! calculations are not native output, transient or true-peak measurements.

use super::{
    ChannelLayout, EffectsError, Payload, PreampMode, ProcessingEnvironment, BAND_FREQUENCIES,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Biquad {
    pub b0: f64,
    pub b1: f64,
    pub b2: f64,
    pub a1: f64,
    pub a2: f64,
}

impl Biquad {
    pub const IDENTITY: Self = Self {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    #[must_use]
    pub fn peaking(center_hz: f64, sample_rate: u32, gain_db: f64) -> Self {
        if gain_db == 0.0 || sample_rate == 0 || center_hz > 0.45 * f64::from(sample_rate) {
            return Self::IDENTITY;
        }
        let amplitude = 10_f64.powf(gain_db / 40.0);
        let omega = std::f64::consts::TAU * center_hz / f64::from(sample_rate);
        let alpha = omega.sin() / (2.0 * std::f64::consts::SQRT_2);
        let a0 = 1.0 + alpha / amplitude;
        Self {
            b0: (1.0 + alpha * amplitude) / a0,
            b1: -2.0 * omega.cos() / a0,
            b2: (1.0 - alpha * amplitude) / a0,
            a1: -2.0 * omega.cos() / a0,
            a2: (1.0 - alpha / amplitude) / a0,
        }
    }

    fn gain_db(self, sine: f64, cosine: f64) -> f64 {
        let cosine2 = (2.0 * cosine).mul_add(cosine, -1.0);
        let sine2 = 2.0 * sine * cosine;
        let numerator_real = self.b2.mul_add(cosine2, self.b1.mul_add(cosine, self.b0));
        let numerator_imaginary = self.b2.mul_add(sine2, self.b1 * sine);
        let denominator_real = self.a2.mul_add(cosine2, self.a1.mul_add(cosine, 1.0));
        let denominator_imaginary = self.a2.mul_add(sine2, self.a1 * sine);
        let numerator = numerator_real.mul_add(numerator_real, numerator_imaginary.powi(2));
        let denominator = denominator_real.mul_add(denominator_real, denominator_imaginary.powi(2));
        10.0 * (numerator / denominator).log10()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct EffectsAnalysis {
    pub effective_preamp_db: f64,
    pub safe_preamp_db: f64,
    pub peak_gain_db: f64,
    pub active_bands: [bool; 10],
    pub coefficients: [Biquad; 10],
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ResponsePoint {
    pub frequency_hz: f64,
    pub gain_db: f64,
}

/// Sum of individual dB responses equals the logarithm of their product.
#[must_use]
pub fn response_db(
    coefficients: &[Biquad; 10],
    sample_rate: u32,
    frequency_hz: f64,
    preamp_db: f64,
) -> f64 {
    let omega = std::f64::consts::TAU * frequency_hz / f64::from(sample_rate);
    let (sine, cosine) = omega.sin_cos();
    preamp_db
        + coefficients
            .iter()
            .map(|filter| filter.gain_db(sine, cosine))
            .sum::<f64>()
}

/// # Errors
/// Invalid payloads, zero rates and unsupported layouts are rejected atomically.
pub fn analyze(
    payload: &Payload,
    environment: ProcessingEnvironment,
) -> Result<EffectsAnalysis, EffectsError> {
    payload.validate()?;
    if environment.sample_rate == 0 || environment.channel_layout == ChannelLayout::Other {
        return Err(EffectsError::Unavailable);
    }
    match payload {
        Payload::Spatial(spatial) => {
            if environment.channel_layout != ChannelLayout::Stereo {
                return Err(EffectsError::Unavailable);
            }
            let peak_gain_db = 20.0 * spatial.width.log10();
            let safe_preamp_db = -peak_gain_db.max(0.0);
            Ok(EffectsAnalysis {
                // The final limiter provides the remaining sample-peak
                // protection; a small fixed lift avoids baking its full
                // threshold margin into every automatic setting.
                effective_preamp_db: safe_preamp_db + AUTO_HEADROOM_DB,
                safe_preamp_db,
                peak_gain_db,
                active_bands: [false; 10],
                coefficients: [Biquad::IDENTITY; 10],
            })
        }
        Payload::Eq(curve) => {
            let sample_rate = environment.sample_rate;
            let active_bands =
                BAND_FREQUENCIES.map(|frequency| frequency <= 0.45 * f64::from(sample_rate));
            let coefficients = std::array::from_fn(|index| {
                Biquad::peaking(BAND_FREQUENCIES[index], sample_rate, curve.gains_db[index])
            });
            let neutral = curve
                .gains_db
                .iter()
                .zip(active_bands)
                .all(|(gain, active)| !active || *gain == 0.0);
            let peak_gain_db = if neutral {
                0.0
            } else {
                peak_gain(&coefficients, sample_rate)
            };
            let safe_preamp_db = if neutral { 0.0 } else { -peak_gain_db.max(0.0) };
            let effective_preamp_db = match curve.preamp_mode {
                PreampMode::Auto => safe_preamp_db + if neutral { 0.0 } else { AUTO_HEADROOM_DB },
                PreampMode::Manual => curve.requested_preamp_db.min(safe_preamp_db),
            };
            Ok(EffectsAnalysis {
                effective_preamp_db,
                safe_preamp_db,
                peak_gain_db,
                active_bands,
                coefficients,
            })
        }
    }
}

fn peak_gain(coefficients: &[Biquad; 10], sample_rate: u32) -> f64 {
    const INTERVALS: u32 = 32768;
    let step = f64::from(sample_rate) / (2.0 * f64::from(INTERVALS));
    let evaluate = |frequency| response_db(coefficients, sample_rate, frequency, 0.0);
    let mut maximum = evaluate(0.0).max(evaluate(f64::from(sample_rate) / 2.0));
    for center in BAND_FREQUENCIES {
        if center <= 0.45 * f64::from(sample_rate) {
            maximum = maximum.max(evaluate(center));
        }
    }
    let mut before = evaluate(0.0);
    let mut current = evaluate(step);
    for index in 2..=INTERVALS {
        let frequency = f64::from(index) * step;
        let after = evaluate(frequency);
        maximum = maximum.max(current);
        if current > before && current >= after {
            maximum = maximum.max(refine_peak(
                &evaluate,
                (-2.0_f64).mul_add(step, frequency),
                frequency,
            ));
        }
        before = current;
        current = after;
    }
    maximum.max(current)
}

fn refine_peak(evaluate: &impl Fn(f64) -> f64, mut low: f64, mut high: f64) -> f64 {
    // Every sampled local maximum is refined. A narrow peak also has its exact
    // band center sampled above, independent of the uniform grid resolution.
    for _ in 0..48 {
        let left = low + (high - low) / 3.0;
        let right = high - (high - low) / 3.0;
        if evaluate(left) < evaluate(right) {
            low = left;
        } else {
            high = right;
        }
    }
    evaluate((low + high) / 2.0)
}

#[must_use]
pub fn response_points(analysis: &EffectsAnalysis, sample_rate: u32) -> Vec<ResponsePoint> {
    let high = (0.45 * f64::from(sample_rate)).min(20_000.0);
    if high <= 20.0 {
        return Vec::new();
    }
    (0..=200)
        .map(|index| {
            let frequency_hz = 20.0 * (high / 20.0).powf(f64::from(index) / 200.0);
            ResponsePoint {
                frequency_hz,
                gain_db: response_db(
                    &analysis.coefficients,
                    sample_rate,
                    frequency_hz,
                    analysis.effective_preamp_db,
                ),
            }
        })
        .collect()
}

/// Headroom the automatic preamp gives back above the strict peak-protection
/// floor, letting the fixed end limiter absorb small steady overshoots instead
/// of attenuating every sample. Shared so the steady state and the transition
/// path cannot drift apart: an earlier revision used 1.0 dB while the steady
/// state used 0.5 dB.
pub const AUTO_HEADROOM_DB: f64 = 0.5;

/// Peak of the actual intermediate EQ state, using the steady-state estimator.
/// Interpolated gains need not lie on the UI's 0.5 dB steps. Inactive bands are
/// identities, so retained values above the processing-rate boundary contribute
/// no headroom cost. This is a frequency-response estimate, not a ringing bound;
/// the end limiter remains responsible for transient sample peaks.
#[must_use]
pub(crate) fn intermediate_eq_peak_db(gains: [f64; 10], sample_rate: u32) -> f64 {
    let coefficients = std::array::from_fn(|index| {
        Biquad::peaking(BAND_FREQUENCIES[index], sample_rate, gains[index])
    });
    if coefficients
        .iter()
        .all(|filter| *filter == Biquad::IDENTITY)
    {
        0.0
    } else {
        peak_gain(&coefficients, sample_rate)
    }
}

#[cfg(test)]
mod tests;
