//! User-visible recovery evidence. No database details or original payloads cross IPC.
use super::library_durability::{LibraryDurabilityOutcomeDto, LibraryDurabilityReasonDto};
use echo_core::domain::library::durability::{LibraryProjectionStatus, ProjectionUnresolvedReason};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RecoveryIssueReasonDto {
    MissingSong,
    MissingPlaylist,
    IdentityConflict,
    TombstonedDependency,
    MissingSyncDependency,
    AmbiguousMutationVersion,
    InvalidStateFact,
    UnverifiedConfirmationMetadata,
    RequiresStateRecovery,
}

impl From<ProjectionUnresolvedReason> for RecoveryIssueReasonDto {
    fn from(reason: ProjectionUnresolvedReason) -> Self {
        match reason {
            ProjectionUnresolvedReason::MissingSong => Self::MissingSong,
            ProjectionUnresolvedReason::MissingPlaylist => Self::MissingPlaylist,
            ProjectionUnresolvedReason::IdentityConflict => Self::IdentityConflict,
            ProjectionUnresolvedReason::TombstonedDependency => Self::TombstonedDependency,
            ProjectionUnresolvedReason::MissingSyncDependency => Self::MissingSyncDependency,
            ProjectionUnresolvedReason::AmbiguousMutationVersion => Self::AmbiguousMutationVersion,
            ProjectionUnresolvedReason::InvalidStateFact => Self::InvalidStateFact,
            ProjectionUnresolvedReason::UnverifiedConfirmationMetadata => {
                Self::UnverifiedConfirmationMetadata
            }
            ProjectionUnresolvedReason::RequiresStateRecovery => Self::RequiresStateRecovery,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryRecoveryIssueDto {
    /// Opaque stable typed identity for matching details across recovery attempts.
    pub target: String,
    pub reason: RecoveryIssueReasonDto,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryRecoveryStatusDto {
    pub root: String,
    pub read_only: bool,
    pub directory_head_seq: u64,
    pub projected_seq: u64,
    pub needs_projection: bool,
    /// No verified directory commit exists: historical completeness is not proven.
    pub legacy_unverified: bool,
    pub issues: Vec<LibraryRecoveryIssueDto>,
    pub outcome: LibraryDurabilityOutcomeDto,
}

impl LibraryRecoveryStatusDto {
    pub(crate) fn from_projection(
        root: String,
        read_only: bool,
        head: u64,
        status: LibraryProjectionStatus,
    ) -> Result<Self, echo_core::error::Error> {
        let has_unresolved = !status.unresolved.is_empty();
        let issues = status
            .unresolved
            .into_iter()
            .map(|issue| {
                Ok(LibraryRecoveryIssueDto {
                    target: serde_json::to_string(&issue.target).map_err(|source| {
                        echo_core::error::Error::unavailable_with_source(
                            "recovery status",
                            "cannot encode typed identity",
                            source,
                        )
                    })?,
                    reason: issue.reason.into(),
                })
            })
            .collect::<Result<_, echo_core::error::Error>>()?;
        let outcome = if head == 0 {
            LibraryDurabilityOutcomeDto::RecoveryPartial {
                reason: LibraryDurabilityReasonDto::LegacyUnverified,
            }
        } else if has_unresolved {
            LibraryDurabilityOutcomeDto::RecoveryPartial {
                reason: LibraryDurabilityReasonDto::UnresolvedProjection,
            }
        } else if head > status.applied_library_commit_seq {
            LibraryDurabilityOutcomeDto::SavedPendingProjection {
                operation_id: None,
                reason: LibraryDurabilityReasonDto::ProjectionPending,
            }
        } else {
            LibraryDurabilityOutcomeDto::Complete { operation_id: None }
        };
        Ok(Self {
            root,
            read_only,
            directory_head_seq: head,
            projected_seq: status.applied_library_commit_seq,
            needs_projection: head > status.applied_library_commit_seq,
            legacy_unverified: head == 0,
            issues,
            outcome,
        })
    }
}
