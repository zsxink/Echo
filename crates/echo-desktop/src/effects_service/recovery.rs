//! Strict restoration keeps valid individual curves when the request is damaged.
use super::{EffectsDocument, EffectsState, PreferencesRecovery};

pub fn decode(raw: serde_json::Value) -> Result<EffectsDocument, String> {
    let document: EffectsDocument =
        serde_json::from_value(raw).map_err(|error| error.to_string())?;
    EffectsState::from_document(document.clone()).map_err(|error| error.to_string())?;
    Ok(document)
}

pub fn restore(loaded: Result<PreferencesRecovery, String>) -> (EffectsState, Option<String>) {
    match loaded {
        Ok(PreferencesRecovery::Missing) => (EffectsState::default(), None),
        Ok(PreferencesRecovery::Ready(document)) => match EffectsState::from_document(document) {
            Ok(state) => (state, None),
            Err(error) => failed(None, error.to_string()),
        },
        Ok(PreferencesRecovery::Protected { raw, reason }) => failed(raw.as_ref(), reason),
        Err(reason) => failed(None, reason),
    }
}

fn failed(raw: Option<&serde_json::Value>, reason: String) -> (EffectsState, Option<String>) {
    let mut state = EffectsState::default();
    if let Some(items) = raw
        .as_ref()
        .and_then(|raw| raw.get("userPresets"))
        .and_then(serde_json::Value::as_array)
    {
        for item in items {
            if let Ok(preset) = serde_json::from_value::<crate::effects::UserPreset>(item.clone()) {
                // Reuse all ID, curve, normalized-name and quota checks, without
                // trusting a corrupt selection or letting recovery enable it.
                let mut candidate = state.document.clone();
                candidate.user_presets.push(preset);
                if let Ok(valid) = EffectsState::from_document(candidate) {
                    state = valid;
                }
            }
        }
    }
    state.runtime.persistence_status = crate::effects::PersistenceStatus::Failed;
    state.runtime.reason = Some(reason.clone());
    (state, Some(reason))
}
