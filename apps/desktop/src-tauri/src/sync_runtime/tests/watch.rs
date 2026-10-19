//! Watch integration: between cycles a healthy peer holds one long-poll
//! against the host instead of sleeping on the plain waker.

use super::*;
use std::sync::PoisonError;

/// Scripted watch behaviour. Every call reports itself and either returns
/// the next outcome immediately or holds like a real long-poll until the
/// test releases it (or cancellation is requested).
struct WatchScript {
    /// Outcome for the Nth call; the last entry repeats.
    outcomes: Vec<WatchOutcome>,
    /// When true, calls block until released or cancelled.
    hold: bool,
    calls: usize,
}

struct WatchingProvisioner {
    database: Arc<SqliteDatabase>,
    cycles: mpsc::Sender<()>,
    script: Mutex<WatchScript>,
    release: Mutex<mpsc::Receiver<()>>,
    release_sender: mpsc::Sender<()>,
    /// Watches that must fail before the scripted outcome is served.
    failures: Mutex<usize>,
}

impl WatchingProvisioner {
    fn new(
        database: Arc<SqliteDatabase>,
        outcomes: Vec<WatchOutcome>,
        hold: bool,
    ) -> (Arc<Self>, mpsc::Receiver<()>) {
        let (cycles, completed) = mpsc::channel();
        let (release_sender, release) = mpsc::channel();
        let provisioner = Arc::new(Self {
            database,
            cycles,
            script: Mutex::new(WatchScript {
                outcomes,
                hold,
                calls: 0,
            }),
            release: Mutex::new(release),
            release_sender,
            failures: Mutex::new(0),
        });
        (provisioner, completed)
    }

    /// Fail this many watches before serving the scripted outcome.
    fn fail_watches(&self, count: usize) {
        *self.failures.lock().unwrap_or_else(PoisonError::into_inner) = count;
    }

    fn watch_calls(&self) -> usize {
        self.script
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .calls
    }

    /// End a held watch so it returns its scripted outcome.
    fn release_watch(&self) {
        let _ = self.release_sender.send(());
    }
}

impl SyncProvisioner for WatchingProvisioner {
    fn host(&self, _: LibraryRootId) -> Result<ProvisionedHost, SyncRuntimeError> {
        Err(SyncRuntimeError::Unconfigured)
    }

    fn peer(&self, _: LibraryRootId) -> Result<ProvisionedPeer, SyncRuntimeError> {
        Err(SyncRuntimeError::Unconfigured)
    }

    fn revoke_device(&self, _: &str) -> Result<(), SyncRuntimeError> {
        Ok(())
    }

    fn leave_sync(&self, _: LibraryRootId) -> Result<(), SyncRuntimeError> {
        Ok(())
    }

    fn complete_peer_pairing(
        &self,
        _: LibraryRootId,
        _: &PairingInvitation,
        _: &str,
        _: &PairingGrantDto,
    ) -> Result<(), SyncRuntimeError> {
        Err(SyncRuntimeError::Unconfigured)
    }
}

impl SyncCyclePort for WatchingProvisioner {
    fn sync_now(&self, _: SyncRole) -> Result<bool, SyncRuntimeError> {
        self.cycles
            .send(())
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        Ok(false)
    }
    fn watch_host_commits(
        &self,
        cancelled: Arc<AtomicBool>,
        _wait: Duration,
    ) -> Result<WatchOutcome, SyncRuntimeError> {
        let (outcome, hold) = {
            let mut script = self.script.lock().unwrap_or_else(PoisonError::into_inner);
            let index = script.calls.min(script.outcomes.len().saturating_sub(1));
            script.calls += 1;
            (script.outcomes[index], script.hold)
        };
        {
            let mut failures = self.failures.lock().unwrap_or_else(PoisonError::into_inner);
            if *failures > 0 {
                *failures -= 1;
                drop(failures);
                // A recycled keep-alive connection looks exactly like this: the
                // request dies without a response even though the host is up.
                return Err(SyncRuntimeError::SyncConnectionFailed);
            }
        }
        if hold {
            // Stand in for the held HTTP request: end on cancellation or
            // release, never on a timer, so the test controls the cadence. The
            // deadline is only a safety net so a regression that ignores
            // cancellation surfaces as a failed timing assertion instead of a
            // hung test binary.
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                if cancelled.load(Ordering::Acquire) {
                    return Err(SyncRuntimeError::Busy);
                }
                if Instant::now() >= deadline {
                    break;
                }
                let release = self.release.lock().unwrap_or_else(PoisonError::into_inner);
                match release.recv_timeout(Duration::from_millis(10)) {
                    Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
            }
        }
        Ok(outcome)
    }
}

