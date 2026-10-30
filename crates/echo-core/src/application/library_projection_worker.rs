//! Bounded wake-ups for one library's independently persisted read model.
//!
//! A wake carries no operation payload: `catch_up` must read the authoritative
//! directory chain and atomically advance its own projection watermark. Calls
//! are serialized and retried; a wake arriving during a call requires another
//! pass. The callback must bound its blocking I/O because shutdown cannot cancel
//! an in-progress callback. Call `flush` before `join` when pending work must drain.

use crate::domain::ids::LibraryRootId;
use crate::error::Error;
use std::fmt;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const COALESCE_QUIET_PERIOD: Duration = Duration::from_millis(500);
const COALESCE_MAX_DELAY: Duration = Duration::from_secs(30);

/// Current activity of a single-root projection worker.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProjectionWorkerPhase {
    Idle,
    Pending,
    Running,
    RetryWaiting,
    Stopped,
}

/// Runtime diagnostics. Generations count wake-ups, not library commit numbers.
#[derive(Clone, Debug)]
pub struct ProjectionWorkerStatus {
    pub phase: ProjectionWorkerPhase,
    pub requested_generation: u64,
    pub projected_generation: u64,
    pub consecutive_failures: u32,
    /// The original typed error is shared because `Error` deliberately is not Clone.
    pub last_error: Option<Arc<Error>>,
}

impl ProjectionWorkerStatus {
    /// Whether a requested pass has not yet completed successfully.
    #[must_use]
    pub const fn pending(&self) -> bool {
        self.projected_generation < self.requested_generation
    }
}

/// Exponential retry delays. A new wake does not bypass an existing backoff.
#[derive(Clone, Copy, Debug)]
pub struct ProjectionRetryPolicy {
    pub initial_delay: Duration,
    pub max_delay: Duration,
}

impl Default for ProjectionRetryPolicy {
    fn default() -> Self {
        Self {
            initial_delay: Duration::from_millis(10),
            max_delay: Duration::from_secs(1),
        }
    }
}

impl ProjectionRetryPolicy {
    fn validate(self) -> Result<(), Error> {
        if self.initial_delay.is_zero()
            || self.initial_delay > self.max_delay
            || self.max_delay > Duration::from_secs(60)
        {
            return Err(Error::conflict(
                "projection retry delays must be positive, ordered and at most 60 seconds",
            ));
        }
        Ok(())
    }
}

struct State {
    status: ProjectionWorkerStatus,
    stopping: bool,
    callback_panicked: bool,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn stop(&self) {
        self.lock().stopping = true;
        self.changed.notify_all();
    }
}

/// Owns one joinable thread and one coalesced pending wake for a fixed library.
/// No filesystem or database lock is held while the scheduler waits.
pub struct LibraryProjectionWorker {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl LibraryProjectionWorker {
    /// Start idle; the owner must wake once to request startup catch-up.
    ///
    /// # Errors
    /// Reports an OS thread creation failure.
    pub fn spawn(
        root: LibraryRootId,
        catch_up: impl FnMut(LibraryRootId) -> Result<(), Error> + Send + 'static,
    ) -> Result<Self, Error> {
        Self::spawn_with_retry(root, ProjectionRetryPolicy::default(), catch_up)
    }

    /// Start with bounded, explicit retry delays.
    ///
    /// # Errors
    /// Rejects invalid delays or reports an OS thread creation failure.
    pub fn spawn_with_retry(
        root: LibraryRootId,
        retry: ProjectionRetryPolicy,
        mut catch_up: impl FnMut(LibraryRootId) -> Result<(), Error> + Send + 'static,
    ) -> Result<Self, Error> {
        retry.validate()?;
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                status: ProjectionWorkerStatus {
                    phase: ProjectionWorkerPhase::Idle,
                    requested_generation: 0,
                    projected_generation: 0,
                    consecutive_failures: 0,
                    last_error: None,
                },
                stopping: false,
                callback_panicked: false,
            }),
            changed: Condvar::new(),
        });
        let worker = Arc::clone(&shared);
        let thread = thread::Builder::new()
            .name("echo-library-projection".to_owned())
            .spawn(move || run(&worker, root, retry, &mut catch_up))
            .map_err(|error| {
                Error::unavailable_with_source(
                    "library projection worker",
                    "could not create projection thread",
                    error,
                )
            })?;
        Ok(Self {
            shared,
            thread: Some(thread),
        })
    }

    /// Request catch-up. Any number of waiting wakes uses constant storage.
    ///
    /// # Errors
    /// Rejects a stopped worker or exhausted generation counter.
    pub fn wake(&self) -> Result<(), Error> {
        let mut state = self.shared.lock();
        if state.stopping {
            return Err(Error::unavailable(
                "library projection worker",
                "worker has stopped accepting work",
            ));
        }
        state.status.requested_generation = state
            .status
            .requested_generation
            .checked_add(1)
            .ok_or_else(|| Error::conflict("projection wake generation exhausted"))?;
        if state.status.phase == ProjectionWorkerPhase::Idle {
            state.status.phase = ProjectionWorkerPhase::Pending;
        }
        drop(state);
        self.shared.changed.notify_all();
        Ok(())
    }

    /// Read diagnostics without consuming the original callback error.
    #[must_use]
    pub fn status(&self) -> ProjectionWorkerStatus {
        self.shared.lock().status.clone()
    }

    /// Wait for all wakes accepted before this call, including automatic retries.
    /// Later wakes do not extend this barrier. No caller-owned I/O lock may be
    /// held across this call.
    ///
    /// # Errors
    /// Reports a timeout or a worker stopped before completing this barrier. The
    /// last projection error remains attached as the source when available.
    pub fn flush(&self, timeout: Duration) -> Result<(), Error> {
        let deadline = Instant::now()
            .checked_add(timeout)
            .ok_or_else(|| Error::conflict("projection flush timeout exceeds clock range"))?;
        let mut state = self.shared.lock();
        let target = state.status.requested_generation;
        loop {
            if state.status.projected_generation >= target {
                return Ok(());
            }
            if state.status.phase == ProjectionWorkerPhase::Stopped {
                return Err(wait_error(&state, "worker stopped before completing flush"));
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(wait_error(&state, "projection flush timed out"));
            }
            (state, _) = self
                .shared
                .changed
                .wait_timeout(state, deadline.saturating_duration_since(now))
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }

    /// Stop accepting work and cancel waits/retries. Durable operations remain in
    /// the directory for the next worker. An active callback is allowed to finish.
    pub fn shutdown(&self) {
        self.shared.stop();
    }

    /// Request shutdown and join. This does not imply pending work was flushed;
    /// the returned status retains that distinction. Release any resource the
    /// callback may acquire before calling this method or dropping the worker.
    ///
    /// # Errors
    /// Reports a callback or worker panic. A failed pending projection is exposed
    /// by the returned status rather than silently being declared complete.
    pub fn join(mut self) -> Result<ProjectionWorkerStatus, Error> {
        self.shutdown();
        if let Some(thread) = self.thread.take() {
            thread.join().map_err(|_| Error::InvariantViolation {
                why: "library projection thread panicked".to_owned(),
            })?;
        }
        let state = self.shared.lock();
        if state.callback_panicked {
            return Err(wait_error(&state, "projection callback panicked"));
        }
        Ok(state.status.clone())
    }
}

