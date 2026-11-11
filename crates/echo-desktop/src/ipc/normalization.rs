//! Typed projection of the independent loudness-normalization state.

use serde::Serialize;

use crate::effects::loudness::{NormalizationPreference, NormalizationRuntime};
use crate::platform::local_state::NormalizationStoreState;

/// Everything the effects panel needs: the saved request, the player's last
/// computed facts and an explicit recovery diagnostic. No filter strings,
/// absolute paths or database rows cross this boundary.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct NormalizationSnapshotDto {
    pub preference: NormalizationPreference,
    pub runtime: NormalizationRuntime,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery_reason: Option<String>,
}

impl NormalizationSnapshotDto {
    #[must_use]
    pub const fn new(
        preference: NormalizationPreference,
        runtime: NormalizationRuntime,
        recovery_reason: Option<String>,
    ) -> Self {
        Self {
            preference,
            runtime,
            recovery_reason,
        }
    }

    /// Build from the restored local preference plus the player's live facts.
    #[must_use]
    pub fn from_store(state: &NormalizationStoreState, runtime: NormalizationRuntime) -> Self {
        Self::new(state.preference, runtime, state.recovery_reason.clone())
    }
}

/// Deterministic TypeScript projection, appended by the centralized generator.
pub(super) const fn typescript() -> &'static str {
    r"export type NormalizationMode = 'track' | 'album';
export type NormalizationStatus = 'disabled' | 'noTags' | 'applied' | 'clampedByPeak' | 'limitedByCap';
export interface NormalizationPreference {
  readonly schemaVersion: number;
  readonly enabled: boolean;
  readonly mode: NormalizationMode;
  readonly preampDb: number;
}
export interface NormalizationRuntime {
  readonly gainDb: number;
  readonly status: NormalizationStatus;
  readonly reason: string | null;
}
export interface NormalizationSnapshotDto {
  readonly preference: NormalizationPreference;
  readonly runtime: NormalizationRuntime;
  readonly recoveryReason?: string | null;
}
"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::loudness::NormalizationMode;

    #[test]
    fn snapshot_serializes_camel_case_and_omits_absent_reason() {
        let preference = NormalizationPreference {
            enabled: true,
            mode: NormalizationMode::Album,
            preamp_db: 1.5,
            ..NormalizationPreference::default()
        };
        let dto = NormalizationSnapshotDto::new(preference, NormalizationRuntime::default(), None);
        let json = serde_json::to_value(&dto).expect("serialize");
        assert_eq!(json["preference"]["schemaVersion"], 1);
        assert_eq!(json["preference"]["enabled"], true);
        assert_eq!(json["preference"]["mode"], "album");
        assert_eq!(json["preference"]["preampDb"], 1.5);
        assert_eq!(json["runtime"]["status"], "disabled");
        assert!(json.get("recoveryReason").is_none());
    }
}
