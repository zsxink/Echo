//! State transition, durable round-trip and stale-fact regression coverage.
#![allow(clippy::float_cmp)] // These assertions check exact retained inputs and exact constant responses.
use super::*;
use crate::effects::math::peak_scan;
use crate::effects::{ChannelLayout, PreampMode};

#[test]
fn closing_saved_and_draft_have_distinct_targets() {
    let mut state = EffectsState::default();
    state.select(&PresetId::builtin("rock")).unwrap();
    let retained = state.document.retained_payload.clone();
    state.set_enabled(false).unwrap();
    assert_eq!(state.document.selection, Selection::None);
    assert_eq!(state.document.retained_payload, retained);
    assert!(state.document.draft.is_none());
    assert_eq!(state.set_enabled(true), Err(EffectsError::NoTarget));
    let curve = EqCurve {
        gains_db: [1.0; 10],
        preamp_mode: PreampMode::Manual,
        requested_preamp_db: -3.0,
    };
    state.edit(curve.clone()).unwrap();
    state.set_enabled(false).unwrap();
    assert_eq!(state.document.draft, Some(curve));
    state.set_enabled(true).unwrap();
    assert!(state.document.requested_enabled);
}

#[test]
fn reset_and_epoch_reject_old_confirmations() {
    let mut state = EffectsState::default();
    state
        .edit(EqCurve {
            gains_db: [1.0; 10],
            ..EqCurve::default()
        })
        .unwrap();
    let mut receipt = state.runtime.clone();
    receipt.applied = AppliedState::Applied;
    receipt.effective_preamp_db = Some(-3.0);
    receipt.processing_rate = Some(48_000);
    receipt.channel_layout = Some(ChannelLayout::Stereo);
    state.reset().unwrap();
    assert!(!state.confirm(receipt.clone()));
    assert!(!state.document.requested_enabled);
    assert_eq!(state.document.retained_payload, Payload::default());
    state.select(&PresetId::builtin("pop")).unwrap();
    receipt.revision = state.runtime.revision;
    state
        .environment_changed(Some(ProcessingEnvironment {
            sample_rate: 22_050,
            channel_layout: ChannelLayout::Stereo,
        }))
        .unwrap();
    assert!(!state.confirm(receipt));
    assert_eq!(state.runtime.applied, AppliedState::Pending);
}

#[test]
fn disabled_draft_save_is_independent_and_metadata_does_not_touch_audio() {
    let mut state = EffectsState::default();
    let curve = EqCurve {
        gains_db: [2.0; 10],
        ..EqCurve::default()
    };
    state.edit(curve.clone()).unwrap();
    state.set_enabled(false).unwrap();
    let revision = state.runtime.revision;
    let id = PresetId::new_user();
    state
        .saved(UserPreset {
            id: id.clone(),
            name: " My\tcurve ".into(),
            curve,
        })
        .unwrap();
    assert!(!state.document.requested_enabled);
    assert_eq!(state.document.selection, Selection::Preset(id.clone()));
    assert_eq!(state.runtime.revision, revision);
    state.rename(&id, "renamed").unwrap();
    assert_eq!(state.runtime.revision, revision);
    state
        .edit(EqCurve {
            gains_db: [3.0; 10],
            ..EqCurve::default()
        })
        .unwrap();
    let draft = state.document.draft.clone();
    let runtime = state.runtime.clone();
    state.delete(&id).unwrap();
    assert_eq!(state.document.draft, draft);
    assert_eq!(state.runtime.revision, runtime.revision);
    assert!(state.document.requested_enabled);
}

#[test]
fn spatial_edit_replaces_type_and_unsupported_selection_is_atomic() {
    let mut state = EffectsState::default();
    state.select(&PresetId::builtin("surround")).unwrap();
    state.edit(EqCurve::default()).unwrap();
    assert!(matches!(state.document.retained_payload, Payload::Eq(_)));
    state
        .environment_changed(Some(ProcessingEnvironment {
            sample_rate: 48_000,
            channel_layout: ChannelLayout::Mono,
        }))
        .unwrap();
    let before = state.clone();
    assert_eq!(
        state.select(&PresetId::builtin("surround")),
        Err(EffectsError::Unavailable)
    );
    assert_eq!(state, before);
}

