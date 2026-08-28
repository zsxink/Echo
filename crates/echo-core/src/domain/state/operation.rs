//! The per-resource import/delete/restore operation state machine.

use super::TransitionError;

/// Per-resource (audio / sidecar) import/delete/restore operation state.
///
/// The `Pending/Applied` pairs mirror the journal rule: an intent (`*Pending`)
/// is persisted *before* the side effect runs, and `*Applied` is committed
/// only after verification (file present, size/hash correct). Every step is
/// retry-safe; the machine rejects going back.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum OperationState {
    #[default]
    Planned,
    CopyPending,
    CopyApplied,
    ValidatePending,
    Validated,
    PublishPending,
    PublishApplied,
    DatabaseCommitted,
    Completed,
    /// Recoverable failure: keep the journal, close the root write path.
    FailedRecoverable,
    /// Fully rolled back (safe to clean).
    RolledBack,
    // Delete/move states (mirror of the tron-style design §9).
    StagePending,
    StageApplied,
    HiddenInDatabase,
    RestorePending,
    RestoreApplied,
    Restored,
    TrashPending,
    TrashApplied,
    DatabaseFinalized,
    TrashOutcomeUnknown,
}

impl OperationState {
    /// The legal per-resource state graph. Returns `true` only for a known
    /// `(from, to)` forward edge; everything else (including any edge out of a
    /// terminal state) is `false`.
    #[allow(clippy::unnested_or_patterns, clippy::enum_glob_use)]
    #[must_use]
    pub const fn can_transition_to(self, next: Self) -> bool {
        use OperationState::*;
        matches!(
            (self, next, self.is_terminal()),
            (Planned, CopyPending | StagePending, false)
                | (
                    CopyPending,
                    CopyApplied | FailedRecoverable | RolledBack,
                    false
                )
                | (
                    CopyApplied,
                    ValidatePending | FailedRecoverable | RolledBack,
                    false
                )
                | (
                    ValidatePending,
                    Validated | FailedRecoverable | RolledBack,
                    false
                )
                | (
                    Validated,
                    PublishPending | FailedRecoverable | RolledBack,
                    false
                )
                | (
                    PublishPending,
                    PublishApplied | FailedRecoverable | RolledBack,
                    false
                )
                | (PublishApplied, DatabaseCommitted | FailedRecoverable, false)
                | (DatabaseCommitted, Completed, false)
                // Recovery always re-runs the operation from its persisted
                // intent. A retry may select the appropriate initial branch
                // only after validating the journal item and filesystem facts.
                | (FailedRecoverable, CopyPending | StagePending, false)
                | (
                    StagePending,
                    StageApplied | FailedRecoverable | RolledBack,
                    false
                )
                | (StageApplied, HiddenInDatabase | FailedRecoverable, false)
                | (HiddenInDatabase, RestorePending | TrashPending, false)
                | (RestorePending, RestoreApplied | FailedRecoverable, false)
                | (RestoreApplied, Restored | FailedRecoverable, false)
                | (
                    TrashPending,
                    TrashApplied | TrashOutcomeUnknown | FailedRecoverable,
                    false
                )
                | (TrashApplied, DatabaseFinalized | FailedRecoverable, false)
                | (TrashOutcomeUnknown, TrashPending | FailedRecoverable, false)
        )
    }

    /// Move the operation to `next`.
    ///
    /// # Errors
    ///
    /// Returns a [`TransitionError`] when the move is not a legal edge.
    pub fn transition(self, next: Self) -> Result<Self, TransitionError> {
        if self == next {
            return Err(TransitionError::AlreadyIn(format!("{self:?}")));
        }
        if self.can_transition_to(next) {
            Ok(next)
        } else {
            // Distinguish a terminal-state block from an illegal edge.
            if self.is_terminal() {
                Err(TransitionError::Terminal {
                    state: format!("{self:?}"),
                    hint: "the operation is finished; a new operation is required",
                })
            } else {
                Err(TransitionError::Illegal {
                    from: format!("{self:?}"),
                    to: format!("{next:?}"),
                    hint: "operation steps are ordered and forward-only",
                })
            }
        }
    }

    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::RolledBack | Self::Restored | Self::DatabaseFinalized
        )
    }

    /// Whether the resource has been durably *published* (visible final file):
    /// true only in `PublishApplied` and later, and in the delete-finalized
    /// states that prove the file left the library.
    #[must_use]
    pub const fn publish_committed(self) -> bool {
        matches!(
            self,
            Self::PublishApplied
                | Self::DatabaseCommitted
                | Self::Completed
                | Self::TrashApplied
                | Self::DatabaseFinalized
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_operation_forward_and_publish_committed() {
        let mut o = OperationState::Planned;
        for next in [
            OperationState::CopyPending,
            OperationState::CopyApplied,
            OperationState::ValidatePending,
            OperationState::Validated,
            OperationState::PublishPending,
            OperationState::PublishApplied,
            OperationState::DatabaseCommitted,
            OperationState::Completed,
        ] {
            o = o.transition(next).unwrap();
        }
        assert!(o.is_terminal());
        assert!(o.transition(OperationState::Planned).is_err());
        assert!(o.transition(OperationState::CopyPending).is_err());
    }

    #[test]
    fn delete_operation_chain_ends_terminal_and_unknown() {
        let mut o = OperationState::Planned;
        o = o.transition(OperationState::StagePending).unwrap();
        o = o.transition(OperationState::StageApplied).unwrap();
        o = o.transition(OperationState::HiddenInDatabase).unwrap();
        o = o.transition(OperationState::TrashPending).unwrap();
        o = o.transition(OperationState::TrashApplied).unwrap();
        o = o.transition(OperationState::DatabaseFinalized).unwrap();
        assert!(o.is_terminal());
        assert!(o.publish_committed());
        // Unknown outcome: trash pending → outcome unknown keeps the journal
        // alive and can be retried (never inferred as success).
        let mut u = OperationState::TrashPending;
        u = u
            .transition(OperationState::TrashOutcomeUnknown)
            .expect("trash pending → unknown is legal");
        assert!(
            !u.publish_committed(),
            "unknown outcome is NOT publish-committed"
        );
        assert!(
            u.transition(OperationState::TrashPending).is_ok(),
            "unknown outcome can be retried as trash pending"
        );
        // …but the original stage chain is separate: `StagePending` cannot jump
        // straight to an outcome.
        assert!(OperationState::StagePending
            .transition(OperationState::TrashOutcomeUnknown)
            .is_err());
    }

    #[test]
    fn operation_rejects_backward_and_they_are_matchable() {
        let o = OperationState::CopyApplied;
        let err = o.transition(OperationState::CopyPending).unwrap_err();
        assert!(matches!(err, TransitionError::Illegal { .. }));
        assert_eq!(
            err,
            TransitionError::Illegal {
                from: "CopyApplied".into(),
                to: "CopyPending".into(),
                hint: "operation steps are ordered and forward-only",
            }
        );
    }

    #[test]
    fn recoverable_failure_can_restart_and_restored_is_terminal() {
        let retry = OperationState::CopyPending
            .transition(OperationState::FailedRecoverable)
            .unwrap();
        assert_eq!(
            retry.transition(OperationState::CopyPending).unwrap(),
            OperationState::CopyPending
        );
        assert!(OperationState::Restored.is_terminal());
        assert!(OperationState::Restored
            .transition(OperationState::StagePending)
            .is_err());
    }
}
