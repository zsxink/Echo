//! Exercise the real scheduler and SQLite notifications without LAN fixtures.

use super::*;
use echo_core::application::ports::{LibraryRepository, LocalSyncQueuePort, PlaylistRepository};
use echo_core::domain::entities::LibraryRoot;
use echo_core::domain::ids::PlaylistId;

struct MaintenanceProvisioner {
    database: Arc<SqliteDatabase>,
    cycles: mpsc::Sender<()>,
    hold_next: AtomicBool,
    release: Mutex<mpsc::Receiver<()>>,
}

impl SyncProvisioner for MaintenanceProvisioner {
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

impl SyncCyclePort for MaintenanceProvisioner {
    fn sync_now(&self, _: SyncRole) -> Result<bool, SyncRuntimeError> {
        // Production Host performs this maintenance on every cycle, including
        // an empty library. Previously its Ok(0) woke this same worker again.
        self.database
            .advance_rejected_local_mutations()
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        let hold = self.hold_next.swap(false, Ordering::AcqRel);
        self.cycles
            .send(())
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        if hold {
            self.release
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(5))
                .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        }
        Ok(false)
    }
}

struct RunningSync(SyncRuntime);

impl Drop for RunningSync {
    fn drop(&mut self) {
        // Also join the worker if a regression assertion fails.
        let _ = self.0.stop();
    }
}

#[test]
fn idle_sync_blocks_after_maintenance_and_retains_edits_during_a_cycle() {
    let directory = tempfile::tempdir().unwrap();
    let database = Arc::new(SqliteDatabase::open(directory.path().join("echo.sqlite")).unwrap());
    let root = LibraryRootId::new();
    LibraryRepository::upsert(
        database.as_ref(),
        &LibraryRoot::new(root, directory.path().join("library"), true, true),
    )
    .unwrap();
    let (cycles, completed) = mpsc::channel();
    let (release, gate) = mpsc::channel();
    let provisioner = Arc::new(MaintenanceProvisioner {
        database: Arc::clone(&database),
        cycles,
        hold_next: AtomicBool::new(false),
        release: Mutex::new(gate),
    });
    let runtime = RunningSync(SyncRuntime::new(
        Arc::clone(&database),
        Some(provisioner.clone()),
    ));
    {
        let mut inner = runtime.0.inner.lock().unwrap();
        inner.status.enabled = true;
        inner.status.role = SyncRole::Host;
        inner.status.state = SyncState::Hosting;
    }
    runtime.0.start_sync_loop(SyncRole::Host).unwrap();
    completed.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(
        completed.recv_timeout(Duration::from_millis(150)),
        Err(mpsc::RecvTimeoutError::Timeout),
        "empty maintenance must not immediately schedule itself again"
    );

    provisioner.hold_next.store(true, Ordering::Release);
    PlaylistRepository::create(database.as_ref(), PlaylistId::new(), root, "First edit").unwrap();
    completed.recv_timeout(Duration::from_secs(2)).unwrap();
    // The second cycle is still running when a concurrent edit commits.
    PlaylistRepository::create(
        database.as_ref(),
        PlaylistId::new(),
        root,
        "Concurrent edit",
    )
    .unwrap();
    release.send(()).unwrap();
    completed.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(
        completed.recv_timeout(Duration::from_millis(150)),
        Err(mpsc::RecvTimeoutError::Timeout),
        "after processing the concurrent edit the scheduler must return to waiting"
    );
    runtime.0.stop().unwrap();
}