#[test]
fn observed_low_rate_disables_edits_without_deleting_values() {
    let mut state = EffectsState::default();
    state
        .edit(EqCurve {
            gains_db: [2.0; 10],
            ..EqCurve::default()
        })
        .unwrap();
    state
        .environment_changed(Some(ProcessingEnvironment {
            sample_rate: 22_050,
            channel_layout: ChannelLayout::Stereo,
        }))
        .unwrap();
    let before = state.clone();
    let mut edited = state.document.draft.clone().unwrap();
    edited.gains_db[9] = 3.0;
    assert_eq!(state.edit(edited), Err(EffectsError::Unavailable));
    assert_eq!(state, before);
    assert_eq!(state.document.draft.as_ref().unwrap().gains_db[9], 2.0);
}

#[test]
fn persistence_roundtrip_restores_requests_without_native_facts() {
    let mut state = EffectsState::default();
    state.select(&PresetId::builtin("rock")).unwrap();
    let encoded = serde_json::to_value(&state.document).unwrap();
    assert_eq!(encoded["retainedPayload"]["kind"], "eq");
    assert_eq!(encoded["selection"]["kind"], "preset");
    assert!(encoded.get("runtime").is_none());
    let restored = EffectsState::from_document(serde_json::from_value(encoded).unwrap()).unwrap();
    assert_eq!(restored.runtime.applied, AppliedState::Pending);
    assert!(restored.runtime.effective_preamp_db.is_none());
    let mut future = restored.document.clone();
    future.schema_version = 2;
    assert_eq!(
        EffectsState::from_document(future),
        Err(EffectsError::UnsupportedVersion)
    );
    let mut dangling = restored.document;
    dangling.selection = Selection::Preset(PresetId::new_user());
    assert_eq!(dangling.validate(), Err(EffectsError::NotFound));
}

#[test]
fn reference_response_and_registry_are_complete() {
    let state = EffectsState::default();
    let snapshot = state.snapshot();
    assert!(snapshot.reference_response);
    assert_eq!(snapshot.presets.len(), 10);
    assert_eq!(snapshot.response_points.len(), 201);
    assert!(snapshot
        .response_points
        .iter()
        .all(|point| point.gain_db == 0.0));
}

#[test]
fn applied_response_uses_confirmed_actual_preamp_and_failures_are_reference() {
    let mut state = EffectsState::default();
    state.edit(EqCurve::default()).unwrap();
    let mut receipt = state.runtime.clone();
    receipt.applied = AppliedState::Applied;
    receipt.processing_rate = Some(48_000);
    receipt.channel_layout = Some(ChannelLayout::Stereo);
    receipt.effective_preamp_db = Some(-4.0);
    assert!(state.confirm(receipt));
    let snapshot = state.snapshot();
    assert!(!snapshot.reference_response);
    assert!(snapshot
        .response_points
        .iter()
        .all(|point| point.gain_db == -4.0));
    state.runtime.applied = AppliedState::Failed;
    assert!(state.snapshot().reference_response);
}

#[test]
fn invalid_document_selection_snapshot_is_rejected() {
    let mut state = EffectsState::default();
    state.select(&PresetId::builtin("rock")).unwrap();
    state.document.retained_payload = Payload::default();
    assert_eq!(
        state.document.validate(),
        Err(EffectsError::InvalidDocument)
    );
}

