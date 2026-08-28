//! Library-root state and the root-switch activation cluster.

use crate::domain::ids::LibraryRootId;

use super::TransitionError;

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

/// The runtime barrier for switching the active library root.
///
/// This is deliberately separate from [`LibraryRootState`]: a root record can
/// remain active while a *switch operation* validates a candidate. The runtime
/// uses the phase plus an epoch to reject work produced by the old root after a
/// successful activation commit.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RootSwitchState {
    /// The candidate has been validated and fully scanned; the old root is
    /// still serving runtime work.
    #[default]
    Prepare,
    /// Freeze the old watcher and wait for all write blockers to settle.
    QuiesceOldRoot,
    /// Atomically flip the active root and advance the root epoch.
    CommitActivation,
    /// Bind watcher, query invalidation and playback to the new epoch.
    RebindRuntime,
    /// The new root is fully active.
    Completed,
    /// The switch failed before completion; the old runtime stays/re-enters
    /// service and the candidate may be retried from `Prepare`.
    Failed,
}

impl RootSwitchState {
    #[must_use]
    pub const fn can_transition_to(self, next: Self) -> bool {
        matches!(
            (self, next),
            (Self::Prepare, Self::QuiesceOldRoot | Self::Failed)
                | (Self::QuiesceOldRoot, Self::CommitActivation | Self::Failed)
                | (Self::CommitActivation, Self::RebindRuntime | Self::Failed)
                | (Self::RebindRuntime, Self::Completed | Self::Failed)
                | (Self::Failed, Self::Prepare)
        )
    }

    /// Advance this root-switch barrier by one legal phase.
    ///
    /// # Errors
    ///
    /// Returns a matchable [`TransitionError`] for a skipped, repeated or
    /// terminal transition.
    pub fn transition(self, next: Self) -> Result<Self, TransitionError> {
        if self == next {
            return Err(TransitionError::AlreadyIn(format!("{self:?}")));
        }
        if self.can_transition_to(next) {
            Ok(next)
        } else if self.is_terminal() {
            Err(TransitionError::Terminal {
                state: format!("{self:?}"),
                hint: "the root switch is complete; start a new switch",
            })
        } else {
            Err(TransitionError::Illegal {
                from: format!("{self:?}"),
                to: format!("{next:?}"),
                hint: "root switching must quiesce, commit, then rebind in order",
            })
        }
    }

    #[must_use]
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Completed)
    }
}

/// Monotonic generation of the active-root runtime binding. Watcher, scan and
/// command results must carry the epoch they started under; a result with an
/// older epoch is discarded after a root switch commits.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub struct RootEpoch(u64);

impl RootEpoch {
    #[must_use]
    pub const fn from_u64(value: u64) -> Self {
        Self(value)
    }

    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

/// Runtime context for one root switch. It preserves the old and candidate IDs
/// until the `SQLite` activation commit succeeds, and then makes the committed
/// epoch the sole authority for accepting asynchronous results.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RootSwitchBarrier {
    old_root: Option<LibraryRootId>,
    candidate_root: LibraryRootId,
    state: RootSwitchState,
    active_epoch: RootEpoch,
}

impl RootSwitchBarrier {
    #[must_use]
    pub const fn new(
        old_root: Option<LibraryRootId>,
        candidate_root: LibraryRootId,
        active_epoch: RootEpoch,
    ) -> Self {
        Self {
            old_root,
            candidate_root,
            state: RootSwitchState::Prepare,
            active_epoch,
        }
    }

    #[must_use]
    pub const fn state(&self) -> RootSwitchState {
        self.state
    }

    #[must_use]
    pub const fn old_root(&self) -> Option<LibraryRootId> {
        self.old_root
    }

    #[must_use]
    pub const fn candidate_root(&self) -> LibraryRootId {
        self.candidate_root
    }

    #[must_use]
    pub const fn active_epoch(&self) -> RootEpoch {
        self.active_epoch
    }

    /// Advance a non-commit phase. The `SQLite` commit must use
    /// [`Self::commit_activation`] so an epoch cannot be advanced by accident.
    ///
    /// # Errors
    ///
    /// Returns a matchable transition error for an illegal or skipped phase.
    pub fn transition(&mut self, next: RootSwitchState) -> Result<(), TransitionError> {
        if self.state == RootSwitchState::CommitActivation && next == RootSwitchState::RebindRuntime
        {
            return Err(TransitionError::Illegal {
                from: format!("{:?}", self.state),
                to: format!("{next:?}"),
                hint: "commit activation must atomically advance root_epoch",
            });
        }
        self.state = self.state.transition(next)?;
        Ok(())
    }

