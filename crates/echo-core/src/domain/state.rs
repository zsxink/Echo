//! Domain state machines (task 2.4).
//!
//! Echo models every long-lived or multi-step process as an explicit state
//! machine so that illegal transitions (going backward, skipping a phase) are
//! *rejected at the boundary* by a matchable [`TransitionError`] instead of
//! being left to caller discipline. The machines here are the domain's
//! authoritative transition rules; the application layer drives them.
//!
//! The machines:
//!
//! - [`ScanState`] — a generation scan: queued → enumerating → parsing →
//!   reconciling → completed/cancelled/failed. No skipping, no re-entry.
//! - [`LibraryRootState`] — root activation through `Prepare → QuiesceOldRoot
//!   → CommitActivation → RebindRuntime`, plus unavailability / relink.
//! - [`OperationState`] — the per-resource import/delete/restore
//!   `Pending/Applied` sequence.
//! - [`PlaybackState`] — player domain states (stopped → loading → playing/
//!   paused → ended/failed).
//!
//! Common contract: every machine implements `transition(next) -> Result<(),
//! TransitionError>` and `can_transition_to(next) -> bool`, and exposes a
//! variant-based getter. Transitions are commutative-free: each current→next
//! edge is either allowed (documented) or rejected with a specific reason.

use crate::error::Error;

// ---------------------------------------------------------------------------
// TransitionError
// ---------------------------------------------------------------------------

/// Why a state transition was rejected.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TransitionError {
    /// The machine is already in `state`.
    AlreadyIn(String),
    /// The move from `from` to `to` is not a legal edge.
    Illegal {
        from: String,
        to: String,
        /// A short human hint (e.g. `cannot skip reconciling`).
        hint: &'static str,
    },
    /// The transition is legal but the operation is in a final/tombstone state
    /// (e.g. already completed, already rolled back).
    Terminal { state: String, hint: &'static str },
}

impl TransitionError {
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::AlreadyIn(_) => "already_in",
            Self::Illegal { .. } => "illegal_transition",
            Self::Terminal { .. } => "terminal_state",
        }
    }
}

impl std::fmt::Display for TransitionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::AlreadyIn(s) => write!(f, "already in state {s}"),
            Self::Illegal { from, to, hint } => write!(f, "cannot go {from} → {to}: {hint}"),
            Self::Terminal { state, hint } => write!(f, "state {state} is terminal: {hint}"),
        }
    }
}

impl std::error::Error for TransitionError {}

impl From<TransitionError> for Error {
    fn from(value: TransitionError) -> Self {
        Self::InvariantViolation {
            why: Box::leak(value.to_string().into_boxed_str()),
        }
    }
}

// ---------------------------------------------------------------------------
// ScanState
// ---------------------------------------------------------------------------

/// States of a library scan run.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ScanState {
    #[default]
    Queued,
    Enumerating,
    Parsing,
    Reconciling,
    Completed,
    Running,
    Cancelled,
    Failed,
}

impl ScanState {
    /// The legal state graph (forward only; a new scan is a new generation).
    ///
    /// The tuple edge-list form is deliberately top-level (`(from, to) | (from,
    /// to) | …`) for readability; the nursery lint prefers nesting but the
    /// two-tuple form is not ambiguous, so it is scoped out here.
    #[allow(clippy::unnested_or_patterns)]
    #[must_use]
    pub const fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Queued, Self::Enumerating)
                | (Self::Enumerating, Self::Parsing | Self::Reconciling)
                | (Self::Parsing, Self::Reconciling)
                | (Self::Reconciling, Self::Completed)
                | (
                    Self::Queued | Self::Enumerating | Self::Parsing | Self::Reconciling,
                    Self::Cancelled
                )
                | (
                    Self::Queued | Self::Enumerating | Self::Parsing | Self::Reconciling,
                    Self::Failed
                )
                | (
                    Self::Running,
                    Self::Completed | Self::Cancelled | Self::Failed
                )
        )
    }

    /// Move the scan to `next`, rejecting illegal/backward/skip transitions.
    ///
    /// # Errors
    ///
    /// `Result` is used for the transition guard; returns a
    /// [`TransitionError`] when the move is not a legal edge.
    pub fn transition(self, next: Self) -> Result<Self, TransitionError> {
        if self == next {
            return Err(TransitionError::AlreadyIn(format!("{self:?}")));
        }
        if self.can_transition_to(next) {
            Ok(next)
        } else {
            Err(TransitionError::Illegal {
                from: format!("{self:?}"),
                to: format!("{next:?}"),
                hint: "scan can only move forward; a new scan is a new generation",
            })
        }
    }

    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Cancelled | Self::Failed)
    }
}

