use super::*;
use std::sync::{mpsc, Arc};
use std::time::Duration;

#[test]
fn closing_rejects_fresh_outer_operations() {
    let gate = LocalActivity::default();
    let closing = gate.close().unwrap();
    assert!(gate.enter().is_err());
    assert!(gate.exclusive().is_err());
    drop(closing);
    assert!(gate.enter().is_ok());
}

#[test]
fn admitted_thread_can_nest_while_closing_and_drain_waits() {
    let gate = Arc::new(LocalActivity::default());
    let (ready, received) = mpsc::channel();
    let (release, proceed) = mpsc::channel();
    let other = Arc::clone(&gate);
    let worker = thread::spawn(move || {
        let _outer = other.enter().unwrap();
        ready.send(()).unwrap();
        proceed.recv().unwrap();
        let _nested = other.enter().unwrap();
        assert!(other.close().is_err());
    });
    received.recv().unwrap();
    let closing = gate.close().unwrap();
    assert!(gate.enter().is_err());
    release.send(()).unwrap();
    closing.drain();
    worker.join().unwrap();
    closing.seal();
    assert!(gate.enter().is_err());
}

#[test]
fn instances_are_independent() {
    let first = LocalActivity::default();
    let second = LocalActivity::default();
    let _closed = first.close().unwrap();
    assert!(second.enter().is_ok());
}

#[test]
fn closure_wakes_waiting_exclusive_without_deadlocking_producer_join() {
    let gate = Arc::new(LocalActivity::default());
    let (admitted, ready) = mpsc::channel();
    let (release, resume) = mpsc::channel();
    let other = Arc::clone(&gate);
    let worker = thread::spawn(move || {
        let _lease = other.enter().unwrap();
        admitted.send(()).unwrap();
        resume.recv().unwrap();
    });
    ready.recv().unwrap();
    let other = Arc::clone(&gate);
    let (done, result) = mpsc::channel();
    let exclusive = thread::spawn(move || done.send(other.exclusive().is_err()).unwrap());
    let closing = gate.close().unwrap();
    assert!(result.recv_timeout(Duration::from_secs(2)).unwrap());
    exclusive.join().unwrap();
    release.send(()).unwrap();
    closing.drain();
    worker.join().unwrap();
}

#[test]
fn queued_exclusive_cannot_miss_a_complete_temporary_closure() {
    let gate = Arc::new(LocalActivity::default());
    let lease = gate.enter().unwrap();
    let other = Arc::clone(&gate);
    let worker = thread::spawn(move || other.exclusive().is_err());
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while gate.lock().waiting_exclusive == 0 {
        assert!(std::time::Instant::now() < deadline);
        thread::yield_now();
    }
    // Keep the mutex through the entire closure pulse: the waiter cannot
    // observe `closed`, exactly as when it loses scheduling until reopening.
    {
        let mut state = gate.lock();
        state.closed = true;
        state.generation = state.generation.wrapping_add(1);
        state.closed = false;
        drop(state);
        gate.changed.notify_all();
    }
    drop(lease);
    assert!(worker.join().unwrap());
    assert!(gate.enter().is_ok());
}

#[test]
fn queued_enter_cannot_miss_a_complete_temporary_closure() {
    let gate = Arc::new(LocalActivity::default());
    let lease = gate.exclusive().unwrap();
    let other = Arc::clone(&gate);
    let worker = thread::spawn(move || other.enter().is_err());
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while gate.lock().waiting_shared == 0 {
        assert!(std::time::Instant::now() < deadline);
        thread::yield_now();
    }
    // Hold the mutex through the pulse so the queued reader sees only the
    // reopened state. Its captured generation must still reject the old work.
    {
        let mut state = gate.lock();
        state.closed = true;
        state.generation = state.generation.wrapping_add(1);
        state.closed = false;
        drop(state);
        gate.changed.notify_all();
    }
    drop(lease);
    assert!(worker.join().unwrap());
    assert!(gate.enter().is_ok());
}
