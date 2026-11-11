use super::*;
use crate::effects::math::{analyze, EffectsAnalysis};
use crate::effects::{ChannelLayout, EqCurve, Payload, ProcessingEnvironment};
use echo_core::domain::media::LoudnessTags;

fn analysis(effective_preamp_db: f64) -> EffectsAnalysis {
    let mut analysis = analyze(
        &Payload::Eq(EqCurve::default()),
        ProcessingEnvironment {
            sample_rate: 48_000,
            channel_layout: ChannelLayout::Stereo,
        },
    )
    .expect("neutral EQ is always analyzable");
    analysis.effective_preamp_db = effective_preamp_db;
    analysis
}

fn rg(track_mdb: i32, album_mdb: i32, track_peak_ppm: Option<u32>) -> LoudnessTags {
    LoudnessTags::new(
        LoudnessSource::ReplayGain2,
        Some(track_mdb),
        Some(album_mdb),
        track_peak_ppm,
        None,
    )
}

fn close(actual: f64, expected: f64) {
    assert!(
        (actual - expected).abs() < 1e-9,
        "expected {expected}, got {actual}"
    );
}

fn preference(mode: NormalizationMode, preamp_db: f64) -> NormalizationPreference {
    NormalizationPreference {
        enabled: true,
        mode,
        preamp_db,
        ..NormalizationPreference::default()
    }
}

#[test]
fn disabled_preference_keeps_the_p0_preamp_exactly() {
    let result = compute(
        Some(rg(-6_000, -8_000, None)),
        &NormalizationPreference::default(),
        &analysis(-3.5),
    )
    .expect("compute");
    close(result.gain_db, 0.0);
    close(result.combined_preamp_db, -3.5);
    assert_eq!(result.status, NormalizationStatus::Disabled);
    assert_eq!(result.source, None);
}

#[test]
fn no_tags_or_gainless_tags_are_neutral() {
    for tags in [
        None,
        Some(LoudnessTags::new(
            LoudnessSource::ReplayGain2,
            None,
            None,
            Some(500_000),
            None,
        )),
    ] {
        let result = compute(
            tags,
            &preference(NormalizationMode::Track, 0.0),
            &analysis(0.0),
        )
        .expect("compute");
        close(result.gain_db, 0.0);
        assert_eq!(result.status, NormalizationStatus::NoTags);
        close(result.combined_preamp_db, 0.0);
    }
}

#[test]
fn track_and_album_modes_select_their_own_pair_with_fallback() {
    let tags = rg(-6_000, -8_000, None);
    let track = compute(
        Some(tags),
        &preference(NormalizationMode::Track, 0.0),
        &analysis(0.0),
    )
    .expect("track");
    let album = compute(
        Some(tags),
        &preference(NormalizationMode::Album, 0.0),
        &analysis(0.0),
    )
    .expect("album");
    close(track.gain_db, -6.0);
    close(album.gain_db, -8.0);

    let album_only = LoudnessTags::new(LoudnessSource::ReplayGain2, None, Some(-4_000), None, None);
    let fallback = compute(
        Some(album_only),
        &preference(NormalizationMode::Track, 0.0),
        &analysis(0.0),
    )
    .expect("fallback");
    close(fallback.gain_db, -4.0);
}

#[test]
fn r128_is_aligned_to_the_common_reference() {
    let tags = LoudnessTags::new(LoudnessSource::R128, Some(-6_000), None, None, None);
    let result = compute(
        Some(tags),
        &preference(NormalizationMode::Track, 0.0),
        &analysis(0.0),
    )
    .expect("compute");
    close(R128_ALIGNMENT_DB, 5.0);
    close(result.gain_db, -1.0);
    assert_eq!(result.source, Some(LoudnessSource::R128));
}

#[test]
fn user_preamp_is_added_after_source_alignment() {
    let result = compute(
        Some(rg(-6_000, -8_000, None)),
        &preference(NormalizationMode::Track, 3.0),
        &analysis(0.0),
    )
    .expect("compute");
    close(result.gain_db, -3.0);
    assert_eq!(result.status, NormalizationStatus::Applied);
}

#[test]
fn peak_clamping_prevents_steady_state_clipping() {
    let tags = LoudnessTags::new(
        LoudnessSource::ReplayGain2,
        Some(9_000),
        None,
        Some(890_000), // 0.89 amplitude
        None,
    );
    let result = compute(
        Some(tags),
        &preference(NormalizationMode::Track, 0.0),
        &analysis(0.0),
    )
    .expect("compute");
    let expected = (-20.0f64).mul_add(0.89_f64.log10(), -PEAK_HEADROOM_DB);
    close(result.gain_db, expected);
    assert!(result.gain_db < 9.0);
    assert_eq!(result.status, NormalizationStatus::ClampedByPeak);
}

#[test]
fn missing_peak_is_capped_and_reported() {
    let tags = LoudnessTags::new(LoudnessSource::R128, Some(9_000), None, None, None);
    let result = compute(
        Some(tags),
        &preference(NormalizationMode::Track, 0.0),
        &analysis(0.0),
    )
    .expect("compute");
    close(result.gain_db, MAX_BOOST_DB);
    assert_eq!(result.status, NormalizationStatus::LimitedByCap);
}

#[test]
fn combined_preamp_adds_to_eq_protection_without_overwriting_it() {
    let result = compute(
        Some(rg(2_000, 2_000, None)),
        &preference(NormalizationMode::Track, 0.0),
        &analysis(-3.5),
    )
    .expect("compute");
    close(result.gain_db, 2.0);
    close(result.combined_preamp_db, -1.5);
    assert!(result.combined_preamp_db <= MAX_COMBINED_DB);
}

#[test]
fn combined_preamp_stays_finite_and_bounded() {
    for effective in [-80.0, -3.0, 0.0, 5.0] {
        let result = compute(
            Some(LoudnessTags::new(
                LoudnessSource::R128,
                Some(20_000),
                None,
                None,
                None,
            )),
            &preference(NormalizationMode::Track, 12.0),
            &analysis(effective),
        )
        .expect("compute");
        assert!(result.combined_preamp_db.is_finite());
        assert!(result.combined_preamp_db >= MIN_COMBINED_DB);
        assert!(result.combined_preamp_db <= MAX_COMBINED_DB);
    }
}

#[test]
fn illegal_preferences_are_rejected_not_clamped() {
    for preamp_db in [f64::NAN, f64::INFINITY, -12.5, 12.5, 0.25, 100.0] {
        let pref = preference(NormalizationMode::Track, preamp_db);
        assert_eq!(
            compute(None, &pref, &analysis(0.0)),
            Err(NormalizationError::InvalidPreference)
        );
    }
    let future = NormalizationPreference {
        schema_version: NORMALIZATION_SCHEMA_VERSION + 1,
        ..NormalizationPreference::default()
    };
    assert_eq!(
        compute(None, &future, &analysis(0.0)),
        Err(NormalizationError::InvalidPreference)
    );
    for preamp_db in [-12.0, -0.5, 0.0, 0.5, 12.0] {
        assert!(preference(NormalizationMode::Album, preamp_db)
            .validate()
            .is_ok());
    }
}

#[test]
fn same_inputs_are_deterministic() {
    let tags = Some(rg(-7_500, -9_000, Some(950_000)));
    let pref = preference(NormalizationMode::Album, 1.5);
    let analysis = analysis(-2.0);
    let first = compute(tags, &pref, &analysis).expect("first");
    let second = compute(tags, &pref, &analysis).expect("second");
    assert_eq!(first, second);
    assert!(first.gain_db.is_finite());
}
