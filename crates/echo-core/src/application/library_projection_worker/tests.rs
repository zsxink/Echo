//! Scheduler tests use controlled callbacks; no filesystem or SQLite timing is assumed.
use super::*;
use std::error::Error as _;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;

fn wait_for(worker: &LibraryProjectionWorker, predicate: impl Fn(&ProjectionWorkerStatus) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if predicate(&worker.status()) {
            return;
        }
        assert!(Instant::now() < deadline, "worker state did not settle");
        thread::sleep(Duration::from_millis(1));
    }
}

#[test]
fn library_projection_worker_coalesces_waiting_wakes_and_serializes_passes() {
    let root = LibraryRootId::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let recorded = Arc::clone(&calls);
    let (started, running) = mpsc::sync_channel(1);
    let (release, gate) = mpsc::sync_channel(1);
    let worker = LibraryProjectionWorker::spawn(root, move |received| {
        assert_eq!(received, root);
        if recorded.fetch_add(1, Ordering::SeqCst) == 0 {
            started.send(()).unwrap();
            gate.recv_timeout(Duration::from_secs(2)).unwrap();
        }
        Ok(())
    })
    .unwrap();
    worker.wake().unwrap();
    running.recv_timeout(Duration::from_secs(2)).unwrap();
    for _ in 0..1_000 {
        worker.wake().unwrap();
    }
    assert!(worker.status().pending());
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    release.send(()).unwrap();
    worker.flush(Duration::from_secs(2)).unwrap();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    assert_eq!(worker.status().projected_generation, 1_001);
    let stopped = worker.join().unwrap();
    assert_eq!(stopped.phase, ProjectionWorkerPhase::Stopped);
    assert!(!stopped.pending());
}

#[test]
fn library_projection_worker_retries_and_new_wakes_do_not_bypass_backoff() {
    let (attempt, attempts) = mpsc::channel();
    let mut count = 0;
    let worker = LibraryProjectionWorker::spawn_with_retry(
        LibraryRootId::new(),
        ProjectionRetryPolicy {
            initial_delay: Duration::from_millis(60),
            max_delay: Duration::from_millis(60),
        },
        move |_| {
            count += 1;
            attempt.send(Instant::now()).unwrap();
            if count == 1 {
                Err(Error::conflict("projection temporarily unavailable"))
            } else {
                Ok(())
            }
        },
    )
    .unwrap();
    worker.wake().unwrap();
    let first = attempts.recv_timeout(Duration::from_secs(2)).unwrap();
    wait_for(&worker, |status| {
        status.phase == ProjectionWorkerPhase::RetryWaiting
    });
    let failed = worker.status();
    assert!(failed.pending());
    assert_eq!(failed.consecutive_failures, 1);
    assert!(matches!(
        failed.last_error.as_deref(),
        Some(Error::Conflict { .. })
    ));
    for _ in 0..100 {
        worker.wake().unwrap();
    }
    let second = attempts.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(second.duration_since(first) >= Duration::from_millis(55));
    worker.flush(Duration::from_secs(2)).unwrap();
    assert_eq!(worker.status().consecutive_failures, 0);
    assert!(worker.status().last_error.is_none());
    assert!(!worker.join().unwrap().pending());
}

#[test]
fn library_projection_worker_timeout_preserves_failure_and_shutdown_interrupts_retry() {
    let worker = LibraryProjectionWorker::spawn_with_retry(
        LibraryRootId::new(),
        ProjectionRetryPolicy {
            initial_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(1),
        },
        |_| Err(Error::conflict("locked projection")),
    )
    .unwrap();
    worker.wake().unwrap();
    wait_for(&worker, |status| {
        status.phase == ProjectionWorkerPhase::RetryWaiting
    });
    let timeout = worker.flush(Duration::ZERO).unwrap_err();
    assert!(timeout.source().is_some());
    assert!(worker.status().pending());
    let before = Instant::now();
    worker.shutdown();
    assert!(worker.wake().is_err());
    let stopped = worker.join().unwrap();
    assert!(before.elapsed() < Duration::from_millis(500));
    assert_eq!(stopped.phase, ProjectionWorkerPhase::Stopped);
    assert!(stopped.pending());
    assert!(matches!(
        stopped.last_error.as_deref(),
        Some(Error::Conflict { .. })
    ));
}

#[test]
fn library_projection_worker_shutdown_finishes_active_pass_and_preserves_next_wake() {
    let (started, running) = mpsc::sync_channel(1);
    let (release, gate) = mpsc::sync_channel(1);
    let worker = LibraryProjectionWorker::spawn(LibraryRootId::new(), move |_| {
        started.send(()).unwrap();
        gate.recv_timeout(Duration::from_secs(2)).unwrap();
        Ok(())
    })
    .unwrap();
    worker.wake().unwrap();
    running.recv_timeout(Duration::from_secs(2)).unwrap();
    worker.wake().unwrap();
    worker.shutdown();
    assert!(worker.wake().is_err());
    release.send(()).unwrap();
    let stopped = worker.join().unwrap();
    assert_eq!(stopped.projected_generation, 1);
    assert_eq!(stopped.requested_generation, 2);
    assert!(stopped.pending());
}

#[test]
fn library_projection_worker_callback_panic_stops_and_unblocks_flush() {
    let worker = LibraryProjectionWorker::spawn(LibraryRootId::new(), |_| {
        panic!("controlled callback panic")
    })
    .unwrap();
    worker.wake().unwrap();
    assert!(worker.flush(Duration::from_secs(2)).is_err());
    assert_eq!(worker.status().phase, ProjectionWorkerPhase::Stopped);
    assert!(worker.status().pending());
    assert!(worker.join().is_err());
}

#[test]
fn library_projection_worker_rejects_invalid_policy_and_idle_flush_is_complete() {
    assert!(
        LibraryProjectionWorker::spawn_with_retry(
            LibraryRootId::new(),
            ProjectionRetryPolicy {
                initial_delay: Duration::ZERO,
                max_delay: Duration::from_secs(1),
            },
            |_| Ok(()),
        )
        .is_err()
    );
    let worker = LibraryProjectionWorker::spawn(LibraryRootId::new(), |_| Ok(())).unwrap();
    worker.flush(Duration::ZERO).unwrap();
    assert!(!worker.join().unwrap().pending());
}
