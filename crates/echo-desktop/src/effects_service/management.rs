//! Durable preset management and compensation for deleting the active item.
use super::{reconcile, EffectsService, EffectsServiceError, PreferencesWrite};
use crate::effects::{
    AppliedState, EffectsState, PersistenceStatus, PresetId, Selection, UserPreset,
};
use std::time::Instant;

impl EffectsService {
    /// Saving a disabled draft retains its disabled request and never starts audio.
    /// # Errors
    /// Returns validation, protected-document, atomic write or safe bypass failures.
    pub fn save(&self, name: &str) -> Result<EffectsState, EffectsServiceError> {
        self.manage(|state| {
            let curve = state
                .document
                .draft
                .clone()
                .ok_or(EffectsServiceError::NotDraft)?;
            state.saved(UserPreset {
                id: PresetId::new_user(),
                name: name.to_owned(),
                curve,
            })?;
            Ok(())
        })
    }

    /// Metadata updates do not submit audio work or replace the native chain.
    /// # Errors
    /// Returns validation, protected-document, atomic write or safe bypass failures.
    pub fn rename(&self, id: &PresetId, name: &str) -> Result<EffectsState, EffectsServiceError> {
        self.manage(|state| {
            state.rename(id, name)?;
            Ok(())
        })
    }

    fn manage(
        &self,
        change: impl FnOnce(&mut EffectsState) -> Result<(), EffectsServiceError>,
    ) -> Result<EffectsState, EffectsServiceError> {
        let mut inner = self
            .shared
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(reason) = &inner.protected {
            return Err(EffectsServiceError::Protected(reason.clone()));
        }
        let mut next = inner.state.clone();
        change(&mut next)?;
        if let Err(reason) = self
            .shared
            .preferences
            .write(&next.document, PreferencesWrite::Normal)
        {
            inner.state.runtime.persistence_status = PersistenceStatus::Failed;
            return Err(EffectsServiceError::Storage(reason));
        }
        next.runtime.persistence_status = PersistenceStatus::Saved;
        inner.state = next;
        inner.pending_since = None;
        Ok(inner.state.clone())
    }

    /// Confirm bypass before deleting the current preset; failed commits compensate.
    /// # Errors
    /// Returns validation, protected-document, atomic write or safe bypass failures.
    pub fn delete(&self, id: &PresetId) -> Result<EffectsState, EffectsServiceError> {
        let mut inner = self
            .shared
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(reason) = &inner.protected {
            return Err(EffectsServiceError::Protected(reason.clone()));
        }
        let old = inner.state.clone();
        let current = old.document.selection == Selection::Preset(id.clone());
        let mut next = old;
        next.delete(id)?;
        if current {
            match self.shared.playback.bypass(&next) {
                Ok(runtime) if runtime.applied == AppliedState::Bypassed => {
                    reconcile(&mut next, runtime);
                }
                Ok(runtime) => {
                    inner.state.runtime.revision = next.runtime.revision;
                    reconcile(&mut inner.state, runtime);
                    inner.state.runtime.applied = AppliedState::Failed;
                    inner.state.runtime.reason = Some("bypass was not confirmed".into());
                    return Err(EffectsServiceError::Playback(
                        "bypass was not confirmed".into(),
                    ));
                }
                Err(reason) => {
                    inner.state.runtime.revision = next.runtime.revision;
                    inner.state.runtime.applied = AppliedState::Failed;
                    inner.state.runtime.reason = Some(reason.clone());
                    return Err(EffectsServiceError::Playback(reason));
                }
            }
        }
        if let Err(reason) = self
            .shared
            .preferences
            .write(&next.document, PreferencesWrite::Normal)
        {
            if current {
                // Compensation uses a fresh revision so the actor cannot discard
                // restoration as an outdated request. The durable list stays old.
                inner.state.runtime.revision = next.runtime.revision.saturating_add(1);
                match self.shared.playback.submit(&inner.state) {
                    Ok(runtime) => reconcile(&mut inner.state, runtime),
                    Err(restore_reason) => {
                        if let Ok(runtime) = self.shared.playback.bypass(&inner.state) {
                            reconcile(&mut inner.state, runtime);
                        }
                        inner.state.runtime.applied = AppliedState::Failed;
                        inner.state.runtime.reason = Some(restore_reason);
                    }
                }
            }
            inner.state.runtime.persistence_status = PersistenceStatus::Failed;
            inner.pending_since = Some(Instant::now());
            return Err(EffectsServiceError::Storage(reason));
        }
        next.runtime.persistence_status = PersistenceStatus::Saved;
        inner.state = next;
        inner.pending_since = None;
        Ok(inner.state.clone())
    }
}