struct RunningSync(SyncRuntime);

impl Drop for RunningSync {
    fn drop(&mut self) {
        // Also join the worker if a regression assertion fails.
        let _ = self.0.stop();
    }
}

fn peer_runtime(provisioner: Arc<WatchingProvisioner>) -> RunningSync {
    let runtime = RunningSync(SyncRuntime::new(
        Arc::clone(&provisioner.database),
        Some(provisioner),
    ));
    {
        let mut inner = runtime.0.inner.lock().unwrap();
        inner.status.enabled = true;
        inner.status.role = SyncRole::Peer;
        inner.status.state = SyncState::Connected;
    }
    runtime
}

fn drain_cycles(cycles: &mpsc::Receiver<()>) -> usize {
    let mut count = 0;
    while cycles.try_recv().is_ok() {
        count += 1;
    }
    count
}

fn wait_for_watch(provisioner: &WatchingProvisioner, expected: usize) {
    let started = Instant::now();
    while provisioner.watch_calls() < expected {
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the sync loop never entered its {expected}th watch"
        );
        thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_watch_outcome_alone_drives_the_next_cycle() {
    let directory = tempfile::tempdir().unwrap();
    let database = Arc::new(SqliteDatabase::open(directory.path().join("echo.sqlite")).unwrap());
    // The first watch reports a host publication; later watches keep holding.
    let (provisioner, cycles) =
        WatchingProvisioner::new(database, vec![WatchOutcome::Advanced], true);
    let runtime = peer_runtime(Arc::clone(&provisioner));
    runtime.0.start_sync_loop(SyncRole::Peer).unwrap();

    // The initial wake runs the first cycle; the held watch must not replace
    // it, and the host publication must run the next one without any local
    // write or waker hint.
    cycles.recv_timeout(Duration::from_secs(2)).unwrap();
    provisioner.release_watch();
    cycles
        .recv_timeout(Duration::from_secs(2))
        .expect("a watch outcome must schedule a cycle on its own");
    assert!(provisioner.watch_calls() >= 1);
    runtime.0.stop().unwrap();
}

#[test]
fn a_single_watch_transport_failure_is_retried_before_it_is_reported() {
    let directory = tempfile::tempdir().unwrap();
    let database = Arc::new(SqliteDatabase::open(directory.path().join("echo.sqlite")).unwrap());
    let (provisioner, cycles) =
        WatchingProvisioner::new(database, vec![WatchOutcome::Advanced], true);
    provisioner.fail_watches(1);
    let runtime = peer_runtime(Arc::clone(&provisioner));
    runtime.0.start_sync_loop(SyncRole::Peer).unwrap();

    // The initial wake runs a cycle; the next watch then fails the way a
    // recycled keep-alive connection does, with no response at all.
    cycles.recv_timeout(Duration::from_secs(2)).unwrap();
    wait_for_watch(&provisioner, 2);
    assert_ne!(
        runtime.0.inner.lock().unwrap().status.state,
        SyncState::Failed,
        "one dropped connection must not be published as a sync failure"
    );
    runtime.0.stop().unwrap();
}

#[test]
fn a_local_write_ends_a_held_watch_without_replaying_the_hint() {
    let directory = tempfile::tempdir().unwrap();
    let database = Arc::new(SqliteDatabase::open(directory.path().join("echo.sqlite")).unwrap());
    let (provisioner, cycles) =
        WatchingProvisioner::new(database, vec![WatchOutcome::LocalWrite], true);
    let runtime = peer_runtime(Arc::clone(&provisioner));
    runtime.0.start_sync_loop(SyncRole::Peer).unwrap();
    cycles.recv_timeout(Duration::from_secs(2)).unwrap();
    wait_for_watch(&provisioner, 1);

    // A local write wakes every listener, so the loop's own channel holds a
    // hint the watch already accounted for. Queue one before releasing.
    let loop_wake = runtime
        .0
        .inner
        .lock()
        .unwrap()
        .sync_loop
        .as_ref()
        .expect("the sync loop is running")
        .wake
        .clone();
    let _ = loop_wake.send(());
    provisioner.release_watch();

    cycles
        .recv_timeout(Duration::from_secs(2))
        .expect("a local write must end the watch and run a cycle");
    assert_eq!(
        cycles.recv_timeout(Duration::from_millis(500)),
        Err(mpsc::RecvTimeoutError::Timeout),
        "the drained hint must not be replayed as an extra cycle"
    );
    runtime.0.stop().unwrap();
}

#[test]
fn stopping_during_a_held_watch_exits_promptly() {
    let directory = tempfile::tempdir().unwrap();
    let database = Arc::new(SqliteDatabase::open(directory.path().join("echo.sqlite")).unwrap());
    let (provisioner, cycles) =
        WatchingProvisioner::new(database, vec![WatchOutcome::BudgetElapsed], true);
    let runtime = peer_runtime(Arc::clone(&provisioner));
    runtime.0.start_sync_loop(SyncRole::Peer).unwrap();
    cycles.recv_timeout(Duration::from_secs(2)).unwrap();
    wait_for_watch(&provisioner, 1);

    // Stop from another thread: a loop that ignored cancellation would block
    // the join forever, and the assertion below would fail instead of hanging.
    let (finished, outcome) = mpsc::channel();
    thread::spawn(move || {
        let started = Instant::now();
        let stopped = runtime.0.stop();
        let _ = finished.send((started.elapsed(), stopped.is_ok()));
    });
    let (elapsed, ok) = outcome
        .recv_timeout(Duration::from_secs(2))
        .expect("stopping during a held watch must not block the caller");
    assert!(ok);
    assert!(
        elapsed < Duration::from_secs(1),
        "stop took {elapsed:?}; a held watch must end as soon as it is cancelled"
    );
}

#[test]
fn a_host_never_opens_a_watch() {
    let directory = tempfile::tempdir().unwrap();
    let database = Arc::new(SqliteDatabase::open(directory.path().join("echo.sqlite")).unwrap());
    let (provisioner, cycles) =
        WatchingProvisioner::new(Arc::clone(&database), vec![WatchOutcome::Advanced], true);
    let dyn_provisioner: Arc<dyn SyncCyclePort> = provisioner.clone();
    let runtime = RunningSync(SyncRuntime::new(
        Arc::clone(&database),
        Some(dyn_provisioner),
    ));
    {
        let mut inner = runtime.0.inner.lock().unwrap();
        inner.status.enabled = true;
        inner.status.role = SyncRole::Host;
        inner.status.state = SyncState::Hosting;
    }
    runtime.0.start_sync_loop(SyncRole::Host).unwrap();
    // The host runs its maintenance cycle from the initial wake and then
    // waits on the plain waker; it has nothing remote to discover.
    cycles.recv_timeout(Duration::from_secs(2)).unwrap();
    thread::sleep(Duration::from_millis(300));
    assert_eq!(provisioner.watch_calls(), 0, "a host must not watch");
    runtime.0.stop().unwrap();
}

#[test]
fn a_host_without_the_wait_capability_keeps_plain_polling_without_a_busy_loop() {
    let directory = tempfile::tempdir().unwrap();
    let database = Arc::new(SqliteDatabase::open(directory.path().join("echo.sqlite")).unwrap());
    // An older host answers immediately: every watch reports Unsupported.
    let (provisioner, cycles) =
        WatchingProvisioner::new(database, vec![WatchOutcome::Unsupported], false);
    let runtime = peer_runtime(Arc::clone(&provisioner));
    runtime.0.start_sync_loop(SyncRole::Peer).unwrap();

    cycles.recv_timeout(Duration::from_secs(2)).unwrap();
    thread::sleep(Duration::from_millis(500));
    assert_eq!(
        drain_cycles(&cycles),
        0,
        "an unsupported watch must fall back to the plain interval, not spin"
    );
    assert_eq!(provisioner.watch_calls(), 1);
    runtime.0.stop().unwrap();
}

#[test]
fn the_initial_wake_runs_the_first_cycle_even_while_watching() {
    let directory = tempfile::tempdir().unwrap();
    let database = Arc::new(SqliteDatabase::open(directory.path().join("echo.sqlite")).unwrap());
    // The watch holds forever unless released; nothing releases it here.
    let (provisioner, cycles) =
        WatchingProvisioner::new(database, vec![WatchOutcome::BudgetElapsed], true);
    let runtime = peer_runtime(Arc::clone(&provisioner));
    runtime.0.start_sync_loop(SyncRole::Peer).unwrap();
    cycles
        .recv_timeout(Duration::from_secs(2))
        .expect("the initial wake must run the first cycle before any watch ends");
    runtime.0.stop().unwrap();
}
