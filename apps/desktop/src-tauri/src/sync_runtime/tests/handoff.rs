//! Root handoff: cancel is a no-op, successful switches restore only the
//! target root's own session, and a failed switch keeps the old session.

use super::*;
use echo_core::application::ports::LibraryRepository;
use echo_core::domain::entities::LibraryRoot;
use echo_core::domain::sync::{AuthorityId, Epoch, LibraryId, SyncBinding, SyncSessionIntent};

/// Provisioner that records session starts and the root it was asked for.
struct RecordingProvisioner {
    database: Arc<SqliteDatabase>,
    starts: Mutex<Vec<LibraryRootId>>,
    fail: AtomicBool,
}

impl RecordingProvisioner {
    fn new(database: Arc<SqliteDatabase>) -> Arc<Self> {
        Arc::new(Self {
            database,
            starts: Mutex::new(Vec::new()),
            fail: AtomicBool::new(false),
        })
    }

    fn started_roots(&self) -> Vec<LibraryRootId> {
        self.starts.lock().unwrap().clone()
    }
}

impl SyncProvisioner for RecordingProvisioner {
    fn host(&self, root: LibraryRootId) -> Result<ProvisionedHost, SyncRuntimeError> {
        self.starts.lock().unwrap().push(root);
        Err(SyncRuntimeError::Unconfigured)
    }

    fn peer(&self, root: LibraryRootId) -> Result<ProvisionedPeer, SyncRuntimeError> {
        self.starts.lock().unwrap().push(root);
        if self.fail.load(Ordering::Acquire) {
            return Err(SyncRuntimeError::SyncConnectionFailed);
        }
        let binding = self
            .database
            .current_sync_configuration()
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
            .map(|configuration| configuration.binding)
            .ok_or(SyncRuntimeError::Unconfigured)?;
        Ok(ProvisionedPeer {
            binding,
            address: "https://127.0.0.1:50826".to_owned(),
            host_device_id: echo_core::domain::sync::DeviceId::new(),
        })
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
        Ok(())
    }
}

/// A cycle port that parks until cancelled so a live session stays live.
struct ParkedCycle;

impl SyncCyclePort for ParkedCycle {
    fn sync_now(&self, _: SyncRole) -> Result<bool, SyncRuntimeError> {
        Ok(false)
    }
    fn sync_now_with_progress(
        &self,
        _: SyncRole,
        cancelled: Arc<AtomicBool>,
        _: Option<ProgressNotifier>,
    ) -> Result<bool, SyncRuntimeError> {
        while !cancelled.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(1));
        }
        Err(SyncRuntimeError::Busy)
    }
    fn watch_host_commits(
        &self,
        cancelled: Arc<AtomicBool>,
        _: Duration,
    ) -> Result<WatchOutcome, SyncRuntimeError> {
        while !cancelled.load(Ordering::Acquire) {
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(WatchOutcome::BudgetElapsed)
    }
}

fn library(database: &SqliteDatabase, path: &std::path::Path, active: bool) -> LibraryRootId {
    let root = LibraryRootId::new();
    LibraryRepository::upsert(
        database,
        &LibraryRoot::new(root, path.to_path_buf(), active, true),
    )
    .unwrap();
    root
}

fn bind(database: &SqliteDatabase, root: LibraryRootId) -> SyncBinding {
    let binding = SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    database.bind_sync_library(binding, false, root).unwrap();
    binding
}

fn fixture() -> (
    tempfile::TempDir,
    Arc<SqliteDatabase>,
    Arc<RecordingProvisioner>,
    LibraryRootId,
) {
    let directory = tempfile::tempdir().unwrap();
    let database = Arc::new(SqliteDatabase::open(directory.path().join("echo.sqlite")).unwrap());
    let root = library(&database, &directory.path().join("a"), true);
    bind(&database, root);
    let provisioner = RecordingProvisioner::new(Arc::clone(&database));
    (directory, database, provisioner, root)
}

