//! Real SQLite + `RootControlPlane` business writes at filesystem boundaries.
use super::*;
use echo_core::application::root_switch::ActivateLibrary;
use echo_core::domain::ids::LibraryRootId;
use echo_core::domain::library::LibraryManifest;
use echo_core::infrastructure::filesystem::RootControlPlane;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

pub(super) struct RealFixture {
    _directory: tempfile::TempDir,
    pub(super) app: Arc<AppServices>,
    pub(super) root: LibraryRootId,
    candidate: LibraryRootId,
}

pub(super) fn real_fixture(
    phase: &'static str,
    reached: mpsc::Sender<()>,
    resume: mpsc::Receiver<()>,
) -> RealFixture {
    let directory = tempfile::tempdir().unwrap();
    let routed = crate::runtime::app::assemble(
        &directory.path().join("local.db"),
        &directory.path().join("covers"),
    )
    .unwrap();
    let root = LibraryRootId::new();
    let candidate = LibraryRootId::new();
    for (id, active, name) in [(root, true, "old"), (candidate, false, "new")] {
        let path = directory
            .path()
            .join("音乐资料根路径".repeat(8))
            .join("演奏者专辑目录".repeat(8))
            .join("多语言歌曲归档".repeat(8))
            .join(name);
        std::fs::create_dir_all(&path).unwrap();
        routed.registry.register(id, &path);
        LibraryRepository::upsert(
            routed.database.as_ref(),
            &LibraryRoot::new(id, path, active, true),
        )
        .unwrap();
        routed
            .deps
            .control
            .write_manifest(
                id,
                &LibraryManifest::new(
                    echo_core::domain::library::LibraryId::from_uuid(uuid::Uuid::new_v4()),
                    "test",
                ),
            )
            .unwrap();
    }
    let startup = Arc::new(StartupSupervisor::new());
    let supervisor = ScanSupervisor::new();
    startup
        .run_recovery(&routed.deps, &supervisor, &FakeTrash::new())
        .unwrap();
    let resume = Mutex::new(resume);
    let first = std::sync::atomic::AtomicBool::new(true);
    let control = Arc::new(
        RootControlPlane::new(routed.registry.clone()).with_commit_hook(move |at, _| {
            if at == phase && first.swap(false, std::sync::atomic::Ordering::SeqCst) {
                reached.send(()).unwrap();
                resume.lock().unwrap().recv().unwrap();
            }
            Ok(())
        }),
    );
    routed.database.configure_library_control(control.clone());
    let deps = Arc::new(ScanDeps {
        control,
        ..(*routed.deps).clone()
    });
    let app = Arc::new(AppServices::with_runtime(
        deps,
        supervisor,
        startup,
        Arc::new(TestDialogs::cancelling()),
        routed.registry,
        routed.database,
        Blockers::new(),
    ));
    RealFixture {
        _directory: directory,
        app,
        root,
        candidate,
    }
}

#[test]
fn activation_waits_for_actual_directory_write_before_changing_epoch() {
    for phase in ["prepared", "committed"] {
        let (reached, paused) = mpsc::channel();
        let (release, resume) = mpsc::channel();
        let f = real_fixture(phase, reached, resume);
        let writer_app = Arc::clone(&f.app);
        let root = f.root;
        let writer =
            std::thread::spawn(move || writer_app.create_playlist(root, "saved before switching"));
        paused.recv_timeout(Duration::from_secs(5)).unwrap();
        let app = Arc::clone(&f.app);
        let candidate = f.candidate;
        let (done, result) = mpsc::channel();
        let activation = std::thread::spawn(move || {
            done.send(
                ActivateLibrary::new(
                    &app.deps,
                    app.deps.roots.as_ref(),
                    &app.supervisor,
                    app.state.as_ref(),
                    &app.blockers,
                )
                .activate(candidate),
            )
            .unwrap();
        });
        let premature = result.recv_timeout(Duration::from_millis(200)).ok();
        let epoch = f.app.state.load("root_epoch").unwrap();
        release.send(()).unwrap();
        writer.join().unwrap().unwrap();
        activation.join().unwrap();
        assert!(
            premature.is_none(),
            "activation returned while {phase} filesystem write was paused"
        );
        assert_eq!(epoch, None, "epoch advanced before the old write drained");
        assert!(result.recv_timeout(Duration::from_secs(5)).unwrap().is_ok());
    }
}

