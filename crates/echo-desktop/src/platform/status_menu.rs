//! macOS menu-bar / Windows-Linux tray adapter logic (task 9.3).
//!
//! The 1.9 Gate prototype only offered 显示/退出. This module productionizes
//! the entry: a current-play summary plus 播放/暂停、上一首、下一首、显示、
//! 退出 (spec "系统托盘与菜单栏入口必须提供一致的后台控制").
//!
//! The Tauri shell (main.rs) builds the real `Menu`/`TrayIcon` and delegates
//! every *decision* here — the pinned semantic item ids, the summary lines, the
//! play/pause label, and which [`PlayerCommand`] a clicked transport item maps
//! to. Keeping that logic free of Tauri types lets the menu/tray contract be
//! proven by unit tests while the binary stays thin and cannot drift.
//!
//! Transport items emit a coarse [`PlayerCommand`] to the coordinator; 显示 and
//! 退出 are the shell's window-lifecycle concern and return `None` here.

use echo_core::domain::state::PlaybackState;

use crate::player::port::{PlayerCommand, PlayerError};

/// Semantic id of the "显示 Echo" item — wakes and focuses the main window.
pub const MENU_SHOW: &str = "show";
/// Semantic id of the play/pause toggle item.
pub const MENU_PLAY_PAUSE: &str = "play_pause";
/// Semantic id of the previous-track item.
pub const MENU_PREVIOUS: &str = "previous";
/// Semantic id of the next-track item.
pub const MENU_NEXT: &str = "next";
/// Semantic id of the quit item — explicit application exit.
pub const MENU_QUIT: &str = "quit";

/// The current play summary shown by the platform entry's first lines.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PlaySummary {
    /// The current track's display name — intentionally a display name, never
    /// a filesystem path (the shell must not leak absolute paths).
    pub display_name: Option<String>,
    /// The authoritative playback state.
    pub state: PlaybackState,
    /// Total queue length (including the current entry).
    pub queue_len: usize,
}

impl PlaySummary {
    /// The first menu line: the now-playing track, a "正在加载…" placeholder
    /// while loading, or a "未在播放" placeholder when stopped.
    #[must_use]
    pub fn title_line(&self) -> String {
        match self.state {
            PlaybackState::Loading => "正在加载…".to_owned(),
            PlaybackState::Stopped | PlaybackState::Ended | PlaybackState::Failed => {
                "未在播放".to_owned()
            }
            PlaybackState::Playing | PlaybackState::Paused => self
                .display_name
                .clone()
                .unwrap_or_else(|| "未知曲目".to_owned()),
        }
    }

    /// The second menu line: state + queue length, e.g. "播放中 · 队列 3".
    #[must_use]
    pub fn status_line(&self) -> String {
        let state = match self.state {
            PlaybackState::Playing => "播放中",
            PlaybackState::Paused => "已暂停",
            PlaybackState::Loading => "加载中",
            PlaybackState::Stopped => "已停止",
            PlaybackState::Ended => "已结束",
            PlaybackState::Failed => "播放失败",
        };
        format!("{state} · 队列 {}", self.queue_len)
    }
}

/// The play/pause item label must mirror the state the coordinator owns: when
/// the player is playing, the item offers to pause ("暂停"); any other active
/// or idle state offers to play ("播放").
#[must_use]
pub fn play_pause_label(state: PlaybackState) -> &'static str {
    if state == PlaybackState::Playing {
        "暂停"
    } else {
        "播放"
    }
}

/// Map a clicked transport item's semantic id to the coarse playback command
/// the coordinator acts on. 显示/退出 are window-lifecycle actions and return
/// `None` — the shell handles them (window lifecycle is not a player command).
#[must_use]
pub fn transport_command(id: &str) -> Option<PlayerCommand> {
    match id {
        MENU_PLAY_PAUSE => Some(PlayerCommand::TogglePlayPause),
        MENU_PREVIOUS => Some(PlayerCommand::Previous),
        MENU_NEXT => Some(PlayerCommand::Next),
        // MENU_SHOW / MENU_QUIT belong to the shell.
        _ => None,
    }
}