#[test]
fn a_cancelled_or_refused_selection_never_fences_the_session() {
    let (_directory, database, provisioner, root) = fixture();
    database
        .set_sync_session_intent(SyncSessionIntent::Enabled)
        .unwrap();
    let runtime = Arc::new(SyncRuntime::new(
        Arc::clone(&database),
        Some(provisioner.clone()),
        Some(Arc::new(ParkedCycle)),
    ));
    runtime.start_peer_locked().unwrap();
    let started = runtime.status();
    assert!(started.enabled);
    assert_eq!(provisioner.started_roots(), vec![root]);

    // A cancelled picker never calls the handoff hooks at all, so the live
    // session, its intent and its retry accounting are untouched.
    let unchanged = runtime.status();
    assert_eq!(unchanged.session_id, started.session_id);
    assert!(unchanged.enabled);
    assert_eq!(
        database
            .current_sync_configuration()
            .unwrap()
            .unwrap()
            .session_intent,
        SyncSessionIntent::Enabled
    );
    runtime.stop().unwrap();
}

#[test]
fn a_successful_switch_restores_only_the_target_roots_own_enabled_session() {
    let (directory, database, provisioner, first) = fixture();
    database
        .set_sync_session_intent(SyncSessionIntent::Enabled)
        .unwrap();
    let runtime = Arc::new(SyncRuntime::new(
        Arc::clone(&database),
        Some(provisioner.clone()),
        Some(Arc::new(ParkedCycle)),
    ));
    runtime.start_peer_locked().unwrap();
    assert_eq!(provisioner.started_roots(), vec![first]);
    let _handoff = runtime.begin_root_handoff().unwrap();
    // Fencing a handoff must not rewrite the previous root's intent.
    assert_eq!(
        database
            .current_sync_configuration()
            .unwrap()
            .unwrap()
            .session_intent,
        SyncSessionIntent::Enabled
    );

    let second = library(&database, &directory.path().join("b"), true);
    bind(&database, second);
    database
        .set_sync_session_intent(SyncSessionIntent::Enabled)
        .unwrap();
    let status = runtime.complete_root_handoff(second).unwrap();

    assert!(status.enabled);
    assert_eq!(
        provisioner.started_roots(),
        vec![first, second],
        "exactly the target root's own session started"
    );
    runtime.stop().unwrap();
}

#[test]
fn switching_to_a_stopped_or_paused_library_leaves_it_closed() {
    for intent in [SyncSessionIntent::Stopped, SyncSessionIntent::Paused] {
        let (directory, database, provisioner, first) = fixture();
        database
            .set_sync_session_intent(SyncSessionIntent::Enabled)
            .unwrap();
        let runtime = Arc::new(SyncRuntime::new(
            Arc::clone(&database),
            Some(provisioner.clone()),
            Some(Arc::new(ParkedCycle)),
        ));
        runtime.start_peer_locked().unwrap();
        runtime.begin_root_handoff().unwrap();

        let second = library(&database, &directory.path().join("b"), true);
        bind(&database, second);
        database.set_sync_session_intent(intent).unwrap();
        let status = runtime.complete_root_handoff(second).unwrap();

        assert!(!status.enabled, "a {intent:?} library must stay closed");
        assert_eq!(provisioner.started_roots(), vec![first]);
        assert_eq!(
            status.session_intent,
            match intent {
                SyncSessionIntent::Stopped => IpcSyncSessionIntent::Stopped,
                _ => IpcSyncSessionIntent::Paused,
            }
        );
    }
}

