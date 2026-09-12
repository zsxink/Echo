//! Media-control seam (task 9.4): macOS Now Playing, Windows SMTC and Linux
//! MPRIS all reduce to "a media key pressed once → exactly one coordinator
//! command". The OS-specific adapters are platform-crate + composition-root
//! work; this module owns the platform-agnostic contract, so the cores of 9.4
//! are proven without any OS:
//!   - the per-OS key-name decoder ([`MediaKey::from_name`]);
//!   - the exactly-one mapping [`to_player_command`];
//!   - and an explicit degrade path ([`MediaControlSink`] that defaults to a
//!     no-op → the shell degrades to window/frontend control) so an unavailable
//!     capability never silently drops a press.

use crate::player::port::PlayerCommand;

/// A single decoded media key, agnostic of the OS that delivered it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MediaKey {
    /// Play or pause (toggle).
    PlayPause,
    /// Skip to the previous track.
    Previous,
    /// Skip to the next track.
    Next,
    /// Toggle mute.
    Mute,
    /// Step volume up.
    VolumeUp,
    /// Step volume down.
    VolumeDown,
    /// A key Echo does not act on (unknown or from an unrelated context).
    Unknown,
}

impl MediaKey {
    /// Decode a platform key name (lower-cased; hyphen/underscore folded to a
    /// space) into the shared vocabulary. Unknown names become [`Self::Unknown`]
    /// rather than erroring — a platform may surface keys we do not serve.
    #[must_use]
    pub fn from_name(name: &str) -> Self {
        let norm: String = name
            .chars()
            .map(|c| {
                if c == '_' || c == '-' {
                    ' '
                } else {
                    c.to_ascii_lowercase()
                }
            })
            .collect();
        match norm.as_str() {
            "play" | "pause" | "playpause" | "play pause" | "toggleplaypause" => Self::PlayPause,
            "previous" | "previoustrack" | "previous track" => Self::Previous,
            "next" | "nexttrack" | "next track" => Self::Next,
            "mute" | "toggle mute" | "togglemute" => Self::Mute,
            "volumeup" | "volume up" => Self::VolumeUp,
            "volumedown" | "volume down" => Self::VolumeDown,
            _ => Self::Unknown,
        }
    }
}

/// Map a decoded media key to exactly one coarse player command.
///
/// Transport and mute keys map directly. Volume keys are absolute on the
/// `PlayerCommand` surface (`SetVolume(f64)`), so a relative step must be
/// resolved against the coordinator's current volume in the sink — they return
/// `None` here rather than guessing an absolute value. An [`MediaKey::Unknown`]
/// key also yields `None` (never a fabricated toggle).
#[must_use]
pub fn to_player_command(key: MediaKey) -> Option<PlayerCommand> {
    match key {
        MediaKey::PlayPause => Some(PlayerCommand::TogglePlayPause),
        MediaKey::Previous => Some(PlayerCommand::Previous),
        MediaKey::Next => Some(PlayerCommand::Next),
        MediaKey::Mute => Some(PlayerCommand::ToggleMute),
        MediaKey::VolumeUp | MediaKey::VolumeDown | MediaKey::Unknown => None,
    }
}

/// The seam a platform adapter presses into. The composition root forwards
/// every press to the [`PlaybackCoordinator`]; until composed the shell
/// manages a no-op sink and degrades to window/frontend control.
///
/// [`PlaybackCoordinator`]: crate::player::coordinator::PlaybackCoordinator
pub trait MediaControlSink: Send + Sync {
    /// Deliver one media-key press as the corresponding coarse command.
    fn on_command(&self, command: PlayerCommand);
}

/// Default sink: does nothing. The shell manages this until the composition
/// root installs the coordinator forwarder.
pub struct NoopSink;

impl MediaControlSink for NoopSink {
    fn on_command(&self, _command: PlayerCommand) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transport_and_mute_map_to_exactly_one_command() {
        assert_eq!(
            to_player_command(MediaKey::PlayPause),
            Some(PlayerCommand::TogglePlayPause)
        );
        assert_eq!(
            to_player_command(MediaKey::Previous),
            Some(PlayerCommand::Previous)
        );
        assert_eq!(to_player_command(MediaKey::Next), Some(PlayerCommand::Next));
        assert_eq!(
            to_player_command(MediaKey::Mute),
            Some(PlayerCommand::ToggleMute)
        );
    }

    #[test]
    fn volume_steps_resolve_in_the_sink_not_here() {
        assert_eq!(to_player_command(MediaKey::VolumeUp), None);
        assert_eq!(to_player_command(MediaKey::VolumeDown), None);
    }

    #[test]
    fn unknown_key_never_becomes_a_toggle() {
        assert_eq!(to_player_command(MediaKey::Unknown), None);
    }

    #[test]
    fn key_names_decode_across_separators_and_case() {
        assert_eq!(MediaKey::from_name("PlayPause"), MediaKey::PlayPause);
        assert_eq!(MediaKey::from_name("play_pause"), MediaKey::PlayPause);
        assert_eq!(MediaKey::from_name("play-pause"), MediaKey::PlayPause);
        assert_eq!(MediaKey::from_name("previous"), MediaKey::Previous);
        assert_eq!(MediaKey::from_name("NEXT"), MediaKey::Next);
        assert_eq!(MediaKey::from_name("volume_up"), MediaKey::VolumeUp);
        assert_eq!(MediaKey::from_name("volume-down"), MediaKey::VolumeDown);
        assert_eq!(MediaKey::from_name("mute"), MediaKey::Mute);
        assert_eq!(MediaKey::from_name("some-other-key"), MediaKey::Unknown);
    }

    #[test]
    fn noop_sink_degrades_without_forwarding() {
        // The degrade path: a no-op sink accepts any command with no side
        // effects — no panic, nothing forwarded.
        let sink = NoopSink;
        sink.on_command(PlayerCommand::TogglePlayPause);
        sink.on_command(PlayerCommand::Next);
    }
}