    /// Record the successful `SQLite` activation transaction and advance the
    /// runtime epoch before watcher/player rebind begins.
    ///
    /// # Errors
    ///
    /// Fails unless the barrier is at `CommitActivation` and the supplied epoch
    /// strictly advances the epoch owned by the old runtime.
    pub fn commit_activation(&mut self, committed_epoch: RootEpoch) -> Result<(), TransitionError> {
        if self.state != RootSwitchState::CommitActivation {
            return Err(TransitionError::Illegal {
                from: format!("{:?}", self.state),
                to: "RebindRuntime".into(),
                hint: "activation can commit only after old runtime quiesces",
            });
        }
        if committed_epoch <= self.active_epoch {
            return Err(TransitionError::Illegal {
                from: format!("epoch {}", self.active_epoch.as_u64()),
                to: format!("epoch {}", committed_epoch.as_u64()),
                hint: "root_epoch must increase on activation commit",
            });
        }
        self.active_epoch = committed_epoch;
        self.state = RootSwitchState::RebindRuntime;
        Ok(())
    }

    /// Whether an asynchronous result belongs to the currently bound root.
    ///
    /// Before the activation commit (`Prepare`/`QuiesceOldRoot`/
    /// `CommitActivation`) — and again after a `Failed` unwind — the *old*
    /// runtime keeps serving, so results stamped with the current (old) epoch
    /// are accepted. Once the commit advances the epoch, only results stamped
    /// with the new epoch are accepted; old-epoch results become stale
    /// immediately.
    #[must_use]
    pub fn accepts_epoch(&self, epoch: RootEpoch) -> bool {
        epoch == self.active_epoch
    }
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn root_switch_barrier_requires_quiesce_commit_and_rebind() {
        let old_root = LibraryRootId::new();
        let candidate = LibraryRootId::new();
        let mut barrier = RootSwitchBarrier::new(Some(old_root), candidate, RootEpoch::from_u64(4));
        assert!(barrier
            .transition(RootSwitchState::CommitActivation)
            .is_err());
        // Before the commit the old runtime keeps serving: its own epoch is
        // accepted in every pre-commit phase, a future epoch is not.
        for phase in [
            RootSwitchState::Prepare,
            RootSwitchState::QuiesceOldRoot,
            RootSwitchState::CommitActivation,
        ] {
            assert_eq!(barrier.state(), phase);
            assert!(barrier.accepts_epoch(RootEpoch::from_u64(4)));
            assert!(!barrier.accepts_epoch(RootEpoch::from_u64(5)));
            barrier
                .transition(match phase {
                    RootSwitchState::Prepare => RootSwitchState::QuiesceOldRoot,
                    RootSwitchState::QuiesceOldRoot => RootSwitchState::CommitActivation,
                    _ => break,
                })
                .unwrap();
        }
        assert!(barrier.commit_activation(RootEpoch::from_u64(4)).is_err());
        barrier.commit_activation(RootEpoch::from_u64(5)).unwrap();
        assert!(!barrier.accepts_epoch(RootEpoch::from_u64(4)));
        assert!(barrier.accepts_epoch(RootEpoch::from_u64(5)));
        barrier.transition(RootSwitchState::Completed).unwrap();
        assert!(barrier.state().is_terminal());
        assert!(barrier.transition(RootSwitchState::Prepare).is_err());
        assert!(barrier.accepts_epoch(RootEpoch::from_u64(5)));

        let failed = RootSwitchState::CommitActivation
            .transition(RootSwitchState::Failed)
            .unwrap();
        assert_eq!(
            failed.transition(RootSwitchState::Prepare).unwrap(),
            RootSwitchState::Prepare
        );
    }

    #[test]
    fn failed_switch_unfreezes_the_old_runtime_epoch() {
        // A switch that fails after quiesce re-enters service on the old root:
        // results stamped with the still-current old epoch must be accepted
        // again through `Failed` and back to `Prepare`.
        let mut barrier = RootSwitchBarrier::new(
            Some(LibraryRootId::new()),
            LibraryRootId::new(),
            RootEpoch::from_u64(7),
        );
        barrier.transition(RootSwitchState::QuiesceOldRoot).unwrap();
        barrier.transition(RootSwitchState::Failed).unwrap();
        barrier.transition(RootSwitchState::Prepare).unwrap();
        assert!(barrier.state() == RootSwitchState::Prepare);
        assert!(barrier.accepts_epoch(RootEpoch::from_u64(7)));
        assert!(!barrier.accepts_epoch(RootEpoch::from_u64(8)));
    }
}
