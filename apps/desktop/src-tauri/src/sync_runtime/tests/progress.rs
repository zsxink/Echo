//! Progress must be visible while the scheduler is still in its cycle.

use super::*;

struct ProgressProvisioner {
    started: mpsc::Sender<ProgressNotifier>,
    release: Mutex<mpsc::Receiver<()>>,
}

impl SyncProvisioner for ProgressProvisioner {
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

impl SyncCyclePort for ProgressProvisioner {
    fn sync_now(&self, _: SyncRole) -> Result<bool, SyncRuntimeError> {
        Err(SyncRuntimeError::Unconfigured)
    }
    fn sync_now_with_progress(
        &self,
        _: SyncRole,
        cancelled: Arc<AtomicBool>,
        progress: Option<ProgressNotifier>,
    ) -> Result<bool, SyncRuntimeError> {
        let progress = progress.unwrap();
        progress(1, partial(4));
        self.started.send(progress).unwrap();
        loop {
            if cancelled.load(Ordering::Acquire) {
                return Err(SyncRuntimeError::Busy);
            }
            if self
                .release
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_millis(10))
                .is_ok()
            {
                return Ok(false);
            }
        }
    }
    fn sync_progress(&self, _: SyncRole) -> Result<SyncProgress, SyncRuntimeError> {
        Ok(SyncProgress {
            pending_changes: Some(0),
            pending_uploads: Some(0),
            pending_downloads: Some(0),
            uploaded_bytes: Some(10),
            total_upload_bytes: Some(10),
            downloaded_bytes: Some(10),
            total_download_bytes: Some(10),
            media_ready: Some(true),
        })
    }
}

fn partial(bytes: u64) -> SyncProgress {
    SyncProgress {
        uploaded_bytes: Some(bytes),
        total_upload_bytes: Some(10),
        downloaded_bytes: Some(bytes),
        total_download_bytes: Some(10),
        ..SyncProgress::default()
    }
}

fn next_matching(
    events: &mpsc::Receiver<SyncStatus>,
    matches: impl Fn(&SyncStatus) -> bool,
) -> SyncStatus {
    loop {
        let status = events.recv_timeout(Duration::from_secs(2)).unwrap();
        if matches(&status) {
            return status;
        }
    }
}

#[test]
fn live_checkpoints_notify_before_cycle_completion_and_final_refresh() {
    let directory = tempfile::tempdir().unwrap();
    let database = Arc::new(SqliteDatabase::open(directory.path().join("echo.sqlite")).unwrap());
    let (started, cycles) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let provisioner = Arc::new(ProgressProvisioner {
        started,
        release: Mutex::new(gate),
    });
    let runtime = SyncRuntime::new(database, Some(provisioner));
    {
        let mut inner = runtime.inner.lock().unwrap();
        inner.status.enabled = true;
        inner.status.role = SyncRole::Host;
        inner.status.state = SyncState::Hosting;
        inner.status.session_id = 7;
    }
    let (statuses, events) = mpsc::channel();
    runtime.set_status_notifier(Arc::new(move |status| {
        statuses.send(status).unwrap();
    }));
    runtime.start_sync_loop(SyncRole::Host).unwrap();
    let checkpoint = cycles.recv_timeout(Duration::from_secs(2)).unwrap();
    let live = next_matching(&events, |status| status.uploaded_bytes == Some(4));
    assert_eq!(live.state, SyncState::Syncing);
    assert_eq!(live.downloaded_bytes, Some(4));
    assert_eq!(live.media_ready, Some(false));
    assert_eq!(runtime.status().uploaded_bytes, Some(4));
    checkpoint(3, partial(8));
    checkpoint(2, partial(5));
    assert_eq!(runtime.status().uploaded_bytes, Some(8));
    release.send(()).unwrap();
    let finished = next_matching(&events, |status| status.state == SyncState::Hosting);
    assert_eq!(finished.uploaded_bytes, Some(10));
    assert_eq!(finished.pending_changes, Some(0));
    assert_eq!(finished.pending_uploads, Some(0));
    assert_eq!(finished.pending_downloads, Some(0));
    assert_eq!(finished.media_ready, Some(true));
    checkpoint(4, partial(1));
    assert_eq!(runtime.status().uploaded_bytes, Some(10));
    runtime.stop().unwrap();
}

#[test]
fn paused_cycle_rejects_late_checkpoint_after_resume_in_same_session() {
    let directory = tempfile::tempdir().unwrap();
    let database = Arc::new(SqliteDatabase::open(directory.path().join("echo.sqlite")).unwrap());
    let (started, cycles) = mpsc::channel();
    let (_release, gate) = mpsc::channel();
    let provisioner = Arc::new(ProgressProvisioner {
        started,
        release: Mutex::new(gate),
    });
    let runtime = SyncRuntime::new(database, Some(provisioner));
    {
        let mut inner = runtime.inner.lock().unwrap();
        inner.status.enabled = true;
        inner.status.role = SyncRole::Host;
        inner.status.state = SyncState::Hosting;
        inner.status.session_id = 7;
    }
    runtime.start_sync_loop(SyncRole::Host).unwrap();
    let old = cycles.recv_timeout(Duration::from_secs(2)).unwrap();
    runtime.pause().unwrap();
    old(2, partial(9));
    assert_eq!(runtime.status().state, SyncState::Paused);
    assert_eq!(runtime.status().uploaded_bytes, Some(4));
    // Deliberately reuse the session to prove cycle fencing as well as the
    // session check. Production resume also advances the session ID.
    runtime.change_state(SyncState::Hosting, None);
    runtime.start_sync_loop(SyncRole::Host).unwrap();
    let current = cycles.recv_timeout(Duration::from_secs(2)).unwrap();
    current(2, partial(6));
    let sequence = runtime.status().event_sequence;
    old(3, partial(9));
    assert_eq!(runtime.status().uploaded_bytes, Some(6));
    assert_eq!(runtime.status().event_sequence, sequence);
    runtime.stop().unwrap();
    current(3, partial(10));
    assert_eq!(runtime.status().state, SyncState::Disabled);
    assert_eq!(runtime.status().uploaded_bytes, None);
}

