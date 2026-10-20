//! Unit tests for the actor machinery.
//!
//! Moved from the former monolithic `actor.rs` (dedented one level; the
//! `mod tests` wrapper is the file's root item).

use super::*;

fn snapshot_stub() -> Arc<RwLock<PlayerSnapshot>> {
    Arc::new(RwLock::new(PlayerSnapshot::default()))
}

/// An empty subscriber registry for tests that drive `ActorLoop` directly
/// (no forwarder attached).
fn subscribers_stub() -> Arc<Mutex<Vec<mpsc::SyncSender<PlayerSnapshot>>>> {
    Arc::new(Mutex::new(Vec::new()))
}

/// Spawn an actor over a `TestBackend` replaying `events`, returning the
/// actor, a shared snapshot the test can observe, and the shared property
/// recorder (so `write_property` calls can be asserted, task 8.8).
#[allow(clippy::type_complexity)] // test-only 3-tuple is fine here
fn spawn_test(
    events: Vec<BackendEvent>,
) -> (
    PlayerActor,
    Arc<RwLock<PlayerSnapshot>>,
    Arc<std::sync::Mutex<Vec<BackendProperty>>>,
) {
    let snapshot = snapshot_stub();
    let props = Arc::new(std::sync::Mutex::new(Vec::new()));
    let props_join = props.clone();
    let actor = PlayerActor::spawn_with(
        move || -> Result<TestBackend, ffi::HandleError> {
            let mut b = TestBackend::new(events);
            b.props = props_join;
            Ok(b)
        },
        snapshot.clone(),
        None,
    )
    .expect("spawn");
    (actor, snapshot, props)
}

/// Spawn an actor whose `TestBackend` counts how often the loop asked it to
/// subscribe to the continuous properties — the request the real backend
/// turns into `mpv_observe_property`. Without it the actor receives no
/// `time-pos`/`duration` events and every live surface freezes.
fn spawn_test_observed() -> (
    PlayerActor,
    Arc<RwLock<PlayerSnapshot>>,
    Arc<std::sync::atomic::AtomicUsize>,
) {
    let snapshot = snapshot_stub();
    let observations = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let observations_join = observations.clone();
    let actor = PlayerActor::spawn_with(
        move || -> Result<TestBackend, ffi::HandleError> {
            let mut b = TestBackend::new(vec![]);
            b.observations = observations_join;
            b.file_loaded_on_load = true;
            Ok(b)
        },
        snapshot.clone(),
        None,
    )
    .expect("spawn");
    (actor, snapshot, observations)
}

/// A backend whose `queue_load` emits its `FileLoaded` event afterwards,
/// matching the causal ordering of libmpv instead of relying on a startup
/// race in a scripted event queue.
type FileLoadedActorFixture = (
    PlayerActor,
    Arc<RwLock<PlayerSnapshot>>,
    Arc<std::sync::Mutex<Vec<BackendProperty>>>,
);

fn spawn_test_file_loaded_on_load() -> FileLoadedActorFixture {
    let snapshot = snapshot_stub();
    let props = Arc::new(std::sync::Mutex::new(Vec::new()));
    let props_join = props.clone();
    let actor = PlayerActor::spawn_with(
        move || -> Result<TestBackend, ffi::HandleError> {
            let mut backend = TestBackend::new(vec![]);
            backend.props = props_join;
            backend.file_loaded_on_load = true;
            Ok(backend)
        },
        snapshot.clone(),
        None,
    )
    .expect("spawn");
    (actor, snapshot, props)
}

/// A `LoadTemporary` command for the throwaway test path.
fn load_temporary(path: &str) -> PlayerCommand {
    PlayerCommand::LoadTemporary {
        display_name: "a.flac".into(),
        path: std::path::PathBuf::from(path),
        session_id: echo_core::domain::ids::PlaybackSessionId::new(),
    }
}

/// Wait (bounded, ~1s) until `cond` holds on the shared snapshot. The bound
/// must clear a loaded CI runner as well as a cold local machine: when the
/// whole workspace test suite runs in parallel, the actor thread competes
/// with hundreds of others and a 500 ms budget was occasionally not enough.
fn wait_for(
    snapshot: &Arc<RwLock<PlayerSnapshot>>,
    mut cond: impl FnMut(&PlayerSnapshot) -> bool,
) -> PlayerSnapshot {
    for _ in 0..200 {
        let s = snapshot.read().expect("snapshot poisoned").clone();
        if cond(&s) {
            return s;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    snapshot.read().expect("snapshot poisoned").clone()
}

fn wait_for_property_writes(
    props: &Arc<std::sync::Mutex<Vec<BackendProperty>>>,
    count: usize,
) -> Vec<BackendProperty> {
    // Same ~1s budget as `wait_for`: the property writes land from the actor
    // thread, which is starved when the whole workspace test suite runs in
    // parallel on a loaded CI runner.
    for _ in 0..200 {
        let writes = props.lock().expect("properties poisoned").clone();
        if writes.len() >= count {
            return writes;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    props.lock().expect("properties poisoned").clone()
}

fn tpos(v: f64) -> BackendEvent {
    BackendEvent::PropertyChanged {
        name: "time-pos".into(),
        value: v,
    }
}

#[allow(clippy::type_complexity)] // test-only 3-tuple is fine here
fn spawn_test_failing_property(
    events: Vec<BackendEvent>,
) -> (
    PlayerActor,
    Arc<RwLock<PlayerSnapshot>>,
    Arc<std::sync::Mutex<Vec<BackendProperty>>>,
) {
    let snapshot = snapshot_stub();
    let props = Arc::new(std::sync::Mutex::new(Vec::new()));
    let props_join = props.clone();
    let actor = PlayerActor::spawn_with(
        move || -> Result<TestBackend, ffi::HandleError> {
            let mut b = TestBackend::new(events);
            b.props = props_join;
            b.fail_all_properties = true;
            Ok(b)
        },
        snapshot.clone(),
        None,
    )
    .expect("spawn");
    (actor, snapshot, props)
}

#[path = "tests_load.rs"]
mod load;
#[path = "tests_native_mpv.rs"]
mod native_mpv;
#[path = "tests_snapshot.rs"]
mod snapshot;
#[path = "tests_transport_volume.rs"]
mod transport_volume;
#[path = "tests_validation.rs"]
mod validation;

/// Spawn an actor over a backend that resolves library songs through
/// `resolver` and emits a `FileLoaded` for **every** queued load (see
/// [`TestBackend::file_loaded_on_load`]), so a test can drive a real
/// `load → FileLoaded → load` sequence. `fail_all` makes every property
/// write fail, for the "never publish a state mpv did not reach" assertions.
#[allow(clippy::type_complexity)] // test-only 3-tuple is fine here
pub(super) fn spawn_test_load_driven(
    resolver: SongResolver,
    fail_all: bool,
) -> (
    PlayerActor,
    Arc<RwLock<PlayerSnapshot>>,
    Arc<std::sync::Mutex<Vec<BackendProperty>>>,
) {
    let snapshot = snapshot_stub();
    let props = Arc::new(std::sync::Mutex::new(Vec::new()));
    let props_join = props.clone();
    let actor = PlayerActor::spawn_with(
        move || -> Result<TestBackend, ffi::HandleError> {
            let mut b = TestBackend::new(vec![]);
            b.props = props_join;
            b.file_loaded_on_load = true;
            b.fail_all_properties = fail_all;
            Ok(b)
        },
        snapshot.clone(),
        Some(resolver),
    )
    .expect("spawn");
    (actor, snapshot, props)
}
