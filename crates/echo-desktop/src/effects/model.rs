//! Typed payloads, native processing facts and request/application status.

use serde::{Deserialize, Serialize};
use std::fmt;

pub const BAND_FREQUENCIES: [f64; 10] = [
    31.25, 62.5, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0,
];
pub const SCHEMA_VERSION: u32 = 1;
pub const REGISTRY_VERSION: u32 = 1;
pub const MAX_USER_PRESETS: usize = 50;

/// Stable namespaces prevent a user curve from replacing a built-in entry.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct PresetId(String);

impl PresetId {
    #[must_use]
    pub fn new_user() -> Self {
        Self(format!("user:{}", uuid::Uuid::new_v4()))
    }

    #[must_use]
    pub fn builtin(id: &str) -> Self {
        Self(format!("builtin:{id}"))
    }

    /// Parse only registered built-ins or canonical user UUIDs.
    /// # Errors
    /// Returns `InvalidId` for unknown namespaces or identifiers.
    pub fn parse(value: &str) -> Result<Self, EffectsError> {
        let id = Self(value.to_owned());
        id.validate()?;
        Ok(id)
    }

    /// # Errors
    /// Returns `InvalidId` for unknown namespaces or identifiers.
    pub fn validate(&self) -> Result<(), EffectsError> {
        if let Some(value) = self.0.strip_prefix("user:") {
            if uuid::Uuid::parse_str(value).is_ok_and(|id| id.hyphenated().to_string() == value) {
                return Ok(());
            }
        } else if let Some(value) = self.0.strip_prefix("builtin:") {
            if super::presets::BUILTIN_IDS.contains(&value) {
                return Ok(());
            }
        }
        Err(EffectsError::InvalidId)
    }

    #[must_use]
    pub fn is_user(&self) -> bool {
        self.0.starts_with("user:")
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PresetId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PreampMode {
    #[default]
    Auto,
    Manual,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EqCurve {
    pub gains_db: [f64; 10],
    pub preamp_mode: PreampMode,
    pub requested_preamp_db: f64,
}

impl EqCurve {
    /// Validate the entire request; never silently clamp or discard a band.
    /// # Errors
    /// Returns `InvalidParameter` for non-finite, out-of-range or off-step values.
    pub fn validate(&self) -> Result<(), EffectsError> {
        for gain in self.gains_db {
            validate_step(gain, -12.0, 12.0)?;
        }
        validate_step(self.requested_preamp_db, -12.0, 0.0)
    }
}

fn validate_step(value: f64, minimum: f64, maximum: f64) -> Result<(), EffectsError> {
    if !value.is_finite() || !(minimum..=maximum).contains(&value) || (value * 2.0).fract() != 0.0 {
        return Err(EffectsError::InvalidParameter);
    }
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Spatial {
    pub width: f64,
    pub mix: f64,
}

impl Default for Spatial {
    fn default() -> Self {
        Self {
            width: 1.25,
            mix: 1.0,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Payload {
    Eq(EqCurve),
    Spatial(Spatial),
}

impl Default for Payload {
    fn default() -> Self {
        Self::Eq(EqCurve::default())
    }
}

impl Payload {
    /// # Errors
    /// Rejects unsupported spatial parameters and invalid EQ values.
    pub fn validate(&self) -> Result<(), EffectsError> {
        match self {
            Self::Eq(curve) => curve.validate(),
            Self::Spatial(spatial) if *spatial == Spatial::default() => Ok(()),
            Self::Spatial(_) => Err(EffectsError::InvalidParameter),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChannelLayout {
    Mono,
    Stereo,
    Other,
}

/// Observed filter input facts, never derived from source metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessingEnvironment {
    pub sample_rate: u32,
    pub channel_layout: ChannelLayout,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AppliedState {
    #[default]
    Bypassed,
    Pending,
    Applied,
    Failed,
    Unavailable,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PersistenceStatus {
    #[default]
    Saved,
    Unsaved,
    Failed,
}

/// Runtime confirmation is deliberately absent from the persisted document.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectsRuntime {
    pub revision: u64,
    pub playback_epoch: u64,
    pub persistence_status: PersistenceStatus,
    pub applied: AppliedState,
    pub effective_preamp_db: Option<f64>,
    pub active_bands: [bool; 10],
    pub processing_rate: Option<u32>,
    pub channel_layout: Option<ChannelLayout>,
    pub reason: Option<String>,
}

impl Default for EffectsRuntime {
    fn default() -> Self {
        Self {
            revision: 0,
            playback_epoch: 0,
            persistence_status: PersistenceStatus::Saved,
            applied: AppliedState::Bypassed,
            effective_preamp_db: Some(0.0),
            active_bands: [false; 10],
            processing_rate: None,
            channel_layout: None,
            reason: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum EffectsError {
    #[error("invalid effect parameter, range or 0.5 dB step")]
    InvalidParameter,
    #[error("invalid effect preset identifier")]
    InvalidId,
    #[error("effect preset does not exist")]
    NotFound,
    #[error("effect name must contain 1–40 grapheme clusters")]
    InvalidName,
    #[error("effect name already exists")]
    DuplicateName,
    #[error("at most 50 user curves can be saved")]
    QuotaExceeded,
    #[error("no EQ draft is available")]
    NoDraft,
    #[error("there is no selection or draft to enable")]
    NoTarget,
    #[error("effect is unavailable for the observed processing environment")]
    Unavailable,
    #[error("unsupported effects document version")]
    UnsupportedVersion,
    #[error("inconsistent effects document")]
    InvalidDocument,
    #[error("effect revision or playback epoch overflow")]
    SequenceOverflow,
}
