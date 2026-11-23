use super::*;
use std::path::Path;
use std::sync::{Arc, Mutex, RwLock};

#[derive(Debug, PartialEq)]
enum Write {
    Property(BackendProperty),
    Gain(f64),
    Reset,
}

#[derive(Default)]
struct Probe {
    writes: Vec<Write>,
    reject_seek: bool,
    reject_gain: bool,
    reject_unity: bool,
    reject_reset: bool,
}

impl Backend for Probe {
    fn pump(&mut self) -> Option<BackendEvent> {
        None
    }

    fn queue_load(&mut self, _: &Path) {}

    fn observe_continuous(&mut self) {}

    fn write_property(&mut self, prop: BackendProperty) -> bool {
        self.writes.push(Write::Property(prop));
        !self.reject_seek || !matches!(prop, BackendProperty::Seek(_))
    }

    fn supports_seek_gain(&self) -> bool {
        true
    }

    fn set_seek_gain_db(&mut self, db: f64) -> bool {
        self.writes.push(Write::Gain(db));
        !self.reject_gain && (!self.reject_unity || db < 0.0)
    }

    fn reset_seek_gain(&mut self) -> bool {
        self.writes.push(Write::Reset);
        !self.reject_reset
    }

    fn terminate(&mut self) {}
}

fn ready(now: Instant, probe: &mut Probe) -> SeekTransition {
    let mut transition = SeekTransition::default();
    transition.file_loaded();
    assert_eq!(transition.restarted(now, probe), None);
    transition
}

fn ms(now: Instant, offset: u64) -> Instant {
    now + Duration::from_millis(offset)
}

fn fade_out(transition: &mut SeekTransition, now: Instant, probe: &mut Probe) -> Option<f64> {
    for offset in [0, 3, 6, 9, 12, 15] {
        assert_eq!(transition.tick(ms(now, offset), true, probe), None);
    }
    transition.tick(ms(now, 17), true, probe)
}

#[test]
fn keeps_gain_low_until_the_started_seek_restarts_and_coalesces_latest() {
    let now = Instant::now();
    let mut probe = Probe::default();
    let mut transition = ready(now, &mut probe);
    assert_eq!(transition.request(now, 1.0, true, &mut probe), None);
    assert_eq!(transition.request(now, 2.0, true, &mut probe), None);
    assert_eq!(fade_out(&mut transition, now, &mut probe), Some(2.0));
    assert!(probe.writes.windows(2).any(|pair| matches!(
        pair,
        [Write::Gain(db), Write::Property(BackendProperty::Seek(2.0))] if *db <= -90.0
    )));
    assert_eq!(transition.request(ms(now, 18), 3.0, true, &mut probe), None);
    assert_eq!(transition.request(ms(now, 19), 4.0, true, &mut probe), None);

    let writes = probe.writes.len();
    assert_eq!(transition.restarted(ms(now, 20), &mut probe), None);
    assert_eq!(probe.writes.len(), writes, "stray restart must not fade in");
    transition.started();
    assert_eq!(transition.restarted(ms(now, 21), &mut probe), Some(4.0));
    assert_eq!(
        probe.writes.last(),
        Some(&Write::Property(BackendProperty::Seek(4.0)))
    );
    transition.started();
    assert_eq!(transition.restarted(ms(now, 22), &mut probe), None);
    for offset in [25, 28, 31, 34] {
        assert_eq!(transition.tick(ms(now, offset), true, &mut probe), None);
    }
    assert_eq!(probe.writes.last(), Some(&Write::Gain(0.0)));
    assert!(!transition.fading());
}

#[test]
fn failed_seek_does_not_commit_position_and_recovers_gain() {
    let now = Instant::now();
    let mut probe = Probe {
        reject_seek: true,
        ..Probe::default()
    };
    let mut transition = ready(now, &mut probe);
    assert_eq!(transition.request(now, 2.0, true, &mut probe), None);
    assert_eq!(fade_out(&mut transition, now, &mut probe), None);
    assert_eq!(transition.tick(ms(now, 29), true, &mut probe), None);
    assert_eq!(probe.writes.last(), Some(&Write::Gain(0.0)));
}

#[test]
fn unsupported_gain_downgrades_to_the_normal_seek_path_across_loads() {
    let now = Instant::now();
    let mut probe = Probe {
        reject_gain: true,
        ..Probe::default()
    };
    let mut transition = ready(now, &mut probe);
    assert_eq!(transition.request(now, 1.0, true, &mut probe), None);
    assert_eq!(transition.tick(now, true, &mut probe), Some(1.0));
    transition.new_file(&mut probe);
    transition.file_loaded();
    assert_eq!(transition.restarted(now, &mut probe), None);
    assert_eq!(transition.request(now, 2.0, true, &mut probe), Some(2.0));
    assert_eq!(
        probe.writes,
        [
            Write::Gain(0.0),
            Write::Property(BackendProperty::Seek(1.0)),
            Write::Property(BackendProperty::Seek(2.0)),
        ]
    );
}

