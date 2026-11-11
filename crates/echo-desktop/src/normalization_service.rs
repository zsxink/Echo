//! The single owner of the normalization preference and its live facts.
//!
//! Preference lives in the desktop state store; the player actor owns the
//! computed gain. This service joins them for the typed IPC surface and is the
//! only place allowed to publish a preference to the actor.

use std::sync::{Arc, Mutex};

use crate::effects::loudness::NormalizationPreference;
use crate::ipc::normalization::NormalizationSnapshotDto;
use crate::platform::local_state::{DesktopStateStore, NormalizationStoreState};
use crate::player::port::PlayerPort;

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
pub enum NormalizationServiceError {
    #[error("invalid normalization preference")]
    InvalidPreference,
    #[error("normalization preference could not be saved")]
    Storage,
    #[error("normalization preference could not be applied")]
    Playback,
}

/// One service instance shared by every window (see `AGENT.md`: 唯一状态).
pub struct NormalizationService {
    store: Arc<DesktopStateStore>,
    player: Arc<dyn PlayerPort>,
    state: Mutex<NormalizationStoreState>,
}

impl NormalizationService {
    /// Restore the saved request and publish it to the actor. A storage read
    /// failure degrades to the safe default and is reported in the snapshot.
    #[must_use]
    pub fn restore(store: Arc<DesktopStateStore>, player: Arc<dyn PlayerPort>) -> Self {
        let state = store.normalization_state().unwrap_or_else(|error| {
            tracing::warn!(error = %error, "normalization preference unreadable");
            NormalizationStoreState {
                recovery_reason: Some("normalization preference could not be read".to_owned()),
                ..NormalizationStoreState::default()
            }
        });
        if let Err(error) = player.normalization_submit(state.preference) {
            tracing::warn!(error = %error, "normalization preference not accepted by actor");
        }
        Self {
            store,
            player,
            state: Mutex::new(state),
        }
    }

    #[must_use]
    pub fn snapshot(&self) -> NormalizationSnapshotDto {
        let state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        NormalizationSnapshotDto::from_store(&state, self.player.normalization_runtime())
    }

    /// Validate, persist, then publish. Acceptance is not a claim of applied
    /// gain: the snapshot's runtime facts come from the actor.
    ///
    /// # Errors
    /// `InvalidPreference` for illegal requests, `Storage` when the atomic
    /// write fails, `Playback` when the actor cannot accept the request.
    pub fn set(
        &self,
        preference: NormalizationPreference,
    ) -> Result<NormalizationSnapshotDto, NormalizationServiceError> {
        preference
            .validate()
            .map_err(|_| NormalizationServiceError::InvalidPreference)?;
        self.store
            .write_normalization(&preference)
            .map_err(|_| NormalizationServiceError::Storage)?;
        self.player
            .normalization_submit(preference)
            .map_err(|_| NormalizationServiceError::Playback)?;
        *self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = NormalizationStoreState {
            preference,
            recovery_reason: None,
        };
        Ok(self.snapshot())
    }
}

