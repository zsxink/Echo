//! A short native gain envelope around one playing seek. The actor owns time
//! and all backend calls; no audio data, user volume or mute is modified here.

use std::time::{Duration, Instant};

use super::{ActorLoop, Backend, BackendEvent, BackendProperty, PlaybackState, PlayerCommand};

const FADE: Duration = Duration::from_millis(12);
const TICK_INTERVAL: Duration = Duration::from_millis(3);
const DIRECT_QUIET: Duration = Duration::from_millis(15);
const FLOOR_HOLD: Duration = Duration::from_millis(5);
const GAIN_FLOOR_DB: f64 = -96.0;
const RESTART_LIMIT: Duration = Duration::from_millis(500);
const UNITY_RETRIES: u8 = 2;

#[derive(Default)]
enum Stage {
    #[default]
    Idle,
    FadeOut {
        start: Instant,
        target: f64,
    },
    Floor {
        deadline: Instant,
        target: f64,
    },
    AwaitRestart {
        deadline: Instant,
        started: bool,
        latest: Option<f64>,
    },
    FadeIn {
        start: Instant,
    },
    Restore {
        attempts: u8,
    },
    RestoreFailed {
        reported: bool,
    },
}

#[derive(Default)]
enum Readiness {
    #[default]
    BeforeFileLoaded,
    LoadRestart,
    DirectSeek {
        target: f64,
        started: bool,
        settle_after: Option<Instant>,
        deadline: Instant,
    },
    Ready,
}

#[derive(Default)]
pub(super) struct SeekTransition {
    stage: Stage,
    gain_may_be_lowered: bool,
    // After a lost in-flight fence mpv events do not identify their command.
    // Do not let a late start/restart pair unlock a newer protected seek.
    abandoned_fence: bool,
    gain_disabled: bool,
    // The native load/restart (or a direct, unprotected seek) must finish
    // before its events can be distinguished from a fresh protected seek.
    readiness: Readiness,
    later: Option<f64>,
    last_gain_write: Option<Instant>,
}

impl SeekTransition {
    pub(super) const fn fading(&self) -> bool {
        matches!(
            self.stage,
            Stage::FadeOut { .. } | Stage::FadeIn { .. } | Stage::Floor { .. }
        )
    }

    pub(super) fn request(
        &mut self,
        now: Instant,
        target: f64,
        audible: bool,
        backend: &mut impl Backend,
    ) -> Option<f64> {
        if !backend.supports_seek_gain() {
            return seek(backend, target);
        }
        if audible
            && !self.abandoned_fence
            && !self.gain_disabled
            && matches!(self.readiness, Readiness::DirectSeek { .. })
        {
            self.later = Some(target);
            return None;
        }
        if !audible
            || !matches!(self.readiness, Readiness::Ready)
            || self.abandoned_fence
            || self.gain_disabled
        {
            self.cancel(backend);
            return self.direct_seek(now, backend, target);
        }
        match &mut self.stage {
            Stage::Idle => {
                self.last_gain_write = None;
                self.stage = Stage::FadeOut { start: now, target };
            }
            Stage::FadeOut { target: latest, .. } | Stage::Floor { target: latest, .. } => {
                *latest = target;
            }
            Stage::AwaitRestart { latest, .. } => *latest = Some(target),
            // Once a ramp back begins, finish its short envelope first. A
            // new fade starts at unity, rather than jumping up then back down.
            Stage::FadeIn { .. } | Stage::Restore { .. } | Stage::RestoreFailed { .. } => {
                // Store the last target separately from the current fence.
                self.later = Some(target);
            }
        }
        None
    }

