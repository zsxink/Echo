//! Serial desktop effects use cases, with independent audio and durable requests.
//!
//! This service is composed once per player. Its worker merges preference edits;
//! player actor remains the sole native writer. No library or sync state is used.

mod management;
pub mod player_adapter;
pub(crate) mod recovery;
mod worker;

use crate::effects::{
    EffectsDocument, EffectsError, EffectsRuntime, EffectsState, EqCurve, PersistenceStatus,
    PresetId,
};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Restore outcome distinguishes missing preferences from protected originals.
#[derive(Clone, Debug)]
pub enum PreferencesRecovery {
    Missing,
    Ready(EffectsDocument),
    Protected {
        raw: Option<serde_json::Value>,
        reason: String,
    },
}

/// Only a deliberate repair may replace a document which failed recovery.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PreferencesWrite {
    Normal,
    Repair,
}

/// Local durable boundary; implementations must preserve unrelated fields.
pub trait EffectsPreferencesPort: Send + Sync {
    /// # Errors
    /// Returns IO failures; corrupt data is returned as a protected recovery outcome.
    fn load(&self) -> Result<PreferencesRecovery, String>;
    /// # Errors
    /// Returns protection or atomic write failures without replacing the old file.
    fn write(&self, document: &EffectsDocument, mode: PreferencesWrite) -> Result<(), String>;
}

/// Player actor boundary. Bypass must return a confirmed safe result.
pub trait EffectsPlaybackPort: Send + Sync {
    /// # Errors
    /// Returns actor request failures; acceptance may still be Pending.
    fn submit(&self, state: &EffectsState) -> Result<EffectsRuntime, String>;
    /// # Errors
    /// Returns failure when safety bypass cannot be confirmed.
    fn bypass(&self, state: &EffectsState) -> Result<EffectsRuntime, String>;
    fn runtime(&self) -> EffectsRuntime;
}

#[derive(Debug, thiserror::Error)]
pub enum EffectsServiceError {
    #[error(transparent)]
    Rules(#[from] EffectsError),
    #[error("effects preference write failed: {0}")]
    Storage(String),
    #[error("effects playback request failed: {0}")]
    Playback(String),
    #[error("effects preferences require explicit repair: {0}")]
    Protected(String),
    #[error("save requires an EQ draft")]
    NotDraft,
}

struct Inner {
    state: EffectsState,
    pending_since: Option<Instant>,
    protected: Option<String>,
}
struct Shared {
    inner: Mutex<Inner>,
    preferences: Arc<dyn EffectsPreferencesPort>,
    playback: Arc<dyn EffectsPlaybackPort>,
}

/// Owns the one global request and preference worker; window lifetimes do not.
pub struct EffectsService {
    shared: Arc<Shared>,
    worker: worker::PreferenceWorker,
}

impl EffectsService {
    /// Restore requests without starting playback or claiming Applied.
    pub fn restore(
        preferences: Arc<dyn EffectsPreferencesPort>,
        playback: Arc<dyn EffectsPlaybackPort>,
    ) -> Self {
        let (mut state, protected) = recovery::restore(preferences.load());
        // Publishing a target does not play/load a file: the existing actor
        // holds this request behind its first-sample/environment barrier.
        if protected.is_none() {
            match playback.submit(&state) {
                Ok(runtime) => reconcile(&mut state, runtime),
                Err(reason) => {
                    state.runtime.applied = crate::effects::AppliedState::Failed;
                    state.runtime.reason = Some(reason);
                }
            }
        }
        let shared = Arc::new(Shared {
            inner: Mutex::new(Inner {
                state,
                pending_since: None,
                protected,
            }),
            preferences,
            playback,
        });
        let worker = worker::PreferenceWorker::start(shared.clone());
        Self { shared, worker }
    }

    /// Snapshot reconciles actor receipts only for the latest request/environment.
    #[must_use]
    pub fn snapshot(&self) -> EffectsState {
        let mut inner = self
            .shared
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let runtime = self.shared.playback.runtime();
        reconcile(&mut inner.state, runtime);
        inner.state.clone()
    }