/// Stable label for the actor's last computed status, for UI copy only.
#[must_use]
pub const fn runtime_hint(status: crate::effects::loudness::NormalizationStatus) -> &'static str {
    use crate::effects::loudness::NormalizationStatus as Status;
    match status {
        Status::Disabled => "已关闭",
        Status::NoTags => "当前曲目无可用响度标签",
        Status::Applied => "按标签调整音量",
        Status::ClampedByPeak => "为避免削波已限制提升",
        Status::LimitedByCap => "无峰值信息，提升已限制",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::loudness::{NormalizationMode, NormalizationRuntime, NormalizationStatus};
    use crate::player::port::{NormalizationPort, PlayerCommand, PlayerError, PlayerSnapshot};
    use std::sync::mpsc;

    #[derive(Default)]
    struct RecordingPort {
        submitted: Mutex<Vec<NormalizationPreference>>,
        runtime: Mutex<NormalizationRuntime>,
        fail: bool,
    }

    impl PlayerPort for RecordingPort {
        fn send(&self, _cmd: PlayerCommand) -> Result<(), PlayerError> {
            Ok(())
        }
        fn snapshot(&self) -> PlayerSnapshot {
            PlayerSnapshot::default()
        }
        fn subscribe_snapshots(&self) -> mpsc::Receiver<PlayerSnapshot> {
            mpsc::sync_channel(1).1
        }
    }

    impl NormalizationPort for RecordingPort {
        fn normalization_submit(
            &self,
            preference: NormalizationPreference,
        ) -> Result<(), PlayerError> {
            if self.fail {
                return Err(PlayerError::ActorClosed);
            }
            self.submitted
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(preference);
            Ok(())
        }
        fn normalization_runtime(&self) -> NormalizationRuntime {
            self.runtime
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .clone()
        }
    }

    fn store() -> (tempfile::TempDir, Arc<DesktopStateStore>) {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("desktop-state.json");
        (
            dir,
            Arc::new(DesktopStateStore::new(
                path,
                crate::platform::local_state::PlatformCloseDefault::Other,
            )),
        )
    }

    #[test]
    fn restore_is_off_and_publishes_the_default_to_the_actor() {
        let (_dir, store) = store();
        let port = Arc::new(RecordingPort::default());
        let service = NormalizationService::restore(store, port.clone());
        let snapshot = service.snapshot();
        assert!(!snapshot.preference.enabled);
        assert_eq!(snapshot.runtime.status, NormalizationStatus::Disabled);
        assert_eq!(snapshot.recovery_reason, None);
        assert_eq!(
            port.submitted
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len(),
            1
        );
    }

    #[test]
    fn set_persists_and_publishes_before_reporting() {
        let (_dir, store) = store();
        let port = Arc::new(RecordingPort::default());
        let service = NormalizationService::restore(store.clone(), port.clone());
        let preference = NormalizationPreference {
            enabled: true,
            mode: NormalizationMode::Album,
            preamp_db: 2.0,
            ..NormalizationPreference::default()
        };
        let snapshot = service.set(preference).expect("set");
        assert_eq!(snapshot.preference, preference);
        assert_eq!(
            store.normalization_state().expect("read").preference,
            preference
        );
        assert_eq!(
            port.submitted
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .last(),
            Some(&preference)
        );
    }

    #[test]
    fn invalid_request_changes_nothing() {
        let (_dir, store) = store();
        let port = Arc::new(RecordingPort::default());
        let service = NormalizationService::restore(store.clone(), port.clone());
        assert_eq!(
            service.set(NormalizationPreference {
                preamp_db: 0.25,
                ..NormalizationPreference::default()
            }),
            Err(NormalizationServiceError::InvalidPreference)
        );
        assert_eq!(
            store.normalization_state().expect("read").preference,
            NormalizationPreference::default()
        );
        assert_eq!(
            port.submitted
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .len(),
            1,
            "only the restore request was published"
        );
    }

    #[test]
    fn actor_rejection_is_reported_not_silently_accepted() {
        let (_dir, store) = store();
        let port = Arc::new(RecordingPort {
            fail: true,
            ..RecordingPort::default()
        });
        let service = NormalizationService::restore(store, port);
        assert_eq!(
            service.set(NormalizationPreference::default()),
            Err(NormalizationServiceError::Playback)
        );
    }

    #[test]
    fn runtime_hints_never_claim_measurement_or_true_peak() {
        for status in [
            NormalizationStatus::Disabled,
            NormalizationStatus::NoTags,
            NormalizationStatus::Applied,
            NormalizationStatus::ClampedByPeak,
            NormalizationStatus::LimitedByCap,
        ] {
            let hint = runtime_hint(status);
            assert!(!hint.contains("true-peak"));
            assert!(!hint.contains("提升音质"));
        }
    }
}