#[test]
fn progress_observer_rejects_changed_session_and_cancellation() {
    let state = Arc::new(Mutex::new(Inner::default()));
    {
        let mut inner = state.lock().unwrap();
        inner.status.enabled = true;
        inner.status.state = SyncState::Syncing;
        inner.status.session_id = 7;
    }
    let cancelled = Arc::new(AtomicBool::new(false));
    let observer = CycleProgressObserver::new(
        Arc::clone(&state),
        Arc::new(Mutex::new(None)),
        7,
        Arc::clone(&cancelled),
    );
    (observer.notify)(1, partial(4));
    state.lock().unwrap().status.session_id = 8;
    (observer.notify)(2, partial(6));
    assert_eq!(state.lock().unwrap().status.uploaded_bytes, Some(4));
    state.lock().unwrap().status.session_id = 7;
    cancelled.store(true, Ordering::Release);
    (observer.notify)(3, partial(8));
    assert_eq!(state.lock().unwrap().status.uploaded_bytes, Some(4));
}

#[test]
fn status_follows_local_root_and_clears_it_when_stopped() {
    use echo_core::domain::sync::{AuthorityId, Epoch, LibraryId};

    let directory = tempfile::tempdir().unwrap();
    let database = Arc::new(SqliteDatabase::open(directory.path().join("echo.sqlite")).unwrap());
    let runtime = SyncRuntime::new(database, None);
    let binding = SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    assert_eq!(runtime.status().local_root, None);
    let first_root = LibraryRootId::new();
    runtime.set_started(
        first_root,
        binding,
        SyncRole::Peer,
        "https://10.0.0.2".into(),
    );
    let first_session = runtime.status().session_id;
    assert_eq!(
        runtime.status().local_root,
        Some(first_root.as_uuid().to_string())
    );
    runtime.stop().unwrap();
    assert_eq!(runtime.status().local_root, None);
    let second_root = LibraryRootId::new();
    runtime.set_started(
        second_root,
        binding,
        SyncRole::Peer,
        "https://10.0.0.2".into(),
    );
    assert_eq!(
        runtime.status().local_root,
        Some(second_root.as_uuid().to_string())
    );
    assert!(runtime.status().session_id > first_session);
    runtime.stop().unwrap();
    assert_eq!(runtime.status().local_root, None);
}

#[test]
fn manual_cycle_publishes_checkpoints_and_pause_cancels_its_observer() {
    use echo_core::domain::entities::LibraryRoot;
    use echo_core::domain::sync::{AuthorityId, Epoch, LibraryId};

    let directory = tempfile::tempdir().unwrap();
    let database = Arc::new(SqliteDatabase::open(directory.path().join("echo.sqlite")).unwrap());
    let root = LibraryRootId::new();
    LibraryRepository::upsert(
        database.as_ref(),
        &LibraryRoot::new(root, directory.path().join("library"), true, true),
    )
    .unwrap();
    let (started, cycles) = mpsc::channel();
    let (_release, gate) = mpsc::channel();
    let provisioner = Arc::new(ProgressProvisioner {
        started,
        release: Mutex::new(gate),
    });
    let runtime = Arc::new(SyncRuntime::new(database, Some(provisioner)));
    runtime.set_started(
        root,
        SyncBinding {
            library_id: LibraryId::new(),
            authority_id: AuthorityId::new(),
            epoch: Epoch::INITIAL,
        },
        SyncRole::Peer,
        "https://10.0.0.2".into(),
    );
    let (statuses, events) = mpsc::channel();
    runtime.set_status_notifier(Arc::new(move |status| statuses.send(status).unwrap()));
    let manual = Arc::clone(&runtime);
    let worker = thread::spawn(move || manual.sync_now());
    let checkpoint = cycles.recv_timeout(Duration::from_secs(2)).unwrap();
    let live = next_matching(&events, |status| status.uploaded_bytes == Some(4));
    assert_eq!(live.state, SyncState::Syncing);
    assert_eq!(live.local_root, Some(root.as_uuid().to_string()));
    runtime.pause().unwrap();
    assert_eq!(worker.join().unwrap().unwrap_err(), SyncRuntimeError::Busy);
    checkpoint(2, partial(9));
    assert_eq!(runtime.status().state, SyncState::Paused);
    assert_eq!(runtime.status().uploaded_bytes, Some(4));
    runtime.stop().unwrap();
}
