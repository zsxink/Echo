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