    pub(super) fn tick(
        &mut self,
        now: Instant,
        audible: bool,
        backend: &mut impl Backend,
    ) -> Option<f64> {
        match self.stage {
            Stage::FadeOut { start, target } if self.gain_tick_due(now) => {
                let fraction = fraction(now, start);
                self.last_gain_write = Some(now);
                if !self.gain(backend, fade_db(1.0 - fraction)) {
                    tracing::warn!(
                        "mpv actor: independent seek gain unavailable; using direct seek"
                    );
                    self.gain_disabled = true;
                    self.restore(backend);
                    return seek(backend, target);
                }
                if fraction >= 1.0 {
                    self.stage = Stage::Floor {
                        deadline: now + FLOOR_HOLD,
                        target,
                    };
                }
            }
            Stage::Floor { deadline, target } if now >= deadline => {
                return self.dispatch(now, backend, target);
            }
            Stage::AwaitRestart {
                deadline, latest, ..
            } if now >= deadline => {
                tracing::warn!("mpv actor: seek restart not confirmed; disabling seek gain");
                self.abandoned_fence = true;
                self.restore(backend);
                return latest.and_then(|target| seek(backend, target));
            }
            Stage::FadeIn { start } if self.gain_tick_due(now) => {
                let fraction = fraction(now, start);
                self.last_gain_write = Some(now);
                if fraction >= 1.0 {
                    self.restore(backend);
                } else if !self.gain(backend, fade_db(fraction)) {
                    self.gain_disabled = true;
                    self.restore(backend);
                }
            }
            Stage::Restore { attempts } => {
                if backend.set_seek_gain_db(0.0) {
                    self.gain_may_be_lowered = false;
                    self.stage = Stage::Idle;
                } else if attempts >= UNITY_RETRIES {
                    // Give the native boundary one distinct, checked recovery
                    // route before giving up on this handle's gain control.
                    self.gain_disabled = true;
                    if backend.reset_seek_gain() {
                        self.gain_may_be_lowered = false;
                        self.stage = Stage::Idle;
                    } else {
                        tracing::error!("mpv actor: native seek gain could not be restored");
                        self.later = None;
                        self.stage = Stage::RestoreFailed { reported: false };
                    }
                } else {
                    self.stage = Stage::Restore {
                        attempts: attempts + 1,
                    };
                }
            }
            _ => {}
        }
        if matches!(self.stage, Stage::Idle) {
            if let Readiness::DirectSeek { deadline, .. } = self.readiness {
                if now < deadline {
                    return None;
                }
                if let Some(target) = self.later.take() {
                    return self.direct_seek(now, backend, target);
                }
            } else if let Some(target) = self.later.take() {
                return self.request(now, target, audible, backend);
            }
        }
        None
    }

    pub(super) const fn started(&mut self) {
        match (&mut self.stage, &mut self.readiness) {
            (Stage::AwaitRestart { started, .. }, _) => *started = true,
            (
                _,
                Readiness::DirectSeek {
                    started,
                    settle_after,
                    ..
                },
            ) => {
                *started = true;
                *settle_after = None;
            }
            _ => {}
        }
    }

    pub(super) fn restarted(&mut self, now: Instant, backend: &mut impl Backend) -> Option<f64> {
        match self.stage {
            Stage::AwaitRestart {
                started: true,
                latest,
                ..
            } => {
                if let Some(target) = latest {
                    return self.dispatch(now, backend, target);
                }
                self.stage = Stage::FadeIn { start: now };
            }
            _ => match &mut self.readiness {
                Readiness::LoadRestart => self.readiness = Readiness::Ready,
                Readiness::DirectSeek {
                    started: true,
                    settle_after,
                    ..
                } => {
                    *settle_after = Some(now + DIRECT_QUIET);
                }
                _ => {}
            },
        }
        None
    }

    pub(super) fn backend_quiet(&mut self, now: Instant, backend: &impl Backend) {
        if let Readiness::DirectSeek {
            target,
            settle_after: Some(after),
            deadline,
            ..
        } = &mut self.readiness
        {
            if now >= *after && now < *deadline {
                if backend.direct_seek_settled(*target) {
                    self.readiness = Readiness::Ready;
                } else {
                    *after = now + DIRECT_QUIET;
                }
            }
        }
    }

    pub(super) fn new_file(&mut self, backend: &mut impl Backend) {
        self.cancel(backend);
        self.readiness = Readiness::BeforeFileLoaded;
    }

    pub(super) const fn file_loaded(&mut self) {
        // Events from the superseded file precede FILE_LOADED in libmpv's
        // ordered queue. A restart from before this boundary cannot arm us.
        self.readiness = Readiness::LoadRestart;
        if !self.gain_may_be_lowered {
            self.abandoned_fence = false;
        }
    }

    pub(super) fn cancel(&mut self, backend: &mut impl Backend) {
        if matches!(self.stage, Stage::AwaitRestart { .. }) {
            self.abandoned_fence = true;
        }
        self.later = None;
        // Don't replenish the retry budget if several commands are drained
        // before the loop gets a chance to perform recovery.
        if !matches!(
            self.stage,
            Stage::Restore { .. } | Stage::RestoreFailed { .. }
        ) {
            self.restore(backend);
        }
    }

    pub(super) const fn restoration_fault(&self) -> bool {
        matches!(
            self.stage,
            Stage::Restore { .. } | Stage::RestoreFailed { .. }
        )
    }

    const fn take_restore_failure(&mut self) -> bool {
        if let Stage::RestoreFailed { reported } = &mut self.stage {
            let first_report = !*reported;
            *reported = true;
            first_report
        } else {
            false
        }
    }

    fn confirm_playable(&mut self, backend: &mut impl Backend) -> bool {
        if !self.restoration_fault() {
            return true;
        }
        if backend.reset_seek_gain() {
            self.gain_may_be_lowered = false;
            self.stage = Stage::Idle;
            return true;
        }
        false
    }