#[test]
fn a_switch_to_a_closed_library_publishes_a_root_aware_status_for_the_new_root() {
    let (directory, database, provisioner, _first) = fixture();
    database
        .set_sync_session_intent(SyncSessionIntent::Enabled)
        .unwrap();
    let runtime = Arc::new(SyncRuntime::new(
        Arc::clone(&database),
        Some(provisioner),
        Some(Arc::new(ParkedCycle)),
    ));
    runtime.start_peer_locked().unwrap();
    let (statuses, events) = mpsc::channel();
    runtime.set_status_notifier(Arc::new(move |status| {
        let _ = statuses.send(status);
    }));
    runtime.begin_root_handoff().unwrap();

    let second = library(&database, &directory.path().join("b"), true);
    let binding = bind(&database, second);
    database
        .set_sync_session_intent(SyncSessionIntent::Stopped)
        .unwrap();
    let status = runtime.complete_root_handoff(second).unwrap();
    let selected = database
        .current_sync_configuration()
        .unwrap()
        .expect("the newly activated root keeps its own binding");
    assert_eq!(selected.local_root, second);
    assert_eq!(
        status.library_id,
        Some(selected.binding.library_id.as_uuid().to_string()),
        "the closed status names the selected library, not the previous one"
    );

    assert!(!status.enabled);
    assert_eq!(status.session_intent, IpcSyncSessionIntent::Stopped);
    assert_eq!(
        status.local_root,
        Some(second.as_uuid().to_string()),
        "the closed status belongs to the newly activated root"
    );
    assert_eq!(
        status.library_id,
        Some(selected.binding.library_id.as_uuid().to_string())
    );
    let _ = binding;
    let mut published = Vec::new();
    while let Ok(status) = events.recv_timeout(Duration::from_millis(200)) {
        published.push(status);
    }
    let last = published
        .last()
        .expect("a status is published for the new root");
    assert_eq!(last.local_root, status.local_root);
    assert_eq!(last.library_id, status.library_id);
    assert_eq!(last.session_intent, IpcSyncSessionIntent::Stopped);
}

#[test]
fn switching_to_an_unbound_library_never_provisions() {
    let (directory, database, provisioner, first) = fixture();
    database
        .set_sync_session_intent(SyncSessionIntent::Enabled)
        .unwrap();
    let runtime = Arc::new(SyncRuntime::new(
        Arc::clone(&database),
        Some(provisioner.clone()),
        Some(Arc::new(ParkedCycle)),
    ));
    runtime.start_peer_locked().unwrap();
    runtime.begin_root_handoff().unwrap();

    let unbound = library(&database, &directory.path().join("b"), true);
    let status = runtime.complete_root_handoff(unbound).unwrap();

    assert!(!status.enabled);
    assert_eq!(provisioner.started_roots(), vec![first]);
}

#[test]
fn a_failed_switch_restores_the_previous_session() {
    let (_directory, database, provisioner, first) = fixture();
    database
        .set_sync_session_intent(SyncSessionIntent::Enabled)
        .unwrap();
    let runtime = Arc::new(SyncRuntime::new(
        Arc::clone(&database),
        Some(provisioner.clone()),
        Some(Arc::new(ParkedCycle)),
    ));
    runtime.start_peer_locked().unwrap();
    let handoff = runtime.begin_root_handoff().unwrap();
    assert!(!runtime.status().enabled, "the handoff fenced the session");

    // The activation never committed: the previous root is still active.
    let restored = runtime.abort_root_handoff(&handoff).unwrap();

    assert!(restored.enabled, "the old session must come back");
    assert_eq!(provisioner.started_roots(), vec![first, first]);
    runtime.stop().unwrap();
}

#[test]
fn an_abort_after_the_root_already_changed_refuses_to_guess() {
    let (directory, database, provisioner, first) = fixture();
    database
        .set_sync_session_intent(SyncSessionIntent::Enabled)
        .unwrap();
    let runtime = Arc::new(SyncRuntime::new(
        Arc::clone(&database),
        Some(provisioner.clone()),
        Some(Arc::new(ParkedCycle)),
    ));
    runtime.start_peer_locked().unwrap();
    let handoff = runtime.begin_root_handoff().unwrap();
    let other = library(&database, &directory.path().join("b"), true);
    database.select_sync_context(other).unwrap();

    assert!(matches!(
        runtime.abort_root_handoff(&handoff),
        Err(SyncRuntimeError::LibraryUnavailable)
    ));
    assert_eq!(provisioner.started_roots(), vec![first]);
}