    /// # Errors
    /// Returns invalid request, protected-document or preference/playback failures.
    pub fn select(&self, id: &PresetId) -> Result<EffectsState, EffectsServiceError> {
        self.request(|state| state.select(id))
    }
    /// # Errors
    /// Returns invalid request, protected-document or preference/playback failures.
    pub fn edit(&self, curve: EqCurve) -> Result<EffectsState, EffectsServiceError> {
        self.request(|state| state.edit(curve))
    }
    /// # Errors
    /// Returns invalid request, protected-document or preference/playback failures.
    pub fn set_enabled(&self, enabled: bool) -> Result<EffectsState, EffectsServiceError> {
        self.request(|state| state.set_enabled(enabled))
    }
    /// # Errors
    /// Returns invalid request, protected-document or preference/playback failures.
    pub fn reset(&self) -> Result<EffectsState, EffectsServiceError> {
        self.request(EffectsState::reset)
    }

    fn request(
        &self,
        change: impl FnOnce(&mut EffectsState) -> Result<(), EffectsError>,
    ) -> Result<EffectsState, EffectsServiceError> {
        let mut inner = self
            .shared
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        reconcile(&mut inner.state, self.shared.playback.runtime());
        let mut next = inner.state.clone();
        change(&mut next)?;
        match self.shared.playback.submit(&next) {
            Ok(runtime) => reconcile(&mut next, runtime),
            Err(reason) => {
                next.runtime.applied = crate::effects::AppliedState::Failed;
                next.runtime.reason = Some(reason);
            }
        }
        next.runtime.persistence_status = PersistenceStatus::Unsaved;
        inner.state = next;
        inner.pending_since = Some(Instant::now());
        self.worker.wake();
        Ok(inner.state.clone())
    }

    /// Flush the final tail on normal process exit; background hiding does not call it.
    /// # Errors
    /// Returns invalid request, protected-document or preference/playback failures.
    pub fn flush(&self) -> Result<EffectsState, EffectsServiceError> {
        let mut inner = self
            .shared
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        persist(&self.shared, &mut inner, PreferencesWrite::Normal)?;
        Ok(inner.state.clone())
    }

    /// Explicit repair keeps the saved recovery bytes, then commits the retained request.
    /// # Errors
    /// Returns invalid request, protected-document or preference/playback failures.
    pub fn retry_persistence(&self) -> Result<EffectsState, EffectsServiceError> {
        let mut inner = self
            .shared
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        persist(&self.shared, &mut inner, PreferencesWrite::Repair)?;
        inner.protected = None;
        Ok(inner.state.clone())
    }

    /// Retry the retained audio request and deliberately repair local preferences.
    /// # Errors
    /// Returns invalid request, protected-document or preference/playback failures.
    pub fn retry(&self) -> Result<EffectsState, EffectsServiceError> {
        let mut inner = self
            .shared
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        inner.state.runtime.revision = inner
            .state
            .runtime
            .revision
            .checked_add(1)
            .ok_or(EffectsError::SequenceOverflow)?;
        match self.shared.playback.submit(&inner.state) {
            Ok(runtime) => reconcile(&mut inner.state, runtime),
            Err(reason) => {
                inner.state.runtime.applied = crate::effects::AppliedState::Failed;
                inner.state.runtime.reason = Some(reason.clone());
                return Err(EffectsServiceError::Playback(reason));
            }
        }
        persist(&self.shared, &mut inner, PreferencesWrite::Repair)?;
        inner.protected = None;
        Ok(inner.state.clone())
    }
}

fn reconcile(state: &mut EffectsState, runtime: EffectsRuntime) {
    if runtime.revision == state.runtime.revision
        && runtime.playback_epoch >= state.runtime.playback_epoch
    {
        state.runtime.playback_epoch = runtime.playback_epoch;
        let _ = state.confirm(runtime);
    }
}

fn persist(
    shared: &Shared,
    inner: &mut Inner,
    mode: PreferencesWrite,
) -> Result<(), EffectsServiceError> {
    if mode == PreferencesWrite::Normal {
        if let Some(reason) = &inner.protected {
            inner.state.runtime.persistence_status = PersistenceStatus::Failed;
            return Err(EffectsServiceError::Protected(reason.clone()));
        }
        if inner.pending_since.is_none() {
            return Ok(());
        }
    }
    match shared.preferences.write(&inner.state.document, mode) {
        Ok(()) => {
            inner.pending_since = None;
            inner.state.runtime.persistence_status = PersistenceStatus::Saved;
            Ok(())
        }
        Err(reason) => {
            inner.state.runtime.persistence_status = PersistenceStatus::Failed;
            Err(EffectsServiceError::Storage(reason))
        }
    }
}

#[cfg(test)]
mod tests;
