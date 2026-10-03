//! Request state machine. Accepting a request never asserts native application;
//! only a matching revision and playback epoch can publish a confirmation.

use super::{
    math::{self, ResponsePoint},
    naming,
    presets::{self, Preset, PresetSource},
    AppliedState, EffectsDocument, EffectsError, EffectsRuntime, EqCurve, Payload,
    PersistenceStatus, PresetId, ProcessingEnvironment, Selection, UserPreset,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct EffectsState {
    pub document: EffectsDocument,
    pub runtime: EffectsRuntime,
    /// Service capture order, independent of the native audio request revision.
    pub snapshot_sequence: Option<u64>,
    /// Durable recovery protection survives unrelated native confirmations.
    pub recovery_reason: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectsSnapshot {
    pub document: EffectsDocument,
    pub runtime: EffectsRuntime,
    pub presets: Vec<Preset>,
    pub response_points: Vec<ResponsePoint>,
    pub reference_response: bool,
    pub safe_preamp_db: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery_reason: Option<String>,
}

impl EffectsState {
    /// Restore requests without restoring an Applied fact or starting playback.
    /// # Errors
    /// Returns document validation failures without modifying the original data.
    pub fn from_document(document: EffectsDocument) -> Result<Self, EffectsError> {
        document.validate()?;
        let mut state = Self {
            document,
            runtime: EffectsRuntime::default(),
            snapshot_sequence: None,
            recovery_reason: None,
        };
        if state.document.requested_enabled {
            state.runtime.applied = AppliedState::Pending;
            state.runtime.effective_preamp_db = None;
        }
        Ok(state)
    }

    /// # Errors
    /// Rejects missing/unavailable presets without discarding the current target.
    pub fn select(&mut self, id: &PresetId) -> Result<(), EffectsError> {
        let payload = self.document.preset_payload(id)?;
        if let Some(environment) = self.environment() {
            math::analyze(&payload, environment)?;
        }
        let revision = self.next_revision()?;
        self.document.selection = Selection::Preset(id.clone());
        self.document.draft = None;
        self.document.retained_payload = payload;
        self.document.requested_enabled = true;
        self.request_changed(revision);
        Ok(())
    }

    /// # Errors
    /// Rejects invalid whole requests and edits to disabled frequency bands.
    pub fn edit(&mut self, curve: EqCurve) -> Result<(), EffectsError> {
        curve.validate()?;
        if let Some(environment) = self.environment() {
            let previous = match &self.document.retained_payload {
                Payload::Eq(curve) => curve.clone(),
                Payload::Spatial(_) => EqCurve::default(),
            };
            for (index, frequency) in super::BAND_FREQUENCIES.iter().enumerate() {
                if *frequency > 0.45 * f64::from(environment.sample_rate)
                    && (previous.gains_db[index] - curve.gains_db[index]).abs() >= 0.5
                {
                    return Err(EffectsError::Unavailable);
                }
            }
        }
        let revision = self.next_revision()?;
        self.document.selection = Selection::Draft;
        self.document.retained_payload = Payload::Eq(curve.clone());
        self.document.draft = Some(curve);
        self.document.requested_enabled = true;
        self.request_changed(revision);
        Ok(())
    }

    /// # Errors
    /// Enabling requires a selected preset or independent EQ draft.
    pub fn set_enabled(&mut self, enabled: bool) -> Result<(), EffectsError> {
        if enabled && !self.document.can_enable() {
            return Err(EffectsError::NoTarget);
        }
        let revision = self.next_revision()?;
        if !enabled && matches!(self.document.selection, Selection::Preset(_)) {
            self.document.selection = Selection::None;
        }
        self.document.requested_enabled = enabled;
        self.request_changed(revision);
        Ok(())
    }

    /// # Errors
    /// Returns `SequenceOverflow` instead of accepting an unordered request.
    pub fn reset(&mut self) -> Result<(), EffectsError> {
        let revision = self.next_revision()?;
        self.document.selection = Selection::None;
        self.document.draft = None;
        self.document.retained_payload = Payload::default();
        self.document.requested_enabled = false;
        self.request_changed(revision);
        Ok(())
    }

    /// Commit an already-persisted new identity. Audio parameters do not change.
    /// # Errors
    /// Checks snapshot equality, namespace, names and quota before any mutation.
    pub fn saved(&mut self, mut preset: UserPreset) -> Result<(), EffectsError> {
        let draft = self.document.draft.as_ref().ok_or(EffectsError::NoDraft)?;
        preset.curve.validate()?;
        preset.id.validate()?;
        if !preset.id.is_user()
            || self
                .document
                .user_presets
                .iter()
                .any(|entry| entry.id == preset.id)
        {
            return Err(EffectsError::InvalidId);
        }
        if &preset.curve != draft {
            return Err(EffectsError::InvalidDocument);
        }
        naming::validate_capacity(&self.document.user_presets)?;
        preset.name = naming::validate_name(&preset.name, &self.document.user_presets, None)?;
        self.document.selection = Selection::Preset(preset.id.clone());
        self.document.draft = None;
        self.document.user_presets.push(preset);
        self.runtime.persistence_status = PersistenceStatus::Unsaved;
        Ok(())
    }

    /// Metadata changes must not rebuild a confirmed audio chain.
    /// # Errors
    /// Rejects missing/read-only entries and conflicting names.
    pub fn rename(&mut self, id: &PresetId, name: &str) -> Result<(), EffectsError> {
        let index = self
            .document
            .user_presets
            .iter()
            .position(|preset| &preset.id == id)
            .ok_or(EffectsError::NotFound)?;
        let name = naming::validate_name(name, &self.document.user_presets, Some(id))?;
        self.document.user_presets[index].name = name;
        self.runtime.persistence_status = PersistenceStatus::Unsaved;
        Ok(())
    }

    /// The application layer confirms bypass and durable deletion beforehand.
    /// # Errors
    /// Rejects missing/read-only identities and sequence exhaustion atomically.
    pub fn delete(&mut self, id: &PresetId) -> Result<(), EffectsError> {
        let index = self
            .document
            .user_presets
            .iter()
            .position(|preset| &preset.id == id)
            .ok_or(EffectsError::NotFound)?;
        let current = self.document.selection == Selection::Preset(id.clone());
        let revision = if current {
            Some(self.next_revision()?)
        } else {
            None
        };
        self.document.user_presets.remove(index);
        if let Some(revision) = revision {
            self.document.selection = Selection::None;
            self.document.requested_enabled = false;
            self.request_changed(revision);
        }
        self.runtime.persistence_status = PersistenceStatus::Unsaved;
        Ok(())
    }

    /// Invalidate old native facts when the playback chain or output changes.
    /// # Errors
    /// Rejects sequence exhaustion and invalid observed sample rates.
    pub fn environment_changed(
        &mut self,
        environment: Option<ProcessingEnvironment>,
    ) -> Result<(), EffectsError> {
        if environment.is_some_and(|value| value.sample_rate == 0) {
            return Err(EffectsError::Unavailable);
        }
        let epoch = self
            .runtime
            .playback_epoch
            .checked_add(1)
            .ok_or(EffectsError::SequenceOverflow)?;
        self.runtime.playback_epoch = epoch;
        self.runtime.processing_rate = environment.map(|value| value.sample_rate);
        self.runtime.channel_layout = environment.map(|value| value.channel_layout);
        self.runtime.applied = if self.document.requested_enabled {
            AppliedState::Pending
        } else {
            AppliedState::Bypassed
        };
        self.runtime.effective_preamp_db = if self.document.requested_enabled {
            None
        } else {
            Some(0.0)
        };
        self.runtime.active_bands = [false; 10];
        self.runtime.reason = None;
        if self.document.requested_enabled {
            if let Some(environment) = environment {
                match math::analyze(&self.document.requested_payload(), environment) {
                    Ok(analysis) => self.runtime.active_bands = analysis.active_bands,
                    Err(error) => {
                        self.runtime.applied = AppliedState::Unavailable;
                        self.runtime.reason = Some(error.to_string());
                    }
                }
            }
        }
        Ok(())
    }

    /// Publish only a fact for the latest request in the current playback chain.
    #[must_use]
    pub fn confirm(&mut self, mut receipt: EffectsRuntime) -> bool {
        if receipt.revision != self.runtime.revision
            || receipt.playback_epoch != self.runtime.playback_epoch
        {
            return false;
        }
        if receipt.applied == AppliedState::Applied
            && (!self.document.requested_enabled
                || receipt.processing_rate.map_or(true, |rate| rate == 0)
                || receipt.channel_layout.is_none()
                || receipt
                    .effective_preamp_db
                    .map_or(true, |value| !value.is_finite()))
        {
            return false;
        }
        receipt.persistence_status = self.runtime.persistence_status;
        self.runtime = receipt;
        true
    }

    #[must_use]
    pub fn snapshot(&self) -> EffectsSnapshot {
        let reference_response =
            self.runtime.processing_rate.is_none() || self.runtime.applied != AppliedState::Applied;
        let environment = ProcessingEnvironment {
            sample_rate: self.runtime.processing_rate.unwrap_or(48_000),
            channel_layout: self
                .runtime
                .channel_layout
                .unwrap_or(super::ChannelLayout::Stereo),
        };
        let payload = self.document.requested_payload();
        let mut analysis = math::analyze(&payload, environment).ok();
        // Pending/failed/bypassed graphs describe the retained request and are
        // explicitly reference curves. An Applied graph uses the confirmed
        // preamp fact, including any more conservative transition protection.
        if !reference_response {
            if let (Some(analysis), Some(preamp)) =
                (&mut analysis, self.runtime.effective_preamp_db)
            {
                analysis.effective_preamp_db = preamp;
            }
        }
        let mut presets = presets::builtin_presets();
        presets.extend(self.document.user_presets.iter().map(|user| Preset {
            id: user.id.clone(),
            name: user.name.clone(),
            description: "已保存的用户曲线".to_owned(),
            payload: Payload::Eq(user.curve.clone()),
            source: PresetSource::User,
        }));
        let response_points = if matches!(payload, Payload::Eq(_)) {
            analysis.as_ref().map_or_else(Vec::new, |analysis| {
                math::response_points(analysis, environment.sample_rate)
            })
        } else {
            Vec::new()
        };
        EffectsSnapshot {
            document: self.document.clone(),
            runtime: self.runtime.clone(),
            presets,
            response_points,
            reference_response,
            safe_preamp_db: analysis.map(|value| value.safe_preamp_db),
            snapshot_sequence: self.snapshot_sequence,
            recovery_reason: self.recovery_reason.clone(),
        }
    }

    fn environment(&self) -> Option<ProcessingEnvironment> {
        Some(ProcessingEnvironment {
            sample_rate: self.runtime.processing_rate?,
            channel_layout: self.runtime.channel_layout?,
        })
    }

    fn next_revision(&self) -> Result<u64, EffectsError> {
        self.runtime
            .revision
            .checked_add(1)
            .ok_or(EffectsError::SequenceOverflow)
    }

    fn request_changed(&mut self, revision: u64) {
        self.runtime.revision = revision;
        self.runtime.persistence_status = PersistenceStatus::Unsaved;
        self.runtime.applied = AppliedState::Pending;
        self.runtime.effective_preamp_db = None;
        self.runtime.reason = None;
    }
}

#[cfg(test)]
mod tests;
