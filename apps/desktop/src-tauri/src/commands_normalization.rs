//! Typed normalization commands. The service owns persistence and actor
//! publication; command acceptance never fabricates an applied-gain fact.
#![allow(clippy::needless_pass_by_value)] // Tauri extracts owned command arguments across IPC.

use echo_desktop::effects::loudness::NormalizationPreference;
use echo_desktop::ipc::{normalization::NormalizationSnapshotDto, IpcErrorDto};
use echo_desktop::normalization_service::{NormalizationService, NormalizationServiceError};
use std::sync::Arc;
use tauri::State;

fn error(value: NormalizationServiceError) -> IpcErrorDto {
    match value {
        NormalizationServiceError::InvalidPreference => IpcErrorDto::new(
            "validation",
            "normalization.invalidPreference",
            false,
            None,
            Some("preampDb".to_owned()),
        ),
        NormalizationServiceError::Storage => {
            IpcErrorDto::new("io", "normalization.unsaved", true, None, None)
        }
        NormalizationServiceError::Playback => {
            IpcErrorDto::new("unavailable", "normalization.applyFailed", true, None, None)
        }
    }
}

#[tauri::command]
pub fn get_loudness_normalization(
    service: State<'_, Arc<NormalizationService>>,
) -> NormalizationSnapshotDto {
    service.snapshot()
}

#[tauri::command]
pub fn set_loudness_normalization(
    service: State<'_, Arc<NormalizationService>>,
    preference: NormalizationPreference,
) -> Result<NormalizationSnapshotDto, IpcErrorDto> {
    service.set(preference).map_err(error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_desktop::effects::loudness::{NormalizationMode, NormalizationRuntime};

    #[test]
    fn snapshot_dto_contract_is_camel_case_and_path_free() {
        let dto = NormalizationSnapshotDto::new(
            NormalizationPreference {
                enabled: true,
                mode: NormalizationMode::Album,
                preamp_db: 1.5,
                ..NormalizationPreference::default()
            },
            NormalizationRuntime::default(),
            None,
        );
        let json = serde_json::to_value(&dto).expect("serialize");
        assert_eq!(json["preference"]["mode"], "album");
        assert_eq!(json["preference"]["preampDb"], 1.5);
        assert_eq!(json["runtime"]["gainDb"], 0.0);
        assert!(!json.to_string().contains('/'));
    }

    #[test]
    fn preference_contract_rejects_unknown_fields() {
        let ok = serde_json::from_value::<NormalizationPreference>(serde_json::json!({
            "schemaVersion": 1,
            "enabled": false,
            "mode": "track",
            "preampDb": 0.0
        }));
        assert!(ok.is_ok());
        let bad = serde_json::from_value::<NormalizationPreference>(serde_json::json!({
            "schemaVersion": 1,
            "enabled": false,
            "mode": "track",
            "preampDb": 0.0,
            "futureField": true
        }));
        assert!(bad.is_err());
    }

    #[test]
    fn error_structure_is_stable() {
        let invalid = error(NormalizationServiceError::InvalidPreference);
        assert_eq!(invalid.code, "validation");
        assert_eq!(invalid.field.as_deref(), Some("preampDb"));
        assert!(!invalid.retryable);
        assert!(error(NormalizationServiceError::Storage).retryable);
        assert!(error(NormalizationServiceError::Playback).retryable);
    }
}