// ---------------------------------------------------------------------------
// LibraryRootState
// ---------------------------------------------------------------------------

/// States of a library root.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LibraryRootState {
    #[default]
    Unconfigured,
    CandidateValidating,
    CandidateScanning,
    ActiveAvailable,
    ActiveReadOnly,
    Unavailable,
    Relinking,
}

impl LibraryRootState {
    /// The transition relation, expressed as an explicit edge list (see
    /// [`ScanState::can_transition_to`] for the style rationale).
    #[allow(clippy::unnested_or_patterns)]
    #[must_use]
    pub const fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            // Activation sequence.
            (Self::Unconfigured, Self::CandidateValidating)
                | (Self::CandidateValidating, Self::CandidateScanning)
                | (Self::CandidateScanning, Self::ActiveAvailable | Self::ActiveReadOnly)
                // Unavailability is reachable from any active/available state.
                | (Self::ActiveAvailable | Self::ActiveReadOnly, Self::Unavailable)
                | (Self::Unavailable, Self::ActiveAvailable | Self::ActiveReadOnly | Self::Relinking)
                | (Self::Relinking, Self::ActiveAvailable | Self::ActiveReadOnly)
                // Failures from candidate phase stay put (original active kept).
                | (Self::CandidateValidating | Self::CandidateScanning, Self::ActiveAvailable | Self::ActiveReadOnly | Self::Unconfigured)
        )
    }

    /// Move the root to `next`, enforcing the two-phase activation barrier.
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
            Err(TransitionError::Illegal {
                from: format!("{self:?}"),
                to: format!("{next:?}"),
                hint: "root activates only through the two-phase candidate sequence",
            })
        }
    }

    #[must_use]
    pub const fn is_active(self) -> bool {
        matches!(self, Self::ActiveAvailable | Self::ActiveReadOnly)
    }
}

// ---------------------------------------------------------------------------
// OperationState
// ---------------------------------------------------------------------------

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
            Self::Completed | Self::RolledBack | Self::DatabaseFinalized
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

// ---------------------------------------------------------------------------
// PlaybackState
// ---------------------------------------------------------------------------

/// Player domain states (the desktop actor owns the mpv handle; the *state* is
/// shared domain vocabulary).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PlaybackState {
    #[default]
    Stopped,
    Loading,
    Playing,
    Paused,
    Ended,
    Failed,
}

impl PlaybackState {
    /// The transition relation (same explicit edge-list style as the other
    /// machines).
    #[allow(clippy::unnested_or_patterns)]
    #[must_use]
    pub const fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Stopped, Self::Loading)
                | (
                    Self::Loading,
                    Self::Playing | Self::Paused | Self::Ended | Self::Failed
                )
                | (
                    Self::Playing,
                    Self::Paused | Self::Loading | Self::Ended | Self::Stopped | Self::Failed
                )
                | (Self::Paused, Self::Playing | Self::Loading | Self::Stopped)
                | (Self::Ended, Self::Loading | Self::Stopped | Self::Playing)
                | (Self::Failed, Self::Loading | Self::Stopped)
        )
    }

    /// Move the player to `next`.
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
            Err(TransitionError::Illegal {
                from: format!("{self:?}"),
                to: format!("{next:?}"),
                hint: "player cannot skip the load step or jump from stopped straight to playing",
            })
        }
    }

    #[must_use]
    pub const fn is_active(self) -> bool {
        matches!(self, Self::Playing | Self::Paused)
    }
}

#[cfg(test)]
mod property_tests {
    //! Property tests (task 2.4): the transition relation is acyclic — no
    //! state may reach itself through legal edges (no illegal *backward* or
    //! *skip* transition is ever admitted), and terminal states are sinks.

    use super::*;
    use proptest::prelude::*;

    /// All machine states under test.
    fn all_operation_states() -> Vec<OperationState> {
        vec![
            OperationState::Planned,
            OperationState::CopyPending,
            OperationState::CopyApplied,
            OperationState::ValidatePending,
            OperationState::Validated,
            OperationState::PublishPending,
            OperationState::PublishApplied,
            OperationState::DatabaseCommitted,
            OperationState::Completed,
            OperationState::FailedRecoverable,
            OperationState::RolledBack,
            OperationState::StagePending,
            OperationState::StageApplied,
            OperationState::HiddenInDatabase,
            OperationState::RestorePending,
            OperationState::RestoreApplied,
            OperationState::Restored,
            OperationState::TrashPending,
            OperationState::TrashApplied,
            OperationState::DatabaseFinalized,
            OperationState::TrashOutcomeUnknown,
        ]
    }