/// The seam the shell's tray/menu uses to deliver a transport click. The
/// composition root connects this to the [`PlaybackCoordinator`]; until the
/// runtime is composed the shell manages a [`NoopSink`] so the items exist but
/// no coordinator is forced.
///
/// [`PlaybackCoordinator`]: crate::player::coordinator::PlaybackCoordinator
pub trait StatusMenuSink: Send + Sync {
    /// Deliver one coarse playback command from a transport click.
    ///
    /// # Errors
    ///
    /// Returns [`PlayerError`] when the transport cannot perform the command
    /// (for example when there is nothing playable or the player rejects it);
    /// the shell surfaces this as an unavailable/unsupported result.
    fn on_command(&self, command: PlayerCommand) -> Result<(), PlayerError>;
}

/// Default sink: does nothing. The shell manages this until the composition
/// root installs the real coordinator-forwarding sink.
pub struct NoopSink;

impl StatusMenuSink for NoopSink {
    fn on_command(&self, _command: PlayerCommand) -> Result<(), PlayerError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_shows_the_display_name_never_a_path() {
        let s = PlaySummary {
            display_name: Some("周杰伦 - 晴天".to_owned()),
            state: PlaybackState::Playing,
            queue_len: 3,
        };
        assert_eq!(s.title_line(), "周杰伦 - 晴天");
        assert_eq!(s.status_line(), "播放中 · 队列 3");
    }

    #[test]
    fn paused_summary_keeps_the_track_and_adds_a_play_label() {
        let s = PlaySummary {
            display_name: Some("晴天".to_owned()),
            state: PlaybackState::Paused,
            queue_len: 1,
        };
        assert_eq!(s.title_line(), "晴天");
        assert_eq!(s.status_line(), "已暂停 · 队列 1");
        assert_eq!(play_pause_label(s.state), "播放");
    }

    #[test]
    fn stopped_and_unknown_track_use_placeholders() {
        assert_eq!(
            PlaySummary {
                state: PlaybackState::Stopped,
                ..PlaySummary::default()
            }
            .title_line(),
            "未在播放"
        );
        assert_eq!(
            PlaySummary {
                display_name: Some("x.mp3".to_owned()),
                state: PlaybackState::Playing,
                queue_len: 0,
            }
            .title_line(),
            "x.mp3",
            "display name is shown verbatim (it is never an absolute path)"
        );
    }

    #[test]
    fn loading_uses_an_explicit_placeholder() {
        let s = PlaySummary {
            state: PlaybackState::Loading,
            queue_len: 5,
            ..PlaySummary::default()
        };
        assert_eq!(s.title_line(), "正在加载…");
        assert_eq!(s.status_line(), "加载中 · 队列 5");
    }

    #[test]
    fn play_pause_label_mirrors_the_playing_state() {
        assert_eq!(play_pause_label(PlaybackState::Playing), "暂停");
        assert_eq!(play_pause_label(PlaybackState::Paused), "播放");
        assert_eq!(play_pause_label(PlaybackState::Stopped), "播放");
        assert_eq!(play_pause_label(PlaybackState::Loading), "播放");
    }

    #[test]
    fn transport_items_map_to_coarse_player_commands() {
        assert_eq!(
            transport_command(MENU_PLAY_PAUSE),
            Some(PlayerCommand::TogglePlayPause)
        );
        assert_eq!(
            transport_command(MENU_PREVIOUS),
            Some(PlayerCommand::Previous)
        );
        assert_eq!(transport_command(MENU_NEXT), Some(PlayerCommand::Next));
    }

    #[test]
    fn show_and_quit_are_shell_lifecycle_not_player_commands() {
        assert_eq!(transport_command(MENU_SHOW), None);
        assert_eq!(transport_command(MENU_QUIT), None);
        assert_eq!(transport_command("unknown"), None);
    }
}
