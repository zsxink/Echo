use std::sync::atomic::AtomicUsize;

use super::*;
use echo_core::application::ports::LibraryRepository;
use echo_core::domain::entities::LibraryRoot;
use echo_core::domain::sync::{AuthorityId, Epoch, LibraryId, SyncBinding, SyncSessionIntent};

struct IntentProvisioner {
    database: Arc<SqliteDatabase>,
    binding: SyncBinding,
    starts: AtomicUsize,
    fail_on_start: bool,
}

impl SyncProvisioner for IntentProvisioner {
    fn host(&self, _: LibraryRootId) -> Result<ProvisionedHost, SyncRuntimeError> {
        self.starts.fetch_add(1, Ordering::AcqRel);
        Err(SyncRuntimeError::Unconfigured)
    }

    fn peer(&self, root: LibraryRootId) -> Result<ProvisionedPeer, SyncRuntimeError> {
        self.starts.fetch_add(1, Ordering::AcqRel);
        if self.fail_on_start {
            return Err(SyncRuntimeError::Unconfigured);
        }
        self.database
            .bind_sync_library(self.binding, false, root)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        Ok(ProvisionedPeer {
            binding: self.binding,
            address: "https://127.0.0.1:50826".to_owned(),
            host_device_id: echo_core::domain::sync::DeviceId::new(),
        })
    }

    fn revoke_device(&self, _: &str) -> Result<(), SyncRuntimeError> {
        Ok(())
    }

