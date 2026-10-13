//! Durable request data. Validation keeps future or corrupt documents from
//! being interpreted as permission to enable an effect.

use super::{
    naming, presets, EffectsError, EqCurve, Payload, PresetId, MAX_USER_PRESETS, REGISTRY_VERSION,
    SCHEMA_VERSION,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "camelCase")]
pub enum Selection {
    #[default]
    None,
    Preset(PresetId),
    Draft,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct UserPreset {
    pub id: PresetId,
    pub name: String,
    pub curve: EqCurve,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectsDocument {
    pub schema_version: u32,
    pub registry_version: u32,
    pub user_presets: Vec<UserPreset>,
    pub selection: Selection,
    pub retained_payload: Payload,
    pub draft: Option<EqCurve>,
    pub requested_enabled: bool,
}

impl Default for EffectsDocument {
    fn default() -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            registry_version: REGISTRY_VERSION,
            user_presets: Vec::new(),
            selection: Selection::None,
            retained_payload: Payload::default(),
            draft: None,
            requested_enabled: false,
        }
    }
}

impl EffectsDocument {
    /// # Errors
    /// Rejects future versions, invalid curves, identities, names and references.
    pub fn validate(&self) -> Result<(), EffectsError> {
        if self.schema_version != SCHEMA_VERSION || self.registry_version != REGISTRY_VERSION {
            return Err(EffectsError::UnsupportedVersion);
        }
        if self.user_presets.len() > MAX_USER_PRESETS {
            return Err(EffectsError::QuotaExceeded);
        }
        self.retained_payload.validate()?;
        let mut ids = HashSet::new();
        for preset in &self.user_presets {
            preset.id.validate()?;
            if !preset.id.is_user() || !ids.insert(&preset.id) {
                return Err(EffectsError::InvalidId);
            }
            preset.curve.validate()?;
            let normalized =
                naming::validate_name(&preset.name, &self.user_presets, Some(&preset.id))?;
            if normalized != preset.name {
                return Err(EffectsError::InvalidName);
            }
        }
        match (&self.selection, &self.draft) {
            (Selection::Draft, Some(curve)) => {
                curve.validate()?;
                if self.retained_payload != Payload::Eq(curve.clone()) {
                    return Err(EffectsError::InvalidDocument);
                }
            }
            (Selection::Preset(id), None) => {
                if self.preset_payload(id)? != self.retained_payload {
                    return Err(EffectsError::InvalidDocument);
                }
            }
            (Selection::None, None) if !self.requested_enabled => {}
            _ => return Err(EffectsError::InvalidDocument),
        }
        Ok(())
    }

    /// # Errors
    /// Returns `NotFound` for a dangling selected preset.
    pub fn preset_payload(&self, id: &PresetId) -> Result<Payload, EffectsError> {
        presets::builtin(id)
            .map(|preset| preset.payload)
            .or_else(|| {
                self.user_presets
                    .iter()
                    .find(|preset| &preset.id == id)
                    .map(|preset| Payload::Eq(preset.curve.clone()))
            })
            .ok_or(EffectsError::NotFound)
    }

    #[must_use]
    pub const fn can_enable(&self) -> bool {
        !matches!(self.selection, Selection::None)
    }

    #[must_use]
    pub fn requested_payload(&self) -> Payload {
        self.draft.as_ref().map_or_else(
            || self.retained_payload.clone(),
            |curve| Payload::Eq(curve.clone()),
        )
    }
}
