//! Pure loudness-normalization rules.
//!
//! This module owns the only computation that turns embedded tags into an
//! audible gain. It holds no player, storage, time or UI state: the same
//! inputs always produce the same `NormalizationGain`. The player adapter
//! merely serializes the result into the existing `volume` filter node.

use echo_core::domain::media::{LoudnessSource, LoudnessTags};
use serde::{Deserialize, Serialize};

use super::math::EffectsAnalysis;

/// Unified reference used to align the two tagging schemes. It is a
/// convention for aligning tags, not a measurement of decoded audio and not
/// a claim of BS.1770 / EBU R128 compliance.
pub const TARGET_LUFS: f64 = -18.0;
/// `R128_*_GAIN` is relative to -23 LUFS; `ReplayGain` 2.0 to -18 LUFS.
pub const R128_ALIGNMENT_DB: f64 = TARGET_LUFS - (-23.0);
pub const PREAMP_MIN_DB: f64 = -12.0;
pub const PREAMP_MAX_DB: f64 = 12.0;
pub const MIN_GAIN_DB: f64 = -24.0;
pub const MAX_BOOST_DB: f64 = 6.0;
/// Limiter threshold is -1 dBFS; known peaks keep one extra dB of headroom.
pub const PEAK_HEADROOM_DB: f64 = 1.0;
pub const MIN_COMBINED_DB: f64 = -60.0;
pub const MAX_COMBINED_DB: f64 = 6.0;
/// A peak too small to be meaningful is treated as absent.
pub const MIN_PEAK_AMPLITUDE: f64 = 1.0e-6;

/// Which tag pair the user prefers when a file carries both.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NormalizationMode {
    #[default]
    Track,
    Album,
}

/// User preference for the independent normalization layer.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NormalizationPreference {
    pub schema_version: u32,
    pub enabled: bool,
    pub mode: NormalizationMode,
    pub preamp_db: f64,
}

pub const NORMALIZATION_SCHEMA_VERSION: u32 = 1;

impl Default for NormalizationPreference {
    fn default() -> Self {
        Self {
            schema_version: NORMALIZATION_SCHEMA_VERSION,
            enabled: false,
            mode: NormalizationMode::Track,
            preamp_db: 0.0,
        }
    }
}

impl NormalizationPreference {
    /// Reject illegal requests as a whole; never silently clamp user input.
    ///
    /// # Errors
    /// `InvalidPreference` for future schema versions, non-finite values,
    /// out-of-range preamps or values off the 0.5 dB step.
    pub fn validate(&self) -> Result<(), NormalizationError> {
        if self.schema_version != NORMALIZATION_SCHEMA_VERSION
            || !self.preamp_db.is_finite()
            || !(PREAMP_MIN_DB..=PREAMP_MAX_DB).contains(&self.preamp_db)
            || (self.preamp_db * 2.0).fract() != 0.0
        {
            return Err(NormalizationError::InvalidPreference);
        }
        Ok(())
    }
}

/// Why a track received its gain; surfaced by the UI instead of lying about
/// an applied target.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NormalizationStatus {
    /// The user switch is off.
    Disabled,
    /// No usable tag (missing, unsupported encoding or unusable value).
    NoTags,
    /// The computed gain was applied in full.
    Applied,
    /// The tag's sample peak forced a lower gain.
    ClampedByPeak,
    /// The no-peak boost cap or the overall gain bounds reduced the gain.
    LimitedByCap,
}

/// The pure result of one track's normalization computation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NormalizationGain {
    pub gain_db: f64,
    /// Final value for the shared preamp node: EQ protection + normalization.
    pub combined_preamp_db: f64,
    pub status: NormalizationStatus,
    pub source: Option<LoudnessSource>,
}

/// What the player actor last computed, exposed to the UI as facts only.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizationRuntime {
    pub gain_db: f64,
    pub status: NormalizationStatus,
    /// Diagnostic for a degraded track (lookup failure, unsupported tags…).
    pub reason: Option<String>,
}

impl Default for NormalizationRuntime {
    fn default() -> Self {
        Self {
            gain_db: 0.0,
            status: NormalizationStatus::Disabled,
            reason: None,
        }
    }
}

impl From<&NormalizationGain> for NormalizationRuntime {
    fn from(value: &NormalizationGain) -> Self {
        Self {
            gain_db: value.gain_db,
            status: value.status,
            reason: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum NormalizationError {
    #[error("invalid loudness normalization preference")]
    InvalidPreference,
}

/// Compute the gain for one track.
///
/// `analysis` is the *requested* effects analysis; when EQ/spatial is bypassed
/// the caller supplies the neutral EQ analysis so the P0 preamp stays intact.
///
/// # Errors
/// `InvalidPreference` when the preference fails validation.
pub fn compute(
    tags: Option<LoudnessTags>,
    preference: &NormalizationPreference,
    analysis: &EffectsAnalysis,
) -> Result<NormalizationGain, NormalizationError> {
    preference.validate()?;
    let neutral = NormalizationGain {
        gain_db: 0.0,
        combined_preamp_db: analysis.effective_preamp_db,
        status: NormalizationStatus::Disabled,
        source: None,
    };
    if !preference.enabled {
        return Ok(neutral);
    }
    let Some(tags) = tags.filter(|tags| tags.has_gain()) else {
        return Ok(NormalizationGain {
            status: NormalizationStatus::NoTags,
            ..neutral
        });
    };
    let (selected_gain_mdb, peak_ppm) = match preference.mode {
        NormalizationMode::Track => (
            tags.track_gain_mdb.or(tags.album_gain_mdb),
            tags.track_peak_ppm.or(tags.album_peak_ppm),
        ),
        NormalizationMode::Album => (
            tags.album_gain_mdb.or(tags.track_gain_mdb),
            tags.album_peak_ppm.or(tags.track_peak_ppm),
        ),
    };
    let Some(tag_gain_db) = LoudnessTags::gain_db(selected_gain_mdb) else {
        return Ok(NormalizationGain {
            status: NormalizationStatus::NoTags,
            ..neutral
        });
    };
    let alignment = match tags.source {
        LoudnessSource::ReplayGain2 => 0.0,
        LoudnessSource::R128 => R128_ALIGNMENT_DB,
    };
    let requested_db = tag_gain_db + alignment + preference.preamp_db;
    let peak =
        LoudnessTags::peak(peak_ppm).filter(|peak| peak.is_finite() && *peak > MIN_PEAK_AMPLITUDE);
    // A known peak gets its own clip-free ceiling; an unknown peak only gets
    // the global boost cap, with the limiter covering the residual risk.
    let peak_limit_db = peak.map_or(MAX_BOOST_DB, |peak| {
        (-20.0f64)
            .mul_add(peak.log10(), -PEAK_HEADROOM_DB)
            .min(MAX_BOOST_DB)
    });
    let applied_db = requested_db.clamp(MIN_GAIN_DB, peak_limit_db);
    let reduced = applied_db < requested_db;
    let combined_preamp_db =
        (analysis.effective_preamp_db + applied_db).clamp(MIN_COMBINED_DB, MAX_COMBINED_DB);
    let status = if reduced && peak.is_some() {
        NormalizationStatus::ClampedByPeak
    } else if reduced {
        NormalizationStatus::LimitedByCap
    } else {
        NormalizationStatus::Applied
    };
    Ok(NormalizationGain {
        gain_db: applied_db,
        combined_preamp_db,
        status,
        source: Some(tags.source),
    })
}

#[cfg(test)]
mod tests;
