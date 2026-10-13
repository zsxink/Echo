//! Additive audio-effects command/event projection. Runtime state remains local
//! to the desktop player; this boundary does not expose native filter strings.

use serde::Serialize;

use crate::effects::math::ResponsePoint;
use crate::effects::presets::Preset;
use crate::effects::{EffectsDocument, EffectsRuntime, EffectsSnapshot};

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EffectsSnapshotDto {
    pub document: EffectsDocument,
    pub runtime: EffectsRuntime,
    pub presets: Vec<Preset>,
    pub response_points: Vec<ResponsePoint>,
    pub reference_response: bool,
    pub safe_preamp_db: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub editable_bands: Option<[bool; 10]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub response_rate: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub snapshot_sequence: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recovery_reason: Option<String>,
}

impl From<EffectsSnapshot> for EffectsSnapshotDto {
    fn from(value: EffectsSnapshot) -> Self {
        Self {
            document: value.document,
            runtime: value.runtime,
            presets: value.presets,
            response_points: value.response_points,
            reference_response: value.reference_response,
            safe_preamp_db: value.safe_preamp_db,
            editable_bands: value.editable_bands,
            response_rate: value.response_rate,
            snapshot_sequence: value.snapshot_sequence,
            recovery_reason: value.recovery_reason,
        }
    }
}

/// Deterministic TypeScript projection, appended by the centralized generator.
pub(super) const fn typescript() -> &'static str {
    r"export type EffectsSelection = { readonly kind: 'none' } | { readonly kind: 'preset'; readonly id: string } | { readonly kind: 'draft' };
export interface EqCurve {
  readonly gainsDb: readonly number[];
  readonly preampMode: 'auto' | 'manual';
  readonly requestedPreampDb: number;
}

export type EffectsPayload = ({ readonly kind: 'eq' } & EqCurve) | { readonly kind: 'spatial'; readonly width: number; readonly mix: number };
export interface EffectsUserPreset { readonly id: string; readonly name: string; readonly curve: EqCurve; }
export interface EffectsDocument {
  readonly schemaVersion: number;
  readonly registryVersion: number;
  readonly userPresets: readonly EffectsUserPreset[];
  readonly selection: EffectsSelection;
  readonly retainedPayload: EffectsPayload;
  readonly draft: EqCurve | null;
  readonly requestedEnabled: boolean;
}
export interface EffectsRuntime {
  readonly revision: number;
  readonly playbackEpoch: number;
  readonly persistenceStatus: 'saved' | 'unsaved' | 'failed';
  readonly applied: 'bypassed' | 'pending' | 'applied' | 'failed' | 'unavailable';
  readonly effectivePreampDb: number | null;
  readonly activeBands: readonly boolean[];
  readonly processingRate: number | null;
  readonly channelLayout: 'mono' | 'stereo' | 'other' | null;
  readonly reason: string | null;
}
export interface EffectsPreset { readonly id: string; readonly name: string; readonly description: string; readonly payload: EffectsPayload; readonly source: 'builtin' | 'user'; }
export interface EffectsResponsePoint { readonly frequencyHz: number; readonly gainDb: number; }
export interface EffectsSnapshotDto {
  readonly document: EffectsDocument;
  readonly runtime: EffectsRuntime;
  readonly presets: readonly EffectsPreset[];
  readonly responsePoints: readonly EffectsResponsePoint[];
  readonly referenceResponse: boolean;
  readonly safePreampDb: number | null;
  readonly editableBands?: readonly boolean[];
  readonly responseRate?: number;
  readonly snapshotSequence?: number;
  readonly recoveryReason?: string;
}

"
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::{EffectsState, EqCurve, Payload, PresetId};

    #[test]
    fn audio_effects_projection_preserves_request_and_confirmation_separation() {
        let mut state = EffectsState::default();
        state.select(&PresetId::builtin("pop")).expect("select");
        let dto = EffectsSnapshotDto::from(state.snapshot());
        let value = serde_json::to_value(dto).expect("serialize");
        assert_eq!(value["document"]["selection"]["kind"], "preset");
        assert_eq!(value["document"]["selection"]["id"], "builtin:pop");
        assert_eq!(value["document"]["requestedEnabled"], true);
        assert_eq!(value["runtime"]["applied"], "pending");
        assert_eq!(
            value["runtime"]["effectivePreampDb"],
            serde_json::Value::Null
        );
        assert_eq!(value["presets"].as_array().expect("presets").len(), 10);
        assert_eq!(value["presets"][0]["source"], "builtin");
    }

    #[test]
    fn audio_effects_payload_boundary_rejects_unknown_type_and_invalid_numbers() {
        assert!(serde_json::from_str::<Payload>(r#"{"kind":"loudness"}"#).is_err());
        assert!(serde_json::from_str::<EqCurve>(
            r#"{"gainsDb":[0],"preampMode":"auto","requestedPreampDb":0}"#
        )
        .is_err());
        let curve = EqCurve {
            gains_db: [12.5; 10],
            ..EqCurve::default()
        };
        assert!(curve.validate().is_err());
    }

    #[test]
    fn runtime_projection_is_additive_and_not_persisted() {
        let state = EffectsState {
            snapshot_sequence: Some(42),
            recovery_reason: Some("protected document".into()),
            ..EffectsState::default()
        };
        let value = serde_json::to_value(EffectsSnapshotDto::from(state.snapshot())).unwrap();
        assert_eq!(value["snapshotSequence"], 42);
        assert_eq!(value["recoveryReason"], "protected document");
        assert!(value["document"].get("snapshotSequence").is_none());
        assert!(value["document"].get("recoveryReason").is_none());
        let legacy =
            serde_json::to_value(EffectsSnapshotDto::from(EffectsState::default().snapshot()))
                .unwrap();
        assert!(legacy.get("snapshotSequence").is_none());
        assert!(legacy.get("recoveryReason").is_none());
    }
}
