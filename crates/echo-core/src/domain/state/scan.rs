//! The library-scan state machine.

use super::TransitionError;

/// States of a library scan run.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ScanState {
    #[default]
    Queued,
    Enumerating,
    Parsing,
    Reconciling,
    Completed,
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
}
