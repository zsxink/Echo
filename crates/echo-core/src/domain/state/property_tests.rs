//! Property tests (task 2.4), uniform over **all five** machines
//! ([`ScanState`], [`LibraryRootState`], [`RootSwitchState`],
//! [`OperationState`], [`PlaybackState`]): no state reaches itself, the
//! predicate and the transition function agree, and terminal states are
//! sinks — for every machine, not just the operation journal.

use super::*;
use proptest::prelude::*;

/// Uniform view over the machines so one property suite can assert the
/// same invariants for each without copy-paste.
trait Machine: Copy + Eq + std::fmt::Debug + 'static {
    /// Every state of the machine (index range for the proptests).
    const ALL: &'static [Self];
    fn can_go(self, next: Self) -> bool;
    fn advance(self, next: Self) -> Result<Self, TransitionError>;
    fn is_sink(self) -> bool;
}

impl Machine for ScanState {
    const ALL: &'static [Self] = &[
        Self::Queued,
        Self::Enumerating,
        Self::Parsing,
        Self::Reconciling,
        Self::Completed,
        Self::Cancelled,
        Self::Failed,
    ];
    fn can_go(self, next: Self) -> bool {
        self.can_transition_to(next)
    }
    fn advance(self, next: Self) -> Result<Self, TransitionError> {
        self.transition(next)
    }
    fn is_sink(self) -> bool {
        self.is_terminal()
    }
}

impl Machine for LibraryRootState {
    const ALL: &'static [Self] = &[
        Self::Unconfigured,
        Self::CandidateValidating,
        Self::CandidateScanning,
        Self::ActiveAvailable,
        Self::ActiveReadOnly,
        Self::Unavailable,
        Self::Relinking,
    ];
    fn can_go(self, next: Self) -> bool {
        self.can_transition_to(next)
    }
    fn advance(self, next: Self) -> Result<Self, TransitionError> {
        self.transition(next)
    }
    fn is_sink(self) -> bool {
        false
    }
}

impl Machine for RootSwitchState {
    const ALL: &'static [Self] = &[
        Self::Prepare,
        Self::QuiesceOldRoot,
        Self::CommitActivation,
        Self::RebindRuntime,
        Self::Completed,
        Self::Failed,
    ];
    fn can_go(self, next: Self) -> bool {
        self.can_transition_to(next)
    }
    fn advance(self, next: Self) -> Result<Self, TransitionError> {
        self.transition(next)
    }
    fn is_sink(self) -> bool {
        self.is_terminal()
    }
}

impl Machine for OperationState {
    const ALL: &'static [Self] = &[
        Self::Planned,
        Self::CopyPending,
        Self::CopyApplied,
        Self::ValidatePending,
        Self::Validated,
        Self::PublishPending,
        Self::PublishApplied,
        Self::DatabaseCommitted,
        Self::Completed,
        Self::FailedRecoverable,
        Self::RolledBack,
        Self::StagePending,
        Self::StageApplied,
        Self::HiddenInDatabase,
        Self::RestorePending,
        Self::RestoreApplied,
        Self::Restored,
        Self::TrashPending,
        Self::TrashApplied,
        Self::DatabaseFinalized,
        Self::TrashOutcomeUnknown,
    ];
    fn can_go(self, next: Self) -> bool {
        self.can_transition_to(next)
    }
    fn advance(self, next: Self) -> Result<Self, TransitionError> {
        self.transition(next)
    }
    fn is_sink(self) -> bool {
        self.is_terminal()
    }
}

impl Machine for PlaybackState {
    const ALL: &'static [Self] = &[
        Self::Stopped,
        Self::Loading,
        Self::Playing,
        Self::Paused,
        Self::Ended,
        Self::Failed,
    ];
    fn can_go(self, next: Self) -> bool {
        self.can_transition_to(next)
    }
    fn advance(self, next: Self) -> Result<Self, TransitionError> {
        self.transition(next)
    }
    fn is_sink(self) -> bool {
        false
    }
}

/// `OperationState` has the largest state space (21 states); smaller
/// machines are bounds-guarded so one index range drives every machine.
const MAX_STATES: usize = 21;

proptest! {
    // No state transitions to itself directly (identity is rejected).
    #[test]
    fn no_machine_admits_a_self_transition(state in 0usize..MAX_STATES) {
        fn check<M: Machine>(i: usize) -> bool {
            let all = <M as Machine>::ALL;
            i >= all.len() || !all[i].can_go(all[i])
        }
        prop_assert!(check::<ScanState>(state));
        prop_assert!(check::<LibraryRootState>(state));
        prop_assert!(check::<RootSwitchState>(state));
        prop_assert!(check::<OperationState>(state));
        prop_assert!(check::<PlaybackState>(state));
    }

    // Terminal states are sinks: no outgoing edge at all.
    #[test]
    fn terminal_states_have_no_outgoing_edges(state in 0usize..MAX_STATES) {
        fn check<M: Machine>(i: usize) -> bool {
            let all = <M as Machine>::ALL;
            if i >= all.len() || !all[i].is_sink() {
                return true;
            }
            let s = all[i];
            all.iter().all(|&t| !s.can_go(t))
        }
        prop_assert!(check::<ScanState>(state));
        prop_assert!(check::<LibraryRootState>(state));
        prop_assert!(check::<RootSwitchState>(state));
        prop_assert!(check::<OperationState>(state));
        prop_assert!(check::<PlaybackState>(state));
    }

    // `can_transition_to` and `transition` agree: the predicate is true iff
    // the transition function succeeds. This is the core consistency
    // property of the machine — a divergence is a real bug.
    #[test]
    fn predicate_matches_transition_result(from in 0usize..MAX_STATES, to in 0usize..MAX_STATES) {
        fn check<M: Machine>(f: usize, t: usize) -> bool {
            let all = <M as Machine>::ALL;
            if f >= all.len() || t >= all.len() {
                return true;
            }
            let (from, to) = (all[f], all[t]);
            from.can_go(to) == from.advance(to).is_ok()
        }
        prop_assert!(check::<ScanState>(from, to));
        prop_assert!(check::<LibraryRootState>(from, to));
        prop_assert!(check::<RootSwitchState>(from, to));
        prop_assert!(check::<OperationState>(from, to));
        prop_assert!(check::<PlaybackState>(from, to));
    }

    // Terminal states are sinks even when the (already-visited) same state
    // is offered as destination — no double-complete.
    #[test]
    fn terminal_transition_never_returns_ok(state in 0usize..MAX_STATES) {
        fn check<M: Machine>(i: usize) -> bool {
            let all = <M as Machine>::ALL;
            if i >= all.len() || !all[i].is_sink() {
                return true;
            }
            let s = all[i];
            s.advance(s).is_err()
        }
        prop_assert!(check::<ScanState>(state));
        prop_assert!(check::<LibraryRootState>(state));
        prop_assert!(check::<RootSwitchState>(state));
        prop_assert!(check::<OperationState>(state));
        prop_assert!(check::<PlaybackState>(state));
    }
}