    fn leave_sync(&self, root: LibraryRootId) -> Result<(), SyncRuntimeError> {
        let intent = self
            .database
            .current_sync_configuration()
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
            .map(|configuration| configuration.session_intent);
        if intent != Some(SyncSessionIntent::Stopped) {
            return Err(SyncRuntimeError::LibraryUnavailable);
        }
        self.database
            .detach_lan_sync_context(root)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)
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

/// Mark the runtime as an active peer session without touching the network.
/// `pause()` and `resume()` only act on a live session, so a test that races
/// `pause()` against a restore worker needs this exact precondition.
fn mark_started_for_test(runtime: &SyncRuntime, database: &SqliteDatabase) {
    let root = database
        .current_sync_configuration()
        .unwrap()
        .map(|configuration| configuration.local_root);
    let mut inner = runtime.inner.lock().unwrap();
    inner.status.enabled = true;
    inner.status.role = SyncRole::Peer;
    inner.status.state = SyncState::Connected;
    inner.root = root;
}

fn runtime_fixture() -> (
    tempfile::TempDir,
    Arc<SqliteDatabase>,
    Arc<IntentProvisioner>,
) {
    let directory = tempfile::tempdir().unwrap();
    let database = Arc::new(SqliteDatabase::open(directory.path().join("echo.sqlite")).unwrap());
    let root = LibraryRootId::new();
    LibraryRepository::upsert(
        database.as_ref(),
        &LibraryRoot::new(root, directory.path().join("library"), true, true),
    )
    .unwrap();
    let binding = SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    database.bind_sync_library(binding, false, root).unwrap();
    let provisioner = Arc::new(IntentProvisioner {
        database: Arc::clone(&database),
        binding,
        starts: AtomicUsize::new(0),
        fail_on_start: true,
    });

    (directory, database, provisioner)
}

#[test]
fn peer_start_persists_enabled_intent_before_network_provisioning() {
    let (_directory, database, provisioner) = runtime_fixture();
    let runtime = SyncRuntime::new(Arc::clone(&database), Some(provisioner.clone()), None);

    assert!(matches!(
        runtime.peer_start(),
        Err(SyncRuntimeError::Unconfigured)
    ));
    assert_eq!(provisioner.starts.load(Ordering::Acquire), 1);
    assert_eq!(
        database
            .current_sync_configuration()
            .unwrap()
            .unwrap()
            .session_intent,
        SyncSessionIntent::Enabled
    );
}

#[test]
fn failed_enabled_intent_write_prevents_network_provisioning() {
    let (_directory, database, provisioner) = runtime_fixture();
    let connection = rusqlite::Connection::open(database.path()).unwrap();
    connection
        .execute_batch(
            "CREATE TRIGGER reject_enabled_intent BEFORE UPDATE OF session_intent ON sync_binding WHEN NEW.session_intent='enabled' BEGIN SELECT RAISE(ABORT, 'intent write rejected'); END;",
        )
        .unwrap();
    let runtime = SyncRuntime::new(Arc::clone(&database), Some(provisioner.clone()), None);

    assert!(matches!(
        runtime.peer_start(),
        Err(SyncRuntimeError::LibraryUnavailable)
    ));
    assert_eq!(provisioner.starts.load(Ordering::Acquire), 0);
}

#[test]
fn host_start_persists_enabled_intent_before_provisioning() {
    let (_directory, database, provisioner) = runtime_fixture();
    let runtime = SyncRuntime::new(Arc::clone(&database), Some(provisioner.clone()), None);

    assert!(matches!(
        runtime.host_start("192.168.1.5"),
        Err(SyncRuntimeError::Unconfigured)
    ));
    assert_eq!(provisioner.starts.load(Ordering::Acquire), 1);
    assert_eq!(
        database
            .current_sync_configuration()
            .unwrap()
            .unwrap()
            .session_intent,
        SyncSessionIntent::Enabled
    );
}

#[test]
fn explicit_stop_persists_stopped_intent() {
    let (_directory, database, _provisioner) = runtime_fixture();
    database
        .set_sync_session_intent(SyncSessionIntent::Enabled)
        .unwrap();
    let runtime = SyncRuntime::new(Arc::clone(&database), None, None);

    runtime.stop().unwrap();

    assert_eq!(
        database
            .current_sync_configuration()
            .unwrap()
            .unwrap()
            .session_intent,
        SyncSessionIntent::Stopped
    );
}

#[test]
fn leave_persists_stopped_intent() {
    let (_directory, database, provisioner) = runtime_fixture();
    database
        .set_sync_session_intent(SyncSessionIntent::Enabled)
        .unwrap();
    let runtime = SyncRuntime::new(Arc::clone(&database), Some(provisioner), None);

    runtime.leave_sync().unwrap();

    assert!(database.current_sync_configuration().unwrap().is_none());
}

#[test]
fn pause_persists_paused_intent() {
    let (_directory, database, _provisioner) = runtime_fixture();
    database
        .set_sync_session_intent(SyncSessionIntent::Enabled)
        .unwrap();
    let runtime = SyncRuntime::new(Arc::clone(&database), None, None);
    {
        let mut inner = runtime.inner.lock().unwrap();
        inner.status.enabled = true;
        inner.status.role = SyncRole::Peer;
        inner.root = database
            .current_sync_configuration()
            .unwrap()
            .map(|configuration| configuration.local_root);
    }

    runtime.pause().unwrap();

    assert_eq!(
        database
            .current_sync_configuration()
            .unwrap()
            .unwrap()
            .session_intent,
        SyncSessionIntent::Paused
    );
}

#[test]
fn stopping_restore_retry_wakes_worker_without_another_provision_attempt() {
    let (_directory, database, provisioner) = runtime_fixture();
    database
        .set_sync_session_intent(SyncSessionIntent::Enabled)
        .unwrap();
    let runtime = Arc::new(SyncRuntime::new(
        Arc::clone(&database),
        Some(provisioner.clone()),
        None,
    ));
    runtime
        .start_restore_retry_with_delays(5, Duration::from_millis(5), Duration::from_millis(30))
        .unwrap();
    let deadline = std::time::Instant::now() + Duration::from_secs(1);
    while provisioner.starts.load(Ordering::Acquire) == 0 {
        assert!(
            std::time::Instant::now() < deadline,
            "retry attempt did not start"
        );
        std::thread::yield_now();
    }

    runtime.stop().unwrap();
    let attempts_after_stop = provisioner.starts.load(Ordering::Acquire);
    std::thread::sleep(Duration::from_millis(60));

    assert_eq!(
        provisioner.starts.load(Ordering::Acquire),
        attempts_after_stop
    );
}

#[test]
fn restore_does_not_provision_without_enabled_intent() {
    for intent in [SyncSessionIntent::Stopped, SyncSessionIntent::Paused] {
        let (_directory, database, provisioner) = runtime_fixture();
        database.set_sync_session_intent(intent).unwrap();
        let runtime = SyncRuntime::new(Arc::clone(&database), Some(provisioner.clone()), None);

        assert!(runtime.restore_saved_session().unwrap().is_none());
        assert_eq!(provisioner.starts.load(Ordering::Acquire), 0);
    }
}

#[test]
fn restore_does_not_provision_without_a_binding() {
    let directory = tempfile::tempdir().unwrap();
    let database = Arc::new(SqliteDatabase::open(directory.path().join("echo.sqlite")).unwrap());
    let root = LibraryRootId::new();
    LibraryRepository::upsert(
        database.as_ref(),
        &LibraryRoot::new(root, directory.path().join("library"), true, true),
    )
    .unwrap();
    let binding = SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    let provisioner = Arc::new(IntentProvisioner {
        database: Arc::clone(&database),
        binding,
        starts: AtomicUsize::new(0),
        fail_on_start: true,
    });

    let runtime = SyncRuntime::new(database, Some(provisioner.clone()), None);

    assert!(runtime.restore_saved_session().unwrap().is_none());
    assert_eq!(provisioner.starts.load(Ordering::Acquire), 0);
}

#[test]
fn process_drop_preserves_enabled_intent() {
    let (_directory, database, _provisioner) = runtime_fixture();
    database
        .set_sync_session_intent(SyncSessionIntent::Enabled)
        .unwrap();

    drop(SyncRuntime::new(Arc::clone(&database), None, None));

    assert_eq!(
        database
            .current_sync_configuration()
            .unwrap()
            .unwrap()
            .session_intent,
        SyncSessionIntent::Enabled
    );
}

/// Wait until `condition` holds, bounded by a generous deadline. Retry workers
/// run on their own thread, so tests spin on an observable side effect rather
/// than sleeping for a fixed duration.
fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while !condition() {
        assert!(
            std::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn retry_exhaustion_becomes_a_visible_terminal_state_without_further_attempts() {
    let (_directory, database, provisioner) = runtime_fixture();
    database
        .set_sync_session_intent(SyncSessionIntent::Enabled)
        .unwrap();
    let runtime = Arc::new(SyncRuntime::new(
        Arc::clone(&database),
        Some(provisioner.clone()),
        None,
    ));
    runtime
        .start_restore_retry_with_delays(2, Duration::from_millis(1), Duration::from_millis(2))
        .unwrap();

    wait_until("bounded retry exhaustion", || {
        runtime.status().restore_exhausted
    });
    let attempts_at_exhaustion = provisioner.starts.load(Ordering::Acquire);
    assert_eq!(
        attempts_at_exhaustion, 2,
        "restoration must stop at the bounded attempt count"
    );
    let status = runtime.status();
    assert_eq!(status.state, SyncState::Failed);
    assert_eq!(status.session_intent, IpcSyncSessionIntent::Enabled);
    assert!(status.restore_attempts.is_none());
    assert!(!status.enabled);

    // Exhaustion is terminal: no further automatic provisioning happens until
    // the user asks again.
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(
        provisioner.starts.load(Ordering::Acquire),
        attempts_at_exhaustion
    );
}

#[test]
fn explicit_restore_retry_clears_exhaustion_and_restarts_the_bounded_worker() {
    let (_directory, database, provisioner) = runtime_fixture();
    database
        .set_sync_session_intent(SyncSessionIntent::Enabled)
        .unwrap();
    let runtime = Arc::new(SyncRuntime::new(
        Arc::clone(&database),
        Some(provisioner.clone()),
        None,
    ));
    runtime
        .start_restore_retry_with_delays(1, Duration::from_millis(1), Duration::from_millis(2))
        .unwrap();
    wait_until("first exhaustion", || runtime.status().restore_exhausted);

    let cleared = runtime.retry_restore().unwrap();
    assert!(!cleared.restore_exhausted);
    assert_eq!(cleared.state, SyncState::Disabled);
    assert!(cleared.last_error.is_none());

    let before = provisioner.starts.load(Ordering::Acquire);
    runtime
        .start_restore_retry_with_delays(1, Duration::from_millis(1), Duration::from_millis(2))
        .unwrap();
    wait_until("a fresh attempt after the user retry", || {
        provisioner.starts.load(Ordering::Acquire) > before
    });
    runtime.cancel_restore_retry();
}

#[test]
fn pausing_during_backoff_interrupts_the_wait_and_never_provisions_again() {
    let (_directory, database, provisioner) = runtime_fixture();
    database
        .set_sync_session_intent(SyncSessionIntent::Enabled)
        .unwrap();
    {
        // A live peer session is what `pause` requires; the retry worker is
        // independent of it, exactly like startup restoration racing a user.
        let runtime = Arc::new(SyncRuntime::new(
            Arc::clone(&database),
            Some(provisioner.clone()),
            None,
        ));
        mark_started_for_test(&runtime, &database);
        // A long backoff proves the wake-up, not the delay, ends the wait.
        runtime
            .start_restore_retry_with_delays(5, Duration::from_secs(30), Duration::from_secs(30))
            .unwrap();
        std::thread::sleep(Duration::from_millis(10));

        runtime.pause().unwrap();

        assert_eq!(
            database
                .current_sync_configuration()
                .unwrap()
                .unwrap()
                .session_intent,
            SyncSessionIntent::Paused
        );
        // pause() joins the worker, so returning proves the wait was
        // interrupted instead of sleeping out the 30 s backoff.
        assert_eq!(provisioner.starts.load(Ordering::Acquire), 0);
    }
}

#[test]
fn leave_during_backoff_cancels_restoration_and_clears_the_binding() {
    let (_directory, database, provisioner) = runtime_fixture();
    database
        .set_sync_session_intent(SyncSessionIntent::Enabled)
        .unwrap();
    let runtime = Arc::new(SyncRuntime::new(
        Arc::clone(&database),
        Some(provisioner.clone()),
        None,
    ));
    runtime
        .start_restore_retry_with_delays(5, Duration::from_secs(30), Duration::from_secs(30))
        .unwrap();
    std::thread::sleep(Duration::from_millis(10));

    runtime.leave_sync().unwrap();

    assert!(database.current_sync_configuration().unwrap().is_none());
    assert_eq!(provisioner.starts.load(Ordering::Acquire), 0);
}

#[test]
fn switching_the_active_root_supersedes_a_pending_restore() {
    let (_directory, database, provisioner) = runtime_fixture();
    database
        .set_sync_session_intent(SyncSessionIntent::Enabled)
        .unwrap();
    // An inactive candidate: preparing it is what a real root switch does
    // before the activation boundary commits.
    let other_root = LibraryRootId::new();
    LibraryRepository::upsert(
        database.as_ref(),
        &LibraryRoot::new(
            other_root,
            std::path::PathBuf::from("/tmp/other"),
            false,
            true,
        ),
    )
    .unwrap();
    let runtime = Arc::new(SyncRuntime::new(
        Arc::clone(&database),
        Some(provisioner.clone()),
        None,
    ));
    runtime
        .start_restore_retry_with_delays(100, Duration::from_millis(5), Duration::from_millis(10))
        .unwrap();
    wait_until("a first bounded attempt", || {
        provisioner.starts.load(Ordering::Acquire) > 0
    });

    // Activating the other root is the handoff boundary: the old library's
    // worker must stop on its own instead of provisioning it again.
    LibraryRepository::upsert(
        database.as_ref(),
        &LibraryRoot::new(
            other_root,
            std::path::PathBuf::from("/tmp/other"),
            true,
            true,
        ),
    )
    .unwrap();
    let attempts_after_switch = provisioner.starts.load(Ordering::Acquire);
    std::thread::sleep(Duration::from_millis(120));

    assert_eq!(
        provisioner.starts.load(Ordering::Acquire),
        attempts_after_switch,
        "a superseded restore must not provision the old root"
    );
    assert_eq!(
        database
            .current_sync_configuration()
            .unwrap()
            .map(|configuration| configuration.local_root),
        None,
        "the new root has no binding of its own"
    );
}

/// Minimal cycle port so a successful restore can start its loop. It never
/// performs network work and parks until cancelled.
struct ParkedCyclePort;

impl SyncCyclePort for ParkedCyclePort {
    fn sync_now(&self, _: SyncRole) -> Result<bool, SyncRuntimeError> {
        Ok(false)
    }
    fn sync_now_with_progress(
        &self,
        _: SyncRole,
        cancelled: Arc<AtomicBool>,
        _: Option<ProgressNotifier>,
    ) -> Result<bool, SyncRuntimeError> {
        let mut waited = 0_u32;
        while !cancelled.load(Ordering::Acquire) && waited < 10_000 {
            std::thread::sleep(Duration::from_millis(1));
            waited += 1;
        }
        if cancelled.load(Ordering::Acquire) {
            return Err(SyncRuntimeError::Busy);
        }
        Ok(false)
    }
    fn watch_host_commits(
        &self,
        cancelled: Arc<AtomicBool>,
        _: Duration,
    ) -> Result<WatchOutcome, SyncRuntimeError> {
        let mut waited = 0_u32;
        while !cancelled.load(Ordering::Acquire) && waited < 10_000 {
            std::thread::sleep(Duration::from_millis(1));
            waited += 1;
        }
        Ok(WatchOutcome::BudgetElapsed)
    }
}

#[test]
fn a_successful_restore_clears_the_attempt_accounting() {
    let directory = tempfile::tempdir().unwrap();
    let database = Arc::new(SqliteDatabase::open(directory.path().join("echo.sqlite")).unwrap());
    let root = LibraryRootId::new();
    LibraryRepository::upsert(
        database.as_ref(),
        &LibraryRoot::new(root, directory.path().join("library"), true, true),
    )
    .unwrap();
    let binding = SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    database.bind_sync_library(binding, false, root).unwrap();
    database
        .set_sync_session_intent(SyncSessionIntent::Enabled)
        .unwrap();
    // `fail_on_start: false` lets the peer provisioning succeed, which is what
    // ends the retry series.
    let provisioner = Arc::new(IntentProvisioner {
        database: Arc::clone(&database),
        binding,
        starts: AtomicUsize::new(0),
        fail_on_start: false,
    });
    let runtime = Arc::new(SyncRuntime::new(
        Arc::clone(&database),
        Some(provisioner),
        Some(Arc::new(ParkedCyclePort)),
    ));

    runtime
        .start_restore_retry_with_delays(3, Duration::from_millis(1), Duration::from_millis(2))
        .unwrap();
    wait_until("a successful session", || runtime.status().enabled);

    let status = runtime.status();
    assert!(status.restore_attempts.is_none());
    assert!(!status.restore_exhausted);
    assert_eq!(status.session_intent, IpcSyncSessionIntent::Enabled);
    runtime.stop().unwrap();
}
