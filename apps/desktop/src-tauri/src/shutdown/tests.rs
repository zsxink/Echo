use super::*;
use std::sync::{atomic::AtomicUsize, mpsc};
use std::time::Duration;

#[test]
fn repeated_exit_requests_dispatch_one_background_stop_and_wait_for_drain() {
    let coordinator = Arc::new(ExitCoordinator::default());
    let stops = Arc::new(AtomicUsize::new(0));
    let worker_stops = Arc::clone(&stops);
    let (started, start) = mpsc::channel();
    let (release, drain) = mpsc::channel();
    let (done, completion) = mpsc::channel();
    let caller = std::thread::current().id();
    assert!(coordinator
        .dispatch(
            move || {
                worker_stops.fetch_add(1, Ordering::AcqRel);
                started.send(std::thread::current().id()).unwrap();
                drain.recv().unwrap();
                Ok(())
            },
            move || done.send(()).unwrap(),
            |error| panic!("unexpected stop error: {error}"),
        )
        .unwrap());
    assert_ne!(start.recv_timeout(Duration::from_secs(2)).unwrap(), caller);
    for _ in 0..10 {
        assert!(!coordinator
            .dispatch(
                || panic!("duplicate stop"),
                || panic!("duplicate exit"),
                |_| panic!("duplicate failure")
            )
            .unwrap());
    }
    assert!(!coordinator.ready());
    assert!(completion.try_recv().is_err());
    assert_eq!(stops.load(Ordering::Acquire), 1);
    release.send(()).unwrap();
    completion.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(coordinator.ready());
}

#[test]
fn failed_producer_stop_allows_a_second_background_cleanup_attempt() {
    let coordinator = Arc::new(ExitCoordinator::default());
    let (failed, failure) = mpsc::channel();
    assert!(coordinator
        .dispatch(
            || Err(Error::unavailable("producer", "injected cleanup failure")),
            || panic!("failed cleanup must not exit"),
            move |error| failed.send(error.code().to_owned()).unwrap(),
        )
        .unwrap());
    assert_eq!(
        failure.recv_timeout(Duration::from_secs(2)).unwrap(),
        "unavailable"
    );
    assert!(!coordinator.ready());
    let (done, completion) = mpsc::channel();
    assert!(coordinator
        .dispatch(
            || Ok(()),
            move || done.send(()).unwrap(),
            |_| panic!("cleanup retry failed")
        )
        .unwrap());
    completion.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(coordinator.ready());
}

#[test]
fn a_panicking_shutdown_worker_restores_dispatch_for_cleanup_retry() {
    let coordinator = Arc::new(ExitCoordinator::default());
    let (failed, failure) = mpsc::channel();
    coordinator
        .dispatch(
            || panic!("injected producer panic"),
            || panic!("must not exit"),
            move |_| failed.send(()).unwrap(),
        )
        .unwrap();
    failure.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(!coordinator.ready());
    assert!(coordinator.begin());
}
