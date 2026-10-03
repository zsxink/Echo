//! State transition, durable round-trip and stale-fact regression coverage.
#![allow(clippy::float_cmp)] // These assertions check exact retained inputs and exact constant responses.
use super::*;
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
