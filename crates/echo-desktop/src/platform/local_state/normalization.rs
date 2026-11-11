//! Independent, versioned normalization preference slot.
//!
//! Kept out of the effects document on purpose: bumping `EffectsDocument`'s
//! schema would invalidate every saved EQ curve and draft. This slot has its
//! own validation and recovery, reusing the same atomic read-modify-write.

use super::{write_atomically, DesktopStateStore, StateError};
use crate::effects::loudness::NormalizationPreference;

/// The restored preference plus an honest recovery diagnostic. A damaged,
/// future-version or missing slot always yields the safe default (off).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct NormalizationStoreState {
    pub preference: NormalizationPreference,
    pub recovery_reason: Option<String>,
}

impl DesktopStateStore {
    /// Preserve the first damaged document before any write can overwrite it.
    fn preserve_normalization_original(&self, bytes: &[u8]) -> Result<(), StateError> {
        let backup = self.path.with_extension("json.normalization-recovery");
        if !backup.exists() {
            write_atomically(&backup, bytes)?;
        }
        Ok(())
    }

    /// Read the preference without ever failing startup on disk content.
    ///
    /// # Errors
    /// Propagates only real storage I/O failures; bad content degrades.
    pub fn normalization_state(&self) -> Result<NormalizationStoreState, StateError> {
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(bytes) = self.read_bytes()? else {
            return Ok(NormalizationStoreState::default());
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
            self.preserve_normalization_original(&bytes)?;
            return Ok(NormalizationStoreState {
                recovery_reason: Some("invalid desktop-state JSON".to_owned()),
                ..NormalizationStoreState::default()
            });
        };
        let Some(raw) = value.get("normalization") else {
            return Ok(NormalizationStoreState::default());
        };
        match serde_json::from_value::<NormalizationPreference>(raw.clone()) {
            Ok(preference) if preference.validate().is_ok() => Ok(NormalizationStoreState {
                preference,
                recovery_reason: None,
            }),
            _ => {
                self.preserve_normalization_original(&bytes)?;
                Ok(NormalizationStoreState {
                    recovery_reason: Some("invalid normalization preference".to_owned()),
                    ..NormalizationStoreState::default()
                })
            }
        }
    }

    /// Validate and atomically persist one preference, preserving every other
    /// field (including unknown future fields) and the effects document.
    ///
    /// # Errors
    /// `StateError` for invalid preferences, serialization or storage I/O.
    pub fn write_normalization(
        &self,
        preference: &NormalizationPreference,
    ) -> Result<(), StateError> {
        preference
            .validate()
            .map_err(|error| StateError::Serialize(error.to_string()))?;
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(bytes) = self.read_bytes()? {
            let damaged = serde_json::from_slice::<serde_json::Value>(&bytes)
                .ok()
                .and_then(|value| value.get("normalization").cloned())
                .is_some_and(|raw| {
                    serde_json::from_value::<NormalizationPreference>(raw)
                        .ok()
                        .is_none_or(|pref| pref.validate().is_err())
                });
            if damaged {
                self.preserve_normalization_original(&bytes)?;
            }
        }
        let mut raw = self.read_raw()?;
        raw.normalization = Some(
            serde_json::to_value(preference)
                .map_err(|error| StateError::Serialize(error.to_string()))?,
        );
        self.save(&raw)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::loudness::NormalizationMode;

    fn store() -> (tempfile::TempDir, DesktopStateStore) {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("desktop-state.json");
        (
            dir,
            DesktopStateStore::new(path, super::super::PlatformCloseDefault::Other),
        )
    }

    #[test]
    fn missing_slot_defaults_to_off() {
        let (_dir, store) = store();
        let state = store.normalization_state().expect("read");
        assert!(!state.preference.enabled);
        assert_eq!(state.preference.mode, NormalizationMode::Track);
        assert!(state.preference.preamp_db.abs() < f64::EPSILON);
        assert_eq!(state.recovery_reason, None);
    }

    #[test]
    fn write_and_read_round_trip_preserves_other_fields() {
        let (_dir, store) = store();
        store
            .set_theme(super::super::DesktopTheme::Cobalt)
            .expect("theme");
        let preference = NormalizationPreference {
            enabled: true,
            mode: NormalizationMode::Album,
            preamp_db: 3.0,
            ..NormalizationPreference::default()
        };
        store.write_normalization(&preference).expect("write");
        let state = store.normalization_state().expect("read");
        assert_eq!(state.preference, preference);
        assert_eq!(state.recovery_reason, None);
        assert_eq!(
            store.theme().expect("theme survives"),
            super::super::DesktopTheme::Cobalt
        );
    }

    #[test]
    fn damaged_and_future_documents_fall_back_without_overwriting() {
        for raw in [
            serde_json::json!({ "schemaVersion": 999, "enabled": true }),
            serde_json::json!({ "enabled": "yes" }),
            serde_json::json!({ "preampDb": 0.25 }),
            serde_json::json!({ "futureField": 1 }),
        ] {
            let (_dir, store) = store();
            let bytes = serde_json::to_vec(&serde_json::json!({
                "theme": "coral",
                "normalization": raw,
            }))
            .expect("json");
            std::fs::write(&store.path, &bytes).expect("seed file");
            let state = store.normalization_state().expect("read");
            assert!(!state.preference.enabled);
            assert!(state.recovery_reason.is_some());
            assert_eq!(std::fs::read(&store.path).expect("bytes"), bytes);
            assert!(store
                .path
                .with_extension("json.normalization-recovery")
                .exists());
            // A subsequent write preserves the original bytes in the backup.
            store
                .write_normalization(&NormalizationPreference::default())
                .expect("explicit write");
            assert_eq!(
                std::fs::read(store.path.with_extension("json.normalization-recovery"))
                    .expect("backup"),
                bytes
            );
        }
    }

    #[test]
    fn invalid_preference_is_rejected_before_any_write() {
        let (_dir, store) = store();
        assert!(store
            .write_normalization(&NormalizationPreference {
                preamp_db: 0.25,
                ..NormalizationPreference::default()
            })
            .is_err());
        assert!(!store.path.exists());
    }

    #[test]
    fn concurrent_writes_are_serialized() {
        let (_dir, store) = store();
        let shared = std::sync::Arc::new(store);
        let handles: Vec<_> = (0..8)
            .map(|index| {
                let store = std::sync::Arc::clone(&shared);
                std::thread::spawn(move || {
                    store
                        .write_normalization(&NormalizationPreference {
                            enabled: index % 2 == 0,
                            preamp_db: f64::from(index) / 2.0,
                            ..NormalizationPreference::default()
                        })
                        .expect("write");
                })
            })
            .collect();
        for handle in handles {
            handle.join().expect("writer");
        }
        let state = shared.normalization_state().expect("read");
        assert!(state.recovery_reason.is_none());
        assert!(state.preference.validate().is_ok());
    }
}