#[test]
fn lost_restart_falls_open_until_an_ordered_new_file_fence() {
    let now = Instant::now();
    let mut probe = Probe::default();
    let mut transition = ready(now, &mut probe);
    transition.request(now, 1.0, true, &mut probe);
    assert_eq!(fade_out(&mut transition, now, &mut probe), Some(1.0));
    transition.request(ms(now, 18), 2.0, true, &mut probe);
    assert_eq!(transition.tick(ms(now, 517), true, &mut probe), Some(2.0));
    assert_eq!(
        probe.writes[probe.writes.len() - 2..],
        [
            Write::Gain(0.0),
            Write::Property(BackendProperty::Seek(2.0))
        ]
    );
    transition.started();
    assert_eq!(transition.restarted(ms(now, 518), &mut probe), None);
    assert_eq!(
        transition.request(ms(now, 519), 3.0, true, &mut probe),
        Some(3.0)
    );
    transition.new_file(&mut probe);
    assert_eq!(transition.restarted(ms(now, 520), &mut probe), None);
    assert!(matches!(transition.readiness, Readiness::BeforeFileLoaded));
    transition.file_loaded();
    assert_eq!(transition.restarted(ms(now, 521), &mut probe), None);
    assert_eq!(
        transition.request(ms(now, 522), 4.0, true, &mut probe),
        None
    );
    assert!(transition.fading());
}

#[test]
fn paused_seek_is_immediate_and_ignores_unrelated_restart_before_seek_start() {
    let now = Instant::now();
    let mut probe = Probe::default();
    let mut transition = ready(now, &mut probe);
    assert_eq!(transition.request(now, 1.0, false, &mut probe), Some(1.0));
    assert_eq!(probe.writes, [Write::Property(BackendProperty::Seek(1.0))]);
    assert_eq!(transition.restarted(now, &mut probe), None);
    assert!(matches!(
        transition.readiness,
        Readiness::DirectSeek { started: false }
    ));
    transition.started();
    assert_eq!(transition.restarted(now, &mut probe), None);
    assert_eq!(transition.request(now, 2.0, true, &mut probe), None);
    assert!(transition.fading());
}

#[test]
fn cancellation_bounds_unity_retries_and_drops_queued_seek() {
    let now = Instant::now();
    let mut probe = Probe::default();
    let mut transition = ready(now, &mut probe);
    transition.request(now, 1.0, true, &mut probe);
    assert_eq!(fade_out(&mut transition, now, &mut probe), Some(1.0));
    transition.request(ms(now, 18), 2.0, true, &mut probe);
    probe.reject_unity = true;
    transition.cancel(&mut probe);
    for offset in 20..26 {
        transition.cancel(&mut probe); // must not reset the retry budget
        assert_eq!(transition.tick(ms(now, offset), true, &mut probe), None);
    }
    assert!(probe.writes.contains(&Write::Reset));
    assert!(!probe
        .writes
        .contains(&Write::Property(BackendProperty::Seek(2.0))));
    assert!(!transition.gain_may_be_lowered);
}

#[test]
fn actor_mute_and_volume_properties_are_orthogonal_to_seek_gain() {
    let now = Instant::now();
    let mut backend = Probe::default();
    let transition = ready(now, &mut backend);
    let mut actor = ActorLoop::new(
        backend,
        Arc::new(RwLock::new(super::super::PlayerSnapshot::default())),
        Arc::new(Mutex::new(Vec::new())),
    );
    actor.seek_transition = transition;
    actor.state = PlaybackState::Playing;
    actor
        .seek_transition
        .request(now, 2.0, true, &mut actor.backend);
    actor
        .seek_transition
        .tick(ms(now, 6), true, &mut actor.backend);
    actor.handle_command(super::super::PlayerCommand::SetVolume(0.4));
    actor.handle_command(super::super::PlayerCommand::ToggleMute);
    assert!((actor.volume - 0.4).abs() < f64::EPSILON && actor.muted);
    actor.handle_command(super::super::PlayerCommand::Stop);
    assert!(actor.muted);
    assert!(actor.backend.writes.ends_with(&[
        Write::Property(BackendProperty::Pause(true)),
        Write::Gain(0.0),
    ]));
    assert!(!actor
        .backend
        .writes
        .contains(&Write::Property(BackendProperty::Seek(2.0))));
}

#[test]
fn unrecoverable_native_gain_reports_failure_instead_of_silent_playback() {
    let now = Instant::now();
    let mut backend = Probe::default();
    let mut transition = ready(now, &mut backend);
    transition.request(now, 1.0, true, &mut backend);
    assert_eq!(fade_out(&mut transition, now, &mut backend), Some(1.0));
    backend.reject_unity = true;
    backend.reject_reset = true;
    transition.cancel(&mut backend);
    let mut actor = ActorLoop::new(
        backend,
        Arc::new(RwLock::new(super::super::PlayerSnapshot::default())),
        Arc::new(Mutex::new(Vec::new())),
    );
    actor.seek_transition = transition;
    actor.state = PlaybackState::Playing;
    for _ in 0..=UNITY_RETRIES {
        actor.tick_seek_transition();
    }
    assert_eq!(actor.state, PlaybackState::Failed);
    assert_eq!(
        actor.backend.writes.last(),
        Some(&Write::Property(BackendProperty::Pause(true)))
    );
    assert!(actor.seek_transition.gain_may_be_lowered);
}
