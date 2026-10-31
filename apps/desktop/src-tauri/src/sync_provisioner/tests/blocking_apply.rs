//! A slow directory commit must leave the async transport executor responsive.
use super::*;
use echo_core::application::ports::{
    ControlPlaneInventoryPort, ControlPlanePort, LibraryDurabilityPort, ManifestState,
};
use echo_core::domain::library::{LibraryManifest, PortableRecord, RecordKind};
use echo_core::error::Error;

struct SlowDirectory {
    inner: Arc<dyn ControlPlanePort>,
    started: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    completed: Arc<AtomicBool>,
}

impl ControlPlaneInventoryPort for SlowDirectory {
    fn records_present(&self, root: LibraryRootId) -> Result<bool, Error> {
        self.inner.records_present(root)
    }
}

impl ControlPlanePort for SlowDirectory {
    fn durability(&self) -> Option<&dyn LibraryDurabilityPort> {
        self.inner.durability()
    }
    fn read_manifest(&self, root: LibraryRootId) -> Result<Option<LibraryManifest>, Error> {
        let start = self.started.lock().unwrap().take();
        if let Some(started) = start {
            let _ = started.send(());
            std::thread::sleep(Duration::from_millis(250));
            self.completed.store(true, Ordering::Release);
        }
        self.inner.read_manifest(root)
    }
    fn write_manifest(&self, root: LibraryRootId, manifest: &LibraryManifest) -> Result<(), Error> {
        self.inner.write_manifest(root, manifest)
    }
    fn write_record(&self, root: LibraryRootId, record: &PortableRecord) -> Result<(), Error> {
        self.inner.write_record(root, record)
    }
    fn read_record(
        &self,
        root: LibraryRootId,
        kind: RecordKind,
        id: &str,
    ) -> Result<Option<PortableRecord>, Error> {
        self.inner.read_record(root, kind, id)
    }
    fn delete_record(&self, root: LibraryRootId, kind: RecordKind, id: &str) -> Result<(), Error> {
        self.inner.delete_record(root, kind, id)
    }
    fn list_records(&self, root: LibraryRootId, kind: RecordKind) -> Result<Vec<String>, Error> {
        self.inner.list_records(root, kind)
    }
    fn control_plane_usable(&self, root: LibraryRootId) -> Result<bool, Error> {
        self.inner.control_plane_usable(root)
    }
    fn manifest_state(&self, root: LibraryRootId) -> Result<ManifestState, Error> {
        self.inner.manifest_state(root)
    }
}

#[tokio::test]
async fn native_pull_keeps_transport_responsive_during_slow_directory_apply() {
    let fixture = LanMergeFixture::new().await;
    let (start, started) = tokio::sync::oneshot::channel();
    let completed = Arc::new(AtomicBool::new(false));
    fixture
        .replica
        .configure_library_control(Arc::new(SlowDirectory {
            inner: Arc::clone(&fixture.provisioner.scan_deps.control),
            started: Mutex::new(Some(start)),
            completed: Arc::clone(&completed),
        }));
    let queue = ScopedLocalQueue {
        database: fixture.replica.as_ref(),
        root: fixture.root,
    };
    let (pull, ()) = tokio::join!(
        fixture.provisioner.pull_host_changes(
            &fixture.client,
            &fixture.peer,
            &fixture.endpoint,
            &queue
        ),
        async {
            started.await.unwrap();
            tokio::time::sleep(Duration::from_millis(25)).await;
            assert!(
                !completed.load(Ordering::Acquire),
                "directory IO blocked the current-thread transport executor"
            );
        }
    );
    pull.unwrap();
    assert!(completed.load(Ordering::Acquire));
    assert!(fixture
        .replica
        .all_in_root(fixture.root)
        .unwrap()
        .iter()
        .any(|song| song.id() == fixture.host_song));
    assert_eq!(fixture.host_pulls.load(Ordering::Relaxed), 2);
}
