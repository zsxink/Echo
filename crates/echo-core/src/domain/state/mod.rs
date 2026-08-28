//! Domain state machines (task 2.4).
//!
//! Echo models every long-lived or multi-step process as an explicit state
//! machine so that illegal transitions (going backward, skipping a phase) are
//! *rejected at the boundary* by a matchable [`TransitionError`] instead of
//! being left to caller discipline. The machines here are the domain's
//! authoritative transition rules; the application layer drives them.
//!
//! Submodules — one machine per file, plus a uniform property suite:
//!
//! - [`scan`] — [`ScanState`]: a generation scan: queued → enumerating →
//!   parsing → reconciling → completed/cancelled/failed.
//! - [`library_root`] — [`LibraryRootState`] and the root-switch cluster
//!   ([`RootSwitchState`], [`RootEpoch`], [`RootSwitchBarrier`]): activation
//!   through the two-phase candidate sequence, plus unavailability / relink.
//! - [`operation`] — [`OperationState`]: the per-resource import/delete/restore
//!   `Pending/Applied` sequence.
//! - [`playback`] — [`PlaybackState`]: player domain states (stopped → loading →
//!   playing/paused → ended/failed).
//! - [`property_tests`] — the shared invariants asserted over all machines.
//!
//! Common contract: every machine implements `transition(next) -> Result<(),
//! TransitionError>` and `can_transition_to(next) -> bool`, and exposes a
//! variant-based getter. Transitions are commutative-free: each current→next
//! edge is either allowed (documented) or rejected with a specific reason.

pub mod library_root;
pub mod operation;
pub mod playback;
#[cfg(test)]
pub mod property_tests;
pub mod scan;

pub use library_root::{LibraryRootState, RootEpoch, RootSwitchBarrier, RootSwitchState};
pub use operation::OperationState;
pub use playback::PlaybackState;
pub use scan::ScanState;

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
            why: value.to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transition_error_converts_to_core_error() {
        let err = OperationState::Completed
            .transition(OperationState::CopyPending)
            .unwrap_err();
        let core: Error = err.into();
        assert_eq!(core.code(), "invariant_violation");
    }
}