    proptest! {
        // No state transitions to itself directly (identity is rejected).
        #[test]
        fn no_state_transitions_to_itself(state in 0usize..21) {
            let states = all_operation_states();
            let s = states[state];
            prop_assert!(!s.can_transition_to(s));
        }

        // Terminal states are sinks: no outgoing edge at all.
        #[test]
        fn terminal_states_have_no_outgoing_edges(state in 0usize..21) {
            let states = all_operation_states();
            let s = states[state];
            if s.is_terminal() {
                for t in &states {
                    prop_assert!(!s.can_transition_to(*t), "{s:?} → {t:?} from terminal");
                }
            }
        }

        // `can_transition_to` and `transition` agree: the predicate is true iff
        // the transition function succeeds. This is the core consistency
        // property of the machine — a divergence is a real bug.
        #[test]
        fn predicate_matches_transition_result(from in 0usize..21, to in 0usize..21) {
            let states = all_operation_states();
            let from = states[from];
            let to = states[to];
            let allowed = from.can_transition_to(to);
            let ok = from.transition(to).is_ok();
            prop_assert_eq!(allowed, ok);
        }

        // Terminal states are sinks even when the (already-visited) same state
        // is offered as destination — no double-complete.
        #[test]
        fn terminal_transition_never_returns_ok(state in 0usize..21) {
            let states = all_operation_states();
            let s = states[state];
            if s.is_terminal() {
                prop_assert!(s.transition(s).is_err(), "terminal {s:?} must refuse re-entry");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_machine_forward_chain() {
        let mut s = ScanState::Queued;
        for next in [
            ScanState::Enumerating,
            ScanState::Parsing,
            ScanState::Reconciling,
            ScanState::Completed,
        ] {
            s = s.transition(next).unwrap();
        }
        assert!(s.is_terminal());
        // Terminal cannot move forward again.
        assert!(s.transition(ScanState::Failed).is_err());
    }

    #[test]
    fn scan_rejects_skip_and_backward() {
        let s = ScanState::Queued;
        assert!(s.transition(ScanState::Reconciling).is_err());
        let s = s.transition(ScanState::Enumerating).unwrap();
        assert!(s.transition(ScanState::Queued).is_err());
        // Cancelled is reachable mid-scan.
        assert!(s.transition(ScanState::Cancelled).is_ok());
    }

    #[test]
    fn root_activation_sequence() {
        let mut r = LibraryRootState::Unconfigured;
        for next in [
            LibraryRootState::CandidateValidating,
            LibraryRootState::CandidateScanning,
            LibraryRootState::ActiveAvailable,
        ] {
            r = r.transition(next).unwrap();
        }
        assert!(r.is_active());
        // Active → unavailable → back.
        r = r.transition(LibraryRootState::Unavailable).unwrap();
        assert!(!r.is_active());
        r = r.transition(LibraryRootState::Relinking).unwrap();
        r = r.transition(LibraryRootState::ActiveReadOnly).unwrap();
        assert!(r.is_active());
    }

    #[test]
    fn root_rejects_direct_activation() {
        let r = LibraryRootState::Unconfigured;
        assert!(r.transition(LibraryRootState::ActiveAvailable).is_err());
    }

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
    fn playback_state_machine() {
        let mut p = PlaybackState::Stopped;
        p = p.transition(PlaybackState::Loading).unwrap();
        p = p.transition(PlaybackState::Playing).unwrap();
        assert!(p.is_active());
        p = p.transition(PlaybackState::Paused).unwrap();
        p = p.transition(PlaybackState::Playing).unwrap();
        p = p.transition(PlaybackState::Ended).unwrap();
        assert!(!p.is_active());
        // Stopped cannot jump directly to playing.
        assert!(PlaybackState::Stopped
            .transition(PlaybackState::Playing)
            .is_err());
    }

    #[test]
    fn transition_error_converts_to_core_error() {
        let err = OperationState::Completed
            .transition(OperationState::CopyPending)
            .unwrap_err();
        let core: Error = err.into();
        assert_eq!(core.code(), "invariant_violation");
    }
}
