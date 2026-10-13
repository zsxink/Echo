//! Formula and protection tests are independent of native audio measurements.
#![allow(clippy::float_cmp)] // Exact zeros and assigned half-step gains are the contract under test.
use super::*;
use crate::effects::{presets, EqCurve, Spatial};

fn environment(rate: u32) -> ProcessingEnvironment {
    ProcessingEnvironment {
        sample_rate: rate,
        channel_layout: ChannelLayout::Stereo,
    }
}

#[test]
fn center_gain_and_neutral_response_are_exact() {
    for rate in [22_050, 44_100, 48_000, 96_000] {
        for (index, frequency) in BAND_FREQUENCIES.into_iter().enumerate() {
            if frequency > 0.45 * f64::from(rate) {
                continue;
            }
            for gain in [-12.0, 12.0] {
                let mut filters = [Biquad::IDENTITY; 10];
                filters[index] = Biquad::peaking(frequency, rate, gain);
                assert!((response_db(&filters, rate, frequency, 0.0) - gain).abs() < 0.00001);
            }
        }
        let result = analyze(&Payload::default(), environment(rate)).unwrap();
        assert_eq!(result.effective_preamp_db, 0.0);
    }
}

#[test]
fn pressure_and_low_rate_preserve_parameters() {
    for rate in [22_050, 44_100, 48_000, 96_000] {
        for gains_db in [
            [12.0; 10],
            [-12.0; 10],
            [0., 0., 12., 12., 0., 0., 0., 0., 0., 0.],
        ] {
            let curve = EqCurve {
                gains_db,
                preamp_mode: PreampMode::Manual,
                requested_preamp_db: 0.0,
            };
            let result = analyze(&Payload::Eq(curve), environment(rate)).unwrap();
            assert!(result.effective_preamp_db.is_finite());
            assert!(result.peak_gain_db + result.effective_preamp_db <= 0.50001);
            assert_eq!(result.active_bands[9], rate != 22_050);
            if rate == 22_050 {
                assert_eq!(result.coefficients[9], Biquad::IDENTITY);
            }
        }
    }
}

#[test]
fn conservative_auto_preamp_keeps_modeled_steady_peak_within_limiter_budget() {
    for rate in [22_050, 44_100, 48_000, 96_000] {
        for gains_db in [
            [12.0; 10],
            [-12.0; 10],
            [0., 12., 12., 0., -12., 0., 6., -6., 0., 12.],
        ] {
            let curve = EqCurve {
                gains_db,
                preamp_mode: PreampMode::Auto,
                requested_preamp_db: 0.0,
            };
            let analysis = analyze(&Payload::Eq(curve), environment(rate)).unwrap();
            assert!(analysis.effective_preamp_db.is_finite());
            assert!(analysis.peak_gain_db + analysis.effective_preamp_db <= 0.50001);
        }
    }
}

#[test]
fn published_design_comparison_is_reproducible() {
    let cases = [
        (
            "classical",
            [0., 0., 0., 0., 0., 0., 0.5, 0.5, 1., 1.5],
            1.57,
            0.61,
        ),
        (
            "electronic",
            [4., 5., 3., -1., -2., -1.5, 0., 2., 3., 2.],
            6.24,
            3.68,
        ),
        (
            "vocal",
            [-2., -2., -1.5, -0.5, 0., 1.5, 3., 1.5, -1.5, -1.],
            3.48,
            2.32,
        ),
        (
            "bass",
            [3., 6., 5., 1., 0., -0.5, -1., -1., -1.5, -2.],
            7.52,
            4.84,
        ),
    ];
    for (id, old_gains, old_peak, new_peak) in cases {
        let old = analyze(
            &Payload::Eq(EqCurve {
                gains_db: old_gains,
                ..EqCurve::default()
            }),
            environment(48_000),
        )
        .unwrap();
        let new = analyze(
            &presets::builtin(&crate::effects::PresetId::builtin(id))
                .unwrap()
                .payload,
            environment(48_000),
        )
        .unwrap();
        assert!((old.peak_gain_db - old_peak).abs() < 0.02);
        assert!((new.peak_gain_db - new_peak).abs() < 0.02);
        assert!((new.safe_preamp_db + new_peak).abs() < 0.02);
        assert!((new.effective_preamp_db - new.safe_preamp_db - 0.5).abs() < 0.001);
    }
}

#[test]
fn environment_spatial_and_validation_boundaries() {
    assert_eq!(
        analyze(&Payload::default(), environment(0)),
        Err(EffectsError::Unavailable)
    );
    let mut mono = environment(48_000);
    mono.channel_layout = ChannelLayout::Mono;
    assert!(analyze(&Payload::default(), mono).is_ok());
    assert_eq!(
        analyze(&Payload::Spatial(Spatial::default()), mono),
        Err(EffectsError::Unavailable)
    );
    let spatial = analyze(&Payload::Spatial(Spatial::default()), environment(48_000)).unwrap();
    assert!((spatial.peak_gain_db - 1.938_200_26).abs() < 0.000_001);
    assert!((spatial.effective_preamp_db + spatial.peak_gain_db - 0.5).abs() < 0.000_001);
    for invalid in [f64::NAN, f64::INFINITY, 12.5, 0.1] {
        let mut curve = EqCurve::default();
        curve.gains_db[0] = invalid;
        assert_eq!(curve.validate(), Err(EffectsError::InvalidParameter));
    }
}

