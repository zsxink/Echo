//! Path-free diagnostics for qualified listens still awaiting a directory commit.
//!
//! Playback statistics never interrupt playback: a listen that cannot be
//! committed stays in the app-private retry queue and the recorder retries it
//! on a backoff. What the user needs is visibility — how many listens are still
//! pending, which song each belongs to and why it is waiting — without any
//! absolute path or app-private byte crossing the IPC boundary.

use serde::{Deserialize, Serialize};

use crate::platform::playback_retry::{PlaybackRetryReason, PlaybackRetryStatus};

/// Stable reason a qualified listen has not reached the library directory yet.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlaybackRetryReasonDto {
    NotSaved,
    SavedPendingRecovery,
    SavedPendingProjection,
    LibraryUnavailable,
    InactiveLibrary,
    AuthorChanged,
    LibraryChanged,
    MissingSong,
    EnrollmentPending,
}

impl From<PlaybackRetryReason> for PlaybackRetryReasonDto {
    fn from(reason: PlaybackRetryReason) -> Self {
        match reason {
            PlaybackRetryReason::NotSaved => Self::NotSaved,
            PlaybackRetryReason::SavedPendingRecovery => Self::SavedPendingRecovery,
            PlaybackRetryReason::SavedPendingProjection => Self::SavedPendingProjection,
            PlaybackRetryReason::LibraryUnavailable => Self::LibraryUnavailable,
            PlaybackRetryReason::InactiveLibrary => Self::InactiveLibrary,
            PlaybackRetryReason::AuthorChanged => Self::AuthorChanged,
            PlaybackRetryReason::LibraryChanged => Self::LibraryChanged,
            PlaybackRetryReason::MissingSong => Self::MissingSong,
            PlaybackRetryReason::EnrollmentPending => Self::EnrollmentPending,
        }
    }
}

/// One pending qualified listen. `operation_id` is the original private
/// session: opening the details panel never submits a second event.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackRetryStatusDto {
    pub operation_id: String,
    pub song: String,
    pub qualified_at: u64,
    pub reason: PlaybackRetryReasonDto,
    pub attempts: u32,
}

impl From<&PlaybackRetryStatus> for PlaybackRetryStatusDto {
    fn from(status: &PlaybackRetryStatus) -> Self {
        Self {
            operation_id: status.operation_id.to_string(),
            song: status.song.to_string(),
            qualified_at: status.qualified_at,
            reason: status.reason.into(),
            attempts: status.attempts,
        }
    }
}

/// Deterministic TypeScript projection, appended by the centralized generator.
pub(super) const fn typescript() -> &'static str {
    r"export type PlaybackRetryReasonDto = 'notSaved' | 'savedPendingRecovery' | 'savedPendingProjection' | 'libraryUnavailable' | 'inactiveLibrary' | 'authorChanged' | 'libraryChanged' | 'missingSong' | 'enrollmentPending';

export interface PlaybackRetryStatusDto {
  readonly operationId: string;
  readonly song: string;
  readonly qualifiedAt: number;
  readonly reason: PlaybackRetryReasonDto;
  readonly attempts: number;
}
"
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every pending-listen reason survives as a stable camelCase tag, and the
    /// DTO carries the original session identity so the details panel can never
    /// look like a request to submit a second event.
    #[test]
    fn pending_listen_diagnostics_are_path_free_and_name_the_original_session() {
        use echo_core::domain::ids::{PlaybackSessionId, SongId};

        let status = PlaybackRetryStatus {
            operation_id: PlaybackSessionId::new(),
            song: SongId::new(),
            qualified_at: 1_700_000_000_000,
            reason: PlaybackRetryReason::SavedPendingProjection,
            attempts: 3,
        };
        let dto = PlaybackRetryStatusDto::from(&status);
        let value = serde_json::to_value(&dto).unwrap();
        assert_eq!(value["operationId"], status.operation_id.to_string());
        assert_eq!(value["song"], status.song.to_string());
        assert_eq!(value["qualifiedAt"], 1_700_000_000_000u64);
        assert_eq!(value["reason"], "savedPendingProjection");
        assert_eq!(value["attempts"], 3);
        // No path or app-private field may appear in the diagnostics payload.
        let encoded = serde_json::to_string(&dto).unwrap();
        assert!(!encoded.contains('/'));
        assert!(!encoded.contains("library"));
        assert!(!encoded.contains("author"));
        assert!(!encoded.contains("root"));
    }

    #[test]
    fn every_retry_reason_maps_to_a_camel_case_tag() {
        for (reason, expected) in [
            (PlaybackRetryReason::NotSaved, "notSaved"),
            (
                PlaybackRetryReason::SavedPendingRecovery,
                "savedPendingRecovery",
            ),
            (
                PlaybackRetryReason::SavedPendingProjection,
                "savedPendingProjection",
            ),
            (
                PlaybackRetryReason::LibraryUnavailable,
                "libraryUnavailable",
            ),
            (PlaybackRetryReason::InactiveLibrary, "inactiveLibrary"),
            (PlaybackRetryReason::AuthorChanged, "authorChanged"),
            (PlaybackRetryReason::LibraryChanged, "libraryChanged"),
            (PlaybackRetryReason::MissingSong, "missingSong"),
            (PlaybackRetryReason::EnrollmentPending, "enrollmentPending"),
        ] {
            assert_eq!(
                serde_json::to_value(PlaybackRetryReasonDto::from(reason)).unwrap(),
                serde_json::Value::String(expected.into())
            );
        }
    }
}
