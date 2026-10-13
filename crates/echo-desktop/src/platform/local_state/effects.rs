//! Atomic effects slot and preservation of documents requiring explicit repair.

use super::{write_atomically, DesktopStateStore, StateError};
use crate::effects::EffectsDocument;
use crate::effects_service::{EffectsPreferencesPort, PreferencesRecovery, PreferencesWrite};

impl DesktopStateStore {
    /// Keep the first damaged/future document as bytes before any explicit repair.
    pub(super) fn preserve_original(&self, bytes: &[u8]) -> Result<(), StateError> {
        let backup = self.path.with_extension("json.effects-recovery");
        if backup.exists() {
            if std::fs::read(&backup)? != bytes {
                // A later damaged document must not overwrite the first one or
                // lose its own recoverable curves. Content identity also keeps
                // repeated recovery attempts from creating duplicate backups.
                let identity = uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_OID, bytes);
                let additional = self
                    .path
                    .with_extension(format!("json.effects-recovery.{identity}"));
                if !additional.exists() {
                    write_atomically(&additional, bytes)?;
                }
            }
        } else {
            write_atomically(&backup, bytes)?;
        }
        Ok(())
    }
}

impl EffectsPreferencesPort for DesktopStateStore {
    fn load(&self) -> Result<PreferencesRecovery, String> {
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let bytes = self.read_bytes().map_err(|error| error.to_string())?;
        let Some(bytes) = bytes else {
            return Ok(PreferencesRecovery::Missing);
        };
        let value = serde_json::from_slice::<serde_json::Value>(&bytes);
        let Ok(value) = value else {
            self.preserve_original(&bytes)
                .map_err(|error| error.to_string())?;
            return Ok(PreferencesRecovery::Protected {
                raw: None,
                reason: "invalid desktop-state JSON".into(),
            });
        };
        if !value.is_object() {
            self.preserve_original(&bytes)
                .map_err(|error| error.to_string())?;
            return Ok(PreferencesRecovery::Protected {
                raw: None,
                reason: "desktop-state is not an object".into(),
            });
        }
        let Some(raw) = value.get("effects") else {
            return Ok(PreferencesRecovery::Missing);
        };
        match crate::effects_service::recovery::decode(raw.clone()) {
            Ok(document) => Ok(PreferencesRecovery::Ready(document)),
            Err(reason) => {
                self.preserve_original(&bytes)
                    .map_err(|error| error.to_string())?;
                Ok(PreferencesRecovery::Protected {
                    raw: Some(raw.clone()),
                    reason,
                })
            }
        }
    }

    fn write(&self, document: &EffectsDocument, mode: PreferencesWrite) -> Result<(), String> {
        document.validate().map_err(|error| error.to_string())?;
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let bytes = self.read_bytes().map_err(|error| error.to_string())?;
        if let Some(bytes) = &bytes {
            let original = serde_json::from_slice::<serde_json::Value>(bytes);
            let protected = match &original {
                Ok(value) if value.is_object() => value.get("effects").is_some_and(|raw| {
                    crate::effects_service::recovery::decode(raw.clone()).is_err()
                }),
                _ => true,
            };
            if protected {
                self.preserve_original(bytes)
                    .map_err(|error| error.to_string())?;
                if mode != PreferencesWrite::Repair {
                    return Err("effects document requires explicit repair".into());
                }
            }
        }
        let mut raw = self.read_raw().map_err(|error| error.to_string())?;
        raw.effects = Some(serde_json::to_value(document).map_err(|error| error.to_string())?);
        self.save(&raw).map_err(|error| error.to_string())
    }
}