#[test]
fn shutdown_waits_for_actual_directory_write_and_rejects_new_work() {
    for phase in ["prepared", "committed"] {
        let (reached, paused) = mpsc::channel();
        let (release, resume) = mpsc::channel();
        let f = real_fixture(phase, reached, resume);
        let app = Arc::clone(&f.app);
        let root = f.root;
        let writer = std::thread::spawn(move || app.create_playlist(root, "saved before exit"));
        paused.recv_timeout(Duration::from_secs(5)).unwrap();
        let app = Arc::clone(&f.app);
        let (stop, stopped) = mpsc::channel();
        let (done, result) = mpsc::channel();
        let exit = std::thread::spawn(move || {
            done.send(app.shutdown(|| {
                stop.send(()).unwrap();
                Ok(())
            }))
            .unwrap();
        });
        stopped.recv_timeout(Duration::from_secs(5)).unwrap();
        let premature = result.recv_timeout(Duration::from_millis(200)).ok();
        let rejected = f.app.deps.activity.enter().is_err();
        release.send(()).unwrap();
        writer.join().unwrap().unwrap();
        exit.join().unwrap();
        assert!(
            premature.is_none(),
            "shutdown returned while {phase} filesystem write was paused"
        );
        assert!(rejected, "shutdown admitted a fresh outer writer");
        result
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        assert!(f.app.create_playlist(root, "after exit").is_err());
    }
}

#[test]
fn shutdown_failure_reopens_old_library_admission() {
    let (reached, _paused) = mpsc::channel();
    let (_release, resume) = mpsc::channel();
    let f = real_fixture("never", reached, resume);
    assert!(f
        .app
        .shutdown(|| Err(Error::unavailable("producer", "injected stop failure")))
        .is_err());
    assert_eq!(
        f.app.deps.roots.active_root().unwrap().unwrap().id(),
        f.root
    );
    f.app.create_playlist(f.root, "still writable").unwrap();
}

#[test]
fn activation_rechecks_blockers_after_drain_and_reopens_old_library() {
    let (reached, paused) = mpsc::channel();
    let (release, resume) = mpsc::channel();
    let f = real_fixture("committed", reached, resume);
    let app = Arc::clone(&f.app);
    let root = f.root;
    let writer =
        std::thread::spawn(move || app.create_playlist(root, "saved before refused activation"));
    paused.recv_timeout(Duration::from_secs(5)).unwrap();
    let app = Arc::clone(&f.app);
    let candidate = f.candidate;
    let activation = std::thread::spawn(move || {
        ActivateLibrary::new(
            &app.deps,
            app.deps.roots.as_ref(),
            &app.supervisor,
            app.state.as_ref(),
            &app.blockers,
        )
        .activate(candidate)
    });
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !f.app.deps.activity.is_closed() && std::time::Instant::now() < deadline {
        std::thread::yield_now();
    }
    let closed = f.app.deps.activity.is_closed();
    f.app
        .blockers
        .add("new unresolved operation found during drain");
    release.send(()).unwrap();
    writer.join().unwrap().unwrap();
    assert!(activation.join().unwrap().is_err());
    assert!(closed);
    assert_eq!(
        f.app.deps.roots.active_root().unwrap().unwrap().id(),
        f.root
    );
    assert_eq!(f.app.state.load("root_epoch").unwrap(), None);
    f.app
        .create_playlist(root, "old library still writable")
        .unwrap();
}

#[test]
fn long_unicode_root_real_commit_reopen_recover_and_retry_preserve_identity() {
    use echo_core::application::library_recovery::RetryLibraryOperation;
    use echo_core::application::ports::PlaylistRepository;
    let (reached, _paused) = mpsc::channel();
    let (_release, resume) = mpsc::channel();
    let RealFixture {
        _directory: directory,
        app,
        root,
        ..
    } = real_fixture("never", reached, resume);
    let root_path = app
        .deps
        .roots
        .by_id(root)
        .unwrap()
        .unwrap()
        .absolute_path()
        .to_path_buf();
    assert!(root_path.as_os_str().len() > 260);
    let playlist = app
        .create_playlist(root, "旅行 🎻 音乐")
        .unwrap()
        .parse::<PlaylistId>()
        .unwrap();
    let replay = app
        .deps
        .control
        .durability()
        .unwrap()
        .recover(root)
        .unwrap();
    let operation = OperationId::from_uuid(replay.commits.last().unwrap().batch.operation_id);
    drop(app);
    let reopened = crate::runtime::app::assemble(
        &directory.path().join("local.db"),
        &directory.path().join("covers"),
    )
    .unwrap();
    let retry = RetryLibraryOperation::new(
        reopened.deps.control.as_ref(),
        reopened.deps.history.as_ref().unwrap().as_ref(),
    );
    retry.execute(root, operation).unwrap();
    retry.catch_up(root).unwrap();
    let record = PlaylistRepository::by_id(reopened.database.as_ref(), playlist)
        .unwrap()
        .unwrap();
    assert_eq!(record, playlist);
    assert_eq!(
        PlaylistRepository::name(reopened.database.as_ref(), playlist)
            .unwrap()
            .as_deref(),
        Some("旅行 🎻 音乐")
    );
    assert_eq!(
        reopened
            .deps
            .control
            .durability()
            .unwrap()
            .recover(root)
            .unwrap()
            .head(),
        replay.head()
    );
}