#[test]
fn intermediate_peak_tracks_actual_mixed_sign_curves() {
    let left = Payload::Eq(EqCurve {
        gains_db: [12., -12., 12., -12., 12., -12., 12., -12., 12., -12.],
        ..EqCurve::default()
    });
    let right = Payload::Eq(EqCurve {
        gains_db: [-12., 12., -12., 12., -12., 12., -12., 12., -12., 12.],
        ..EqCurve::default()
    });
    for payload in [&left, &right, &Payload::default()] {
        let Payload::Eq(curve) = payload else {
            unreachable!()
        };
        let actual = intermediate_eq_peak_db(curve.gains_db, 48_000);
        let expected = analyze(payload, environment(48_000)).unwrap().peak_gain_db;
        assert!((actual - expected).abs() < 0.000_001);
        assert!(
            actual < 20.0,
            "independent band maxima cannot replace composed response"
        );
    }
}

#[test]
fn intermediate_peak_accepts_fractional_steps_and_ignores_inactive_bands() {
    let mut gains = [0.0; 10];
    gains[5] = 3.125;
    assert!((intermediate_eq_peak_db(gains, 48_000) - 3.125).abs() < 0.000_001);
    gains[9] = 12.0;
    assert!((intermediate_eq_peak_db(gains, 22_050) - 3.125).abs() < 0.000_001);
    gains[5] = 0.0;
    assert_eq!(intermediate_eq_peak_db(gains, 22_050), 0.0);
}

#[test]
fn off_center_response_matches_independent_rbj_reference() {
    // Frozen independent RBJ evaluations, away from the center where an alpha,
    // a2 or frequency-rate error could still pass the nominal-center test.
    let cases = [
        (1_000.0, 48_000, 6.0, 997.0, 5.999_528_772_998_522),
        (1_000.0, 48_000, 12.0, 700.0, 5.833_519_889_402_339),
        (16_000.0, 48_000, -12.0, 12_000.0, -3.576_101_666_973_781),
        (31.25, 22_050, 12.0, 20.0, 4.638_324_941_949_091),
        (4_000.0, 96_000, -6.0, 7_300.0, -1.399_389_518_910_484),
    ];
    for (center, rate, gain, frequency, expected) in cases {
        let mut filters = [Biquad::IDENTITY; 10];
        filters[0] = Biquad::peaking(center, rate, gain);
        let actual = response_db(&filters, rate, frequency, 0.0);
        assert!(
            (actual - expected).abs() < 1e-7,
            "{center}@{rate}: {actual} != {expected}"
        );
        filters[0] = Biquad::peaking(center, rate, -gain);
        assert!((response_db(&filters, rate, frequency, 0.0) + expected).abs() < 1e-7);
    }
}

#[test]
fn composed_off_center_response_matches_four_rate_reference() {
    let gains = [1., 2., 1., -1., -0.5, 0., 1., 2., 1., 0.];
    for (rate, expected) in [
        (22_050, 1.043_598_264_372_979),
        (44_100, 1.105_245_881_414_781),
        (48_000, 1.108_641_416_971_494),
        (96_000, 1.122_604_102_382_467),
    ] {
        let filters = std::array::from_fn(|index| {
            Biquad::peaking(BAND_FREQUENCIES[index], rate, gains[index])
        });
        assert!((response_db(&filters, rate, 1_750.0, -3.0) - (expected - 3.0)).abs() < 1e-7);
    }
}

#[test]
fn adjacent_and_all_boost_extrema_match_independent_peak_solution() {
    for (gains_db, peak_hz, peak_db) in [
        (
            [0., 0., 0., 0., 0., 12., 12., 0., 0., 0.],
            1_025.35,
            14.564_616_258,
        ),
        ([12.; 10], 499.90, 18.402_800_135),
    ] {
        let result = analyze(
            &Payload::Eq(EqCurve {
                gains_db,
                ..EqCurve::default()
            }),
            environment(48_000),
        )
        .unwrap();
        assert!((result.peak_gain_db - peak_db).abs() < 1e-7);
        let peak = response_db(&result.coefficients, 48_000, peak_hz, 0.0);
        assert!((peak - peak_db).abs() < 1e-6);
        for neighbor in [peak_hz - 0.25, peak_hz + 0.25] {
            assert!(response_db(&result.coefficients, 48_000, neighbor, 0.0) < peak);
        }
    }
}
