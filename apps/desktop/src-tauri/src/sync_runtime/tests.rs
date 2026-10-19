use super::*;

#[path = "tests/idle.rs"]
mod idle;

#[path = "tests/watch.rs"]
mod watch;

#[path = "tests/progress.rs"]
mod progress;

#[path = "tests/host_resources.rs"]
mod host_resources;

#[test]
fn unconfigured_runtime_stays_offline_and_stop_is_idempotent() {
    let directory = tempfile::tempdir().unwrap();
    let database = Arc::new(SqliteDatabase::open(directory.path().join("echo.sqlite")).unwrap());
    let runtime = SyncRuntime::new(database, None);
    assert_eq!(runtime.status().state, SyncState::Disabled);
    assert_eq!(
        runtime.host_start("192.168.1.2").unwrap_err(),
        SyncRuntimeError::Unconfigured
    );
    assert_eq!(
        runtime.peer_start().unwrap_err(),
        SyncRuntimeError::Unconfigured
    );
    assert_eq!(
        runtime.sync_now().unwrap_err(),
        SyncRuntimeError::Unconfigured
    );
    assert!(!runtime.status().enabled);
    assert_eq!(runtime.stop().unwrap().state, SyncState::Disabled);
    assert_eq!(runtime.stop().unwrap().state, SyncState::Disabled);
}

#[test]
fn host_address_cannot_bind_wildcards_public_or_loopback_interfaces() {
    for address in [
        "0.0.0.0",
        "127.0.0.1",
        "8.8.8.8",
        "224.0.0.1",
        "::",
        "::1",
        "2001:4860:4860::8888",
        "hostname",
    ] {
        assert_eq!(
            selected_lan_address(address),
            Err(SyncRuntimeError::InvalidAddress)
        );
    }
    for address in [
        "192.168.1.2",
        "10.0.0.1",
        "172.16.0.2",
        "169.254.1.2",
        "fd00::1",
    ] {
        assert!(selected_lan_address(address).is_ok());
    }
}

#[test]
fn local_write_wakes_sync_while_retry_backoff_suppresses_it() {
    let stop = AtomicBool::new(false);
    let (wake, wakeups) = mpsc::channel();
    wake.send(()).unwrap();
    assert!(wait_for_sync_wakeup(
        &wakeups,
        Duration::from_secs(1),
        false,
        &stop
    ));

    let (wake, wakeups) = mpsc::channel();
    let wake_sender = wake.clone();
    let writer = thread::spawn(move || {
        wake.send(()).unwrap();
        thread::sleep(Duration::from_millis(15));
    });
    let started = Instant::now();
    assert!(wait_for_sync_wakeup(
        &wakeups,
        Duration::from_millis(40),
        true,
        &stop
    ));
    writer.join().unwrap();
    drop(wake_sender);
    assert!(started.elapsed() >= Duration::from_millis(35));
}

#[test]
fn stopping_wakes_a_waiting_sync_loop_immediately() {
    let (wake, wakeups) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let worker_stop = Arc::clone(&stop);
    let worker = thread::spawn(move || {
        wait_for_sync_wakeup(&wakeups, Duration::from_secs(30), false, &worker_stop)
    });
    stop.store(true, Ordering::Release);
    let _ = wake.send(());
    assert!(!worker.join().unwrap());
}