#[test]
fn target_editability_is_independent_of_installed_bands_and_payload() {
    let mut state = EffectsState::default();
    state.select(&PresetId::builtin("surround")).unwrap();
    state
        .environment_changed(Some(ProcessingEnvironment {
            sample_rate: 22_050,
            channel_layout: ChannelLayout::Stereo,
        }))
        .unwrap();
    for status in [
        AppliedState::Pending,
        AppliedState::Bypassed,
        AppliedState::Failed,
        AppliedState::Applied,
    ] {
        state.runtime.applied = status;
        let snapshot = state.snapshot();
        assert_eq!(snapshot.runtime.active_bands, [false; 10]);
        assert_eq!(
            snapshot.editable_bands,
            Some([true, true, true, true, true, true, true, true, true, false])
        );
        assert_eq!(snapshot.response_rate, Some(22_050));
    }
    state.environment_changed(None).unwrap();
    assert_eq!(state.snapshot().editable_bands, Some([true; 10]));
    assert_eq!(state.snapshot().response_rate, Some(48_000));
}

#[test]
fn legacy_snapshot_without_additive_ui_fields_still_deserializes() {
    let mut json = serde_json::to_value(EffectsState::default().snapshot()).unwrap();
    let object = json.as_object_mut().unwrap();
    object.remove("editableBands");
    object.remove("responseRate");
    let restored: EffectsSnapshot = serde_json::from_value(json).unwrap();
    assert_eq!(restored.editable_bands, None);
    assert_eq!(restored.response_rate, None);
}

/// A curve that is not neutral, so the analysis really runs the peak sweep.
fn boosted_curve() -> EqCurve {
    EqCurve {
        gains_db: [3.0; 10],
        ..EqCurve::default()
    }
}

#[test]
fn effects_analysis_is_not_recomputed_for_unchanged_document_and_environment() {
    // The position stream repeats the snapshot ten times a second. The
    // frequency-response analysis runs a 32768-point peak sweep, so repeating
    // it per frame is what burned the CPU the position stream needs
    // (issue #112). An unchanged document and environment reuse the result.
    let mut state = EffectsState::default();
    state.edit(boosted_curve()).unwrap();
    let before = peak_scan::count();
    let first = state.snapshot();
    let after_first = peak_scan::count();
    assert!(
        after_first > before,
        "the first snapshot analyzes the curve"
    );
    let second = state.snapshot();
    assert_eq!(
        peak_scan::count(),
        after_first,
        "an unchanged document and environment reuse the analysis"
    );
    assert_eq!(
        first, second,
        "the reused analysis yields the same snapshot"
    );
}

#[test]
fn effects_analysis_matches_recomputation_for_same_inputs() {
    // The memoized result must be indistinguishable from a fresh computation.
    let mut state = EffectsState::default();
    state.edit(boosted_curve()).unwrap();
    let memoized = state.snapshot();
    // A second state with the same facts, but without the memo, recomputes.
    let mut fresh = EffectsState::from_document(state.document.clone()).unwrap();
    fresh.runtime = state.runtime.clone();
    assert_eq!(
        memoized,
        fresh.snapshot(),
        "the memoized analysis equals a fresh computation"
    );
}

#[test]
fn effects_analysis_is_recomputed_for_a_changed_document_or_environment() {
    // The memo is keyed by the document and the environment: a changed
    // document or a changed processing rate recomputes with the new inputs.
    let mut state = EffectsState::default();
    state.edit(boosted_curve()).unwrap();
    let first = state.snapshot();
    let before = peak_scan::count();
    state
        .edit(EqCurve {
            gains_db: [4.0; 10],
            ..EqCurve::default()
        })
        .unwrap();
    let second = state.snapshot();
    assert!(
        peak_scan::count() > before,
        "a changed document recomputes the analysis"
    );
    assert_ne!(first.response_points, second.response_points);

    let before = peak_scan::count();
    state
        .environment_changed(Some(ProcessingEnvironment {
            sample_rate: 96_000,
            channel_layout: ChannelLayout::Stereo,
        }))
        .unwrap();
    let third = state.snapshot();
    assert!(
        peak_scan::count() > before,
        "a changed environment recomputes the analysis"
    );
    assert_ne!(second.response_points, third.response_points);
}
