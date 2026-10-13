//! Fault injection verifies committed lists, request tails and safe compensation.
#![allow(clippy::float_cmp)] // These assertions check exact persisted user-entered half-step values.
use super::*;
use crate::effects::{AppliedState, Selection};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};

#[derive(Default)]
struct Preferences {
    saved: Mutex<Option<EffectsDocument>>,
    fail: AtomicBool,
    writes: AtomicUsize,
}
impl EffectsPreferencesPort for Preferences {
    fn load(&self) -> Result<PreferencesRecovery, String> {
        Ok(self
            .saved
            .lock()
            .unwrap()
            .clone()
            .map_or(PreferencesRecovery::Missing, PreferencesRecovery::Ready))
    }
    fn write(&self, document: &EffectsDocument, _: PreferencesWrite) -> Result<(), String> {
        if self.fail.load(Ordering::SeqCst) {
            return Err("injected write failure".into());
        }
        *self.saved.lock().unwrap() = Some(document.clone());
        self.writes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}
#[derive(Default)]
struct Playback {
    runtime: Mutex<EffectsRuntime>,
    submissions: AtomicUsize,
    fail_bypass: AtomicBool,
    fail_submit: AtomicBool,
    active: AtomicBool,
}
impl EffectsPlaybackPort for Playback {
    fn submit(&self, state: &EffectsState) -> Result<EffectsRuntime, String> {
        self.submissions.fetch_add(1, Ordering::SeqCst);
        if self.fail_submit.load(Ordering::SeqCst) {
            return Err("injected apply failure".into());
        }
        let mut runtime = state.runtime.clone();
        runtime.applied = if state.document.requested_enabled {
            if self.active.load(Ordering::SeqCst) {
                AppliedState::Applied
            } else {
                AppliedState::Pending
            }
        } else {
            AppliedState::Bypassed
        };
        if runtime.applied == AppliedState::Applied {
            runtime.processing_rate = Some(48_000);
            runtime.channel_layout = Some(crate::effects::ChannelLayout::Stereo);
            runtime.effective_preamp_db = Some(0.0);
        }
        *self.runtime.lock().unwrap() = runtime.clone();
        Ok(runtime)
    }
    fn bypass(&self, state: &EffectsState) -> Result<EffectsRuntime, String> {
        if self.fail_bypass.load(Ordering::SeqCst) {
            return Err("injected bypass failure".into());
        }
        let mut runtime = state.runtime.clone();
        runtime.applied = AppliedState::Bypassed;
        *self.runtime.lock().unwrap() = runtime.clone();
        Ok(runtime)
    }
    fn runtime(&self) -> EffectsRuntime {
        self.runtime.lock().unwrap().clone()
    }
}
fn fixture() -> (EffectsService, Arc<Preferences>, Arc<Playback>) {
    let preferences = Arc::new(Preferences::default());
    let playback = Arc::new(Playback::default());
    playback.active.store(true, Ordering::SeqCst);
    let service = EffectsService::restore(preferences.clone(), playback.clone());
    (service, preferences, playback)
}
fn draft(service: &EffectsService) {
    service.edit(EqCurve::default()).unwrap();
}
fn save(service: &EffectsService) -> PresetId {
    draft(service);
    let state = service.save("My curve").unwrap();
    state.document.user_presets[0].id.clone()
}

#[test]
fn failed_save_keeps_draft_and_list_until_retry_commits() {
    let (service, preferences, _) = fixture();
    draft(&service);
    preferences.fail.store(true, Ordering::SeqCst);
    assert!(service.save("My curve").is_err());
    assert!(service.snapshot().document.user_presets.is_empty());
    assert!(service.snapshot().document.draft.is_some());
    preferences.fail.store(false, Ordering::SeqCst);
    assert_eq!(
        service
            .save("My curve")
            .unwrap()
            .document
            .user_presets
            .len(),
        1
    );
}
#[test]
fn saving_closed_draft_does_not_enable_or_submit_audio() {
    let (service, _, playback) = fixture();
    draft(&service);
    service.set_enabled(false).unwrap();
    let before = playback.submissions.load(Ordering::SeqCst);
    let state = service.save("Closed curve").unwrap();
    assert!(!state.document.requested_enabled);
    assert_eq!(playback.submissions.load(Ordering::SeqCst), before);
}
#[test]
fn rename_has_no_audio_side_effect_and_failed_commit_keeps_name() {
    let (service, preferences, playback) = fixture();
    let id = save(&service);
    let before = playback.submissions.load(Ordering::SeqCst);
    service.rename(&id, "Renamed").unwrap();
    assert_eq!(playback.submissions.load(Ordering::SeqCst), before);
    preferences.fail.store(true, Ordering::SeqCst);
    assert!(service.rename(&id, "Lost").is_err());
    assert_eq!(service.snapshot().document.user_presets[0].name, "Renamed");
}
#[test]
fn delete_bypass_failure_does_not_commit_or_publish() {
    let (service, preferences, playback) = fixture();
    let id = save(&service);
    let before = preferences.writes.load(Ordering::SeqCst);
    playback.fail_bypass.store(true, Ordering::SeqCst);
    assert!(service.delete(&id).is_err());
    assert_eq!(preferences.writes.load(Ordering::SeqCst), before);
    assert_eq!(service.snapshot().document.selection, Selection::Preset(id));
}
#[test]
fn delete_write_failure_restores_original_request_and_list() {
    let (service, preferences, playback) = fixture();
    let id = save(&service);
    preferences.fail.store(true, Ordering::SeqCst);
    let before = playback.submissions.load(Ordering::SeqCst);
    assert!(service.delete(&id).is_err());
    let state = service.snapshot();
    assert_eq!(state.document.selection, Selection::Preset(id));
    assert!(state.document.requested_enabled);
    assert_eq!(state.document.user_presets.len(), 1);
    assert_eq!(playback.submissions.load(Ordering::SeqCst), before + 1);
    assert_eq!(state.runtime.applied, AppliedState::Applied);
    assert_eq!(state.runtime.persistence_status, PersistenceStatus::Failed);
}
#[test]
fn failed_delete_compensation_never_claims_original_effect_is_applied() {
    let (service, preferences, playback) = fixture();
    let id = save(&service);
    preferences.fail.store(true, Ordering::SeqCst);
    playback.fail_submit.store(true, Ordering::SeqCst);
    assert!(service.delete(&id).is_err());
    let state = service.snapshot();
    assert_eq!(state.document.user_presets.len(), 1);
    assert_ne!(state.runtime.applied, AppliedState::Applied);
}
#[test]
fn flush_retries_failed_tail_and_keeps_last_edit() {
    let (service, preferences, _) = fixture();
    preferences.fail.store(true, Ordering::SeqCst);
    let mut curve = EqCurve::default();
    curve.gains_db[0] = 2.0;
    service.edit(curve.clone()).unwrap();
    assert!(service.flush().is_err());
    curve.gains_db[0] = 4.0;
    service.edit(curve).unwrap();
    preferences.fail.store(false, Ordering::SeqCst);
    service.flush().unwrap();
    assert_eq!(
        preferences
            .saved
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .draft
            .as_ref()
            .unwrap()
            .gains_db[0],
        4.0
    );
}
#[test]
fn restores_request_as_pending_without_starting_audio() {
    let (service, preferences, _) = fixture();
    draft(&service);
    service.flush().unwrap();
    let playback = Arc::new(Playback::default());
    let restored = EffectsService::restore(preferences, playback.clone());
    assert!(restored.snapshot().document.requested_enabled);
    assert_eq!(restored.snapshot().runtime.applied, AppliedState::Pending);
    assert_eq!(playback.submissions.load(Ordering::SeqCst), 1);
}
#[test]
fn merges_edits_and_flushes_final_tail() {
    let (service, preferences, _) = fixture();
    for gain in 1..=8 {
        let mut curve = EqCurve::default();
        curve.gains_db[0] = f64::from(gain);
        service.edit(curve).unwrap();
    }
    assert_eq!(preferences.writes.load(Ordering::SeqCst), 0);
    service.flush().unwrap();
    assert_eq!(preferences.writes.load(Ordering::SeqCst), 1);
    assert_eq!(
        preferences
            .saved
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .draft
            .as_ref()
            .unwrap()
            .gains_db[0],
        8.0
    );
}

#[test]
fn captures_order_metadata_and_persistence_without_audio_revisions() {
    let (service, _, playback) = fixture();
    let edited = service.edit(EqCurve::default()).unwrap();
    let before_save = service.snapshot();
    let submissions = playback.submissions.load(Ordering::SeqCst);
    let saved = service.save("Ordered curve").unwrap();
    let id = saved.document.user_presets[0].id.clone();
    let renamed = service.rename(&id, "Renamed curve").unwrap();
    let queried = service.snapshot();
    // Deleting an unselected preset is metadata work too.
    service.set_enabled(false).unwrap();
    let before_delete = service.snapshot();
    let submissions_before_delete = playback.submissions.load(Ordering::SeqCst);
    let deleted = service.delete(&id).unwrap();
    let final_query = service.snapshot();
    assert!(edited.snapshot_sequence < before_save.snapshot_sequence);
    assert!(before_save.snapshot_sequence < saved.snapshot_sequence);
    assert!(saved.snapshot_sequence < renamed.snapshot_sequence);
    assert!(renamed.snapshot_sequence < queried.snapshot_sequence);
    assert!(before_delete.snapshot_sequence < deleted.snapshot_sequence);
    assert!(deleted.snapshot_sequence < final_query.snapshot_sequence);
    assert_eq!(saved.runtime.revision, edited.runtime.revision);
    assert_eq!(renamed.runtime.revision, edited.runtime.revision);
    assert_eq!(queried.runtime.revision, edited.runtime.revision);
    assert_eq!(deleted.runtime.revision, before_delete.runtime.revision);
    assert_eq!(submissions_before_delete, submissions + 1);
    assert_eq!(
        playback.submissions.load(Ordering::SeqCst),
        submissions_before_delete
    );
    assert_eq!(saved.document.user_presets[0].name, "Ordered curve");
    assert_eq!(queried.document.user_presets[0].name, "Renamed curve");
    assert!(final_query.document.user_presets.is_empty());
}

#[test]
fn query_orders_native_confirmation_and_persistence_receipts() {
    let (service, _, playback) = fixture();
    playback.active.store(false, Ordering::SeqCst);
    let pending = service.edit(EqCurve::default()).unwrap();
    {
        let mut runtime = playback.runtime.lock().unwrap();
        runtime.applied = AppliedState::Applied;
        runtime.processing_rate = Some(48_000);
        runtime.channel_layout = Some(crate::effects::ChannelLayout::Stereo);
        runtime.effective_preamp_db = Some(0.0);
    }
    let applied = service.snapshot();
    let persisted = service.flush().unwrap();
    assert!(pending.snapshot_sequence < applied.snapshot_sequence);
    assert!(applied.snapshot_sequence < persisted.snapshot_sequence);
    assert_eq!(pending.runtime.revision, persisted.runtime.revision);
    assert_eq!(applied.runtime.applied, AppliedState::Applied);
    assert_eq!(
        persisted.runtime.persistence_status,
        PersistenceStatus::Saved
    );
}

struct ProtectedPreferences {
    writes: AtomicUsize,
}
impl EffectsPreferencesPort for ProtectedPreferences {
    fn load(&self) -> Result<PreferencesRecovery, String> {
        Ok(PreferencesRecovery::Protected {
            raw: Some(serde_json::json!({ "schemaVersion": 999 })),
            reason: "unsupported effects document".into(),
        })
    }
    fn write(&self, _: &EffectsDocument, mode: PreferencesWrite) -> Result<(), String> {
        assert_eq!(mode, PreferencesWrite::Repair);
        self.writes.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

#[test]
fn recovery_projection_survives_actor_queries_until_explicit_repair() {
    let preferences = Arc::new(ProtectedPreferences {
        writes: AtomicUsize::new(0),
    });
    let playback = Arc::new(Playback::default());
    let service = EffectsService::restore(preferences.clone(), playback.clone());
    for _ in 0..3 {
        let state = service.snapshot();
        assert_eq!(
            state.recovery_reason.as_deref(),
            Some("unsupported effects document")
        );
        assert_eq!(state.runtime.persistence_status, PersistenceStatus::Failed);
        let dto = crate::ipc::effects::EffectsSnapshotDto::from(state.snapshot());
        let value = serde_json::to_value(dto).unwrap();
        assert_eq!(value["recoveryReason"], "unsupported effects document");
        assert!(value["snapshotSequence"].as_u64().is_some());
    }
    assert_eq!(preferences.writes.load(Ordering::SeqCst), 0);
    assert_eq!(playback.submissions.load(Ordering::SeqCst), 0);
    let repaired = service.retry_persistence().unwrap();
    assert_eq!(repaired.recovery_reason, None);
    assert_eq!(
        repaired.runtime.persistence_status,
        PersistenceStatus::Saved
    );
    assert_eq!(service.snapshot().recovery_reason, None);
    assert_eq!(preferences.writes.load(Ordering::SeqCst), 1);
}