    fn gain_tick_due(&self, now: Instant) -> bool {
        self.last_gain_write
            .is_none_or(|last| now.saturating_duration_since(last) >= TICK_INTERVAL)
    }

    fn direct_seek(
        &mut self,
        now: Instant,
        backend: &mut impl Backend,
        target: f64,
    ) -> Option<f64> {
        let position = seek(backend, target);
        if position.is_some() {
            self.readiness = Readiness::DirectSeek {
                target,
                started: false,
                settle_after: None,
                deadline: now + RESTART_LIMIT,
            };
        }
        position
    }

    fn dispatch(&mut self, now: Instant, backend: &mut impl Backend, target: f64) -> Option<f64> {
        let position = seek(backend, target);
        if position.is_some() {
            self.stage = Stage::AwaitRestart {
                deadline: now + RESTART_LIMIT,
                started: false,
                latest: None,
            };
        } else {
            self.stage = Stage::FadeIn { start: now };
        }
        position
    }

    fn gain(&mut self, backend: &mut impl Backend, db: f64) -> bool {
        if backend.set_seek_gain_db(db) {
            self.gain_may_be_lowered = db < 0.0;
            true
        } else {
            false
        }
    }

    fn restore(&mut self, backend: &mut impl Backend) {
        self.stage = if !self.gain_may_be_lowered || self.gain(backend, 0.0) {
            Stage::Idle
        } else {
            Stage::Restore { attempts: 0 }
        };
    }
}

fn fraction(now: Instant, start: Instant) -> f64 {
    (now.saturating_duration_since(start).as_secs_f64() / FADE.as_secs_f64()).min(1.0)
}

fn fade_db(fraction: f64) -> f64 {
    let factor = (1.0 - (fraction * std::f64::consts::PI).cos()) * 0.5;
    (20.0 * factor.log10()).max(GAIN_FLOOR_DB)
}

fn seek(backend: &mut impl Backend, target: f64) -> Option<f64> {
    backend
        .write_property(BackendProperty::Seek(target))
        .then_some(target)
}

impl<B: Backend> ActorLoop<B> {
    fn seek_is_audible(&self) -> bool {
        self.state == PlaybackState::Playing
            && !self.intended_paused
            && !self.effects_held
            && !self.muted
            && self.volume > 0.0
    }

    fn publish_seek(&mut self, position: f64) {
        self.position = Some(position);
        self.publish(self.generation);
    }

    pub(super) fn allow_seek_transport_command(&mut self, command: &PlayerCommand) -> bool {
        let loads_file = matches!(
            command,
            PlayerCommand::LoadLibrarySong { .. }
                | PlayerCommand::LoadLibrarySongPaused { .. }
                | PlayerCommand::LoadTemporary { .. }
        );
        if loads_file {
            self.seek_transition.cancel(&mut self.backend);
        }
        let may_resume = loads_file
            || matches!(command, PlayerCommand::Play)
            || (matches!(command, PlayerCommand::TogglePlayPause)
                && self.state == PlaybackState::Paused);
        !may_resume || self.seek_transition.confirm_playable(&mut self.backend)
    }

    pub(super) fn request_seek(&mut self, position: f64) {
        if self.state == PlaybackState::Loading {
            self.pending_seek = Some(position);
            return;
        }
        let audible = self.seek_is_audible();
        if let Some(accepted) =
            self.seek_transition
                .request(Instant::now(), position, audible, &mut self.backend)
        {
            self.publish_seek(accepted);
        }
    }

    pub(super) fn tick_seek_transition(&mut self) {
        let audible = self.seek_is_audible();
        if let Some(position) =
            self.seek_transition
                .tick(Instant::now(), audible, &mut self.backend)
        {
            self.publish_seek(position);
        }
        if self.seek_transition.take_restore_failure() {
            // No known writable gain path remains. Do not advertise a
            // playing-but-silent session or publish a discarded target.
            let _ = self.backend.write_property(BackendProperty::Pause(true));
            self.intended_paused = true;
            self.effects_held = false;
            self.state = PlaybackState::Failed;
            self.position = None;
            self.publish(self.generation);
        }
    }

    pub(super) fn restart_seek_transition(&mut self) {
        if let Some(position) = self
            .seek_transition
            .restarted(Instant::now(), &mut self.backend)
        {
            self.publish_seek(position);
        }
    }

    pub(super) fn pump_seek_backend(&mut self) -> Option<BackendEvent> {
        if self.seek_transition.fading() {
            self.backend.pump_seek_fade()
        } else {
            let event = self.backend.pump();
            if event.is_none() {
                self.seek_transition
                    .backend_quiet(Instant::now(), &self.backend);
            }
            event
        }
    }
}

#[cfg(test)]
#[path = "seek_transition_tests.rs"]
mod tests;