impl Drop for LibraryProjectionWorker {
    fn drop(&mut self) {
        self.shared.stop();
        if let Some(thread) = self.thread.take() {
            // Explicit join reports a panic; Drop still reclaims the thread.
            let _ = thread.join();
        }
    }
}

#[derive(Debug)]
struct SharedFailure(Arc<Error>);

impl fmt::Display for SharedFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl std::error::Error for SharedFailure {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(self.0.as_ref())
    }
}

fn wait_error(state: &State, hint: &'static str) -> Error {
    state.status.last_error.as_ref().map_or_else(
        || Error::unavailable("library projection worker", hint),
        |error| {
            Error::unavailable_with_source(
                "library projection worker",
                hint,
                SharedFailure(Arc::clone(error)),
            )
        },
    )
}

fn run(
    shared: &Shared,
    root: LibraryRootId,
    retry: ProjectionRetryPolicy,
    catch_up: &mut impl FnMut(LibraryRootId) -> Result<(), Error>,
) {
    let mut delay = retry.initial_delay;
    loop {
        let generation = {
            let mut state = shared.lock();
            while !state.stopping && !state.status.pending() {
                state.status.phase = ProjectionWorkerPhase::Idle;
                state = shared
                    .changed
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
            }
            if state.stopping {
                state.status.phase = ProjectionWorkerPhase::Stopped;
                drop(state);
                shared.changed.notify_all();
                return;
            }
            // Let a short burst of UI edits share one recovery/projection pass.
            // The maximum delay bounds backlog even if writes keep arriving.
            state.status.phase = ProjectionWorkerPhase::Pending;
            let max_deadline = Instant::now() + COALESCE_MAX_DELAY;
            let mut quiet_deadline = Instant::now() + COALESCE_QUIET_PERIOD;
            let mut observed = state.status.requested_generation;
            while !state.stopping {
                let now = Instant::now();
                let deadline = quiet_deadline.min(max_deadline);
                if now >= deadline {
                    break;
                }
                (state, _) = shared
                    .changed
                    .wait_timeout(state, deadline.saturating_duration_since(now))
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if state.status.requested_generation != observed {
                    observed = state.status.requested_generation;
                    quiet_deadline = Instant::now() + COALESCE_QUIET_PERIOD;
                }
            }
            if state.stopping {
                state.status.phase = ProjectionWorkerPhase::Stopped;
                drop(state);
                shared.changed.notify_all();
                return;
            }
            state.status.phase = ProjectionWorkerPhase::Running;
            state.status.requested_generation
        };
        // Neither the state mutex nor any scheduler-owned I/O lock surrounds
        // user code, so status, wake and shutdown remain available during I/O.
        let result = catch_unwind(AssertUnwindSafe(|| catch_up(root)));
        let mut state = shared.lock();
        match result {
            Ok(Ok(())) => {
                state.status.projected_generation = generation;
                state.status.consecutive_failures = 0;
                state.status.last_error = None;
                state.status.phase = ProjectionWorkerPhase::Pending;
                delay = retry.initial_delay;
            }
            Ok(Err(error)) => {
                state.status.consecutive_failures =
                    state.status.consecutive_failures.saturating_add(1);
                state.status.last_error = Some(Arc::new(error));
                state.status.phase = ProjectionWorkerPhase::RetryWaiting;
                shared.changed.notify_all();
                let deadline = Instant::now() + delay;
                while !state.stopping && Instant::now() < deadline {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    (state, _) = shared
                        .changed
                        .wait_timeout(state, remaining)
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                }
                delay = delay.saturating_mul(2).min(retry.max_delay);
            }
            Err(_) => {
                state.status.last_error = Some(Arc::new(Error::InvariantViolation {
                    why: "library projection callback panicked".to_owned(),
                }));
                state.callback_panicked = true;
                state.stopping = true;
                state.status.phase = ProjectionWorkerPhase::Stopped;
                drop(state);
                shared.changed.notify_all();
                return;
            }
        }
        drop(state);
        shared.changed.notify_all();
    }
}

#[cfg(test)]
#[path = "library_projection_worker/tests.rs"]
mod tests;
