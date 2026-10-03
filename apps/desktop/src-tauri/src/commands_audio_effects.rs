//! Typed effects transport. Use cases own serialization of audio and storage;
//! command acceptance never fabricates an audio confirmation.
#![allow(clippy::needless_pass_by_value)] // Tauri extracts owned command arguments across IPC.

use echo_desktop::effects::{EffectsError, EffectsState, EqCurve, PresetId};
use echo_desktop::effects_service::{EffectsService, EffectsServiceError};
use echo_desktop::ipc::{effects::EffectsSnapshotDto, IpcErrorDto};
use std::sync::Arc;
use tauri::State;

fn error(value: EffectsServiceError) -> IpcErrorDto {
    let (code, key, retryable, field) = match value {
        EffectsServiceError::Rules(rule) => match rule {
            EffectsError::InvalidName => ("validation", "effects.invalidName", false, Some("name")),
            EffectsError::DuplicateName => {
                ("conflict", "effects.duplicateName", false, Some("name"))
            }
            EffectsError::QuotaExceeded => {
                ("validation", "effects.quotaExceeded", false, Some("name"))
            }
            EffectsError::Unavailable => ("unavailable", "effects.unavailable", true, None),
            _ => ("validation", "effects.invalidRequest", false, None),
        },
        EffectsServiceError::Storage(_) => ("io", "effects.unsaved", true, None),
        EffectsServiceError::Playback(_) => ("unavailable", "effects.applyFailed", true, None),
        EffectsServiceError::Protected(_) => ("storage", "effects.recoveryFailed", true, None),
        EffectsServiceError::NotDraft => ("validation", "effects.noDraft", false, None),
    };
    IpcErrorDto::new(code, key, retryable, None, field.map(str::to_owned))
}

fn preset_id(id: &str) -> Result<PresetId, IpcErrorDto> {
    PresetId::parse(id).map_err(|rule| error(EffectsServiceError::Rules(rule)))
}

fn project(
    value: Result<EffectsState, EffectsServiceError>,
) -> Result<EffectsSnapshotDto, IpcErrorDto> {
    value.map(|state| state.snapshot().into()).map_err(error)
}

#[tauri::command]
pub fn get_audio_effects_snapshot(service: State<'_, Arc<EffectsService>>) -> EffectsSnapshotDto {
    service.snapshot().snapshot().into()
}

#[tauri::command]
pub fn select_audio_effects_preset(
    service: State<'_, Arc<EffectsService>>,
    id: String,
) -> Result<EffectsSnapshotDto, IpcErrorDto> {
    project(service.select(&preset_id(&id)?))
}

#[tauri::command]
pub fn edit_audio_equalizer(
    service: State<'_, Arc<EffectsService>>,
    curve: EqCurve,
) -> Result<EffectsSnapshotDto, IpcErrorDto> {
    project(service.edit(curve))
}

#[tauri::command]
pub fn set_audio_effects_enabled(
    service: State<'_, Arc<EffectsService>>,
    enabled: bool,
) -> Result<EffectsSnapshotDto, IpcErrorDto> {
    project(service.set_enabled(enabled))
}

#[tauri::command]
pub fn reset_audio_effects(
    service: State<'_, Arc<EffectsService>>,
) -> Result<EffectsSnapshotDto, IpcErrorDto> {
    project(service.reset())
}

#[tauri::command]
pub fn save_audio_effects_preset(
    service: State<'_, Arc<EffectsService>>,
    name: String,
) -> Result<EffectsSnapshotDto, IpcErrorDto> {
    project(service.save(&name))
}

#[tauri::command]
pub fn rename_audio_effects_preset(
    service: State<'_, Arc<EffectsService>>,
    id: String,
    name: String,
) -> Result<EffectsSnapshotDto, IpcErrorDto> {
    project(service.rename(&preset_id(&id)?, &name))
}

#[tauri::command]
pub fn delete_audio_effects_preset(
    service: State<'_, Arc<EffectsService>>,
    id: String,
) -> Result<EffectsSnapshotDto, IpcErrorDto> {
    project(service.delete(&preset_id(&id)?))
}

#[tauri::command]
pub fn retry_audio_effects(
    service: State<'_, Arc<EffectsService>>,
) -> Result<EffectsSnapshotDto, IpcErrorDto> {
    project(service.retry())
}

pub fn flush(app: &tauri::AppHandle) {
    use tauri::Manager;
    if let Some(service) = app.try_state::<Arc<EffectsService>>() {
        if let Err(error) = service.flush() {
            tracing::error!(%error, "failed to flush audio effects preferences");
        }
    }
}
