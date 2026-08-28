//! The player domain state machine.

use super::TransitionError;

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
mod tests {
    use super::*;

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
}
