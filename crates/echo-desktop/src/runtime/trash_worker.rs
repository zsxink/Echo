//! In-session finalization of expired delete operations.
//!
//! The Core use case owns all journal and database transitions. This module
//! only supplies a lifecycle-bound blocking scheduler so an open desktop
//! session does not have to be restarted before an expired delete reaches the
//! operating system trash.

use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use echo_core::application::ports::SystemTrashPort;
use echo_core::application::scan::ScanDeps;
use echo_core::application::trash::{FinalizeExpiredDeletes, TrashFinalizationReport};
use echo_core::error::Error;

/// The maximum additional delay after the undo deadline before a live session
/// notices an expired delete.
pub const FINALIZATION_POLL_INTERVAL: Duration = Duration::from_secs(1);

/// Run one finalization pass against the currently active root.
///
/// Root resolution happens for every pass so switching libraries while Echo is
/// running cannot make the worker use a stale root id or absolute path.
///
/// # Errors
///
/// Returns the Core repository, journal, filesystem, or transaction error from
/// the finalization pass.
pub fn finalize_active_root(
    deps: &ScanDeps,
    trash: &dyn SystemTrashPort,
) -> Result<Option<TrashFinalizationReport>, Error> {
    let Some(root) = deps.roots.active_root()? else {
        return Ok(None);
    };
    FinalizeExpiredDeletes::new(deps, trash)
        .run(root.id())
        .map(Some)
}

/// A lifecycle-owned blocking worker for delete finalization.
pub struct TrashFinalizationWorker {
    stop: mpsc::Sender<()>,
    join: Mutex<Option<JoinHandle<()>>>,
}

impl TrashFinalizationWorker {
    /// Start an immediately-polling worker with the production interval.
    ///
    /// # Errors
    ///
    /// Returns an unavailable error if the operating system refuses to create
    /// the worker thread.
    pub fn start(deps: Arc<ScanDeps>, trash: Arc<dyn SystemTrashPort>) -> Result<Self, Error> {
        Self::start_with_interval(deps, trash, FINALIZATION_POLL_INTERVAL)
    }

    fn start_with_interval(
        deps: Arc<ScanDeps>,
        trash: Arc<dyn SystemTrashPort>,
        interval: Duration,
    ) -> Result<Self, Error> {
        let (stop, receive_stop) = mpsc::channel();
        let join = thread::Builder::new()
            .name("echo-trash-finalizer".to_owned())
            .spawn(move || {
                run_pass(&deps, trash.as_ref());
                loop {
                    match receive_stop.recv_timeout(interval) {
                        Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                        Err(mpsc::RecvTimeoutError::Timeout) => run_pass(&deps, trash.as_ref()),
                    }
                }
            })
            .map_err(|error| Error::unavailable("trash finalizer", error.to_string()))?;
        Ok(Self {
            stop,
            join: Mutex::new(Some(join)),
        })
    }
}

impl Drop for TrashFinalizationWorker {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        let mut join = self
            .join
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(handle) = join.take() {
            let _ = handle.join();
        }
    }
}

fn run_pass(deps: &ScanDeps, trash: &dyn SystemTrashPort) {
    match finalize_active_root(deps, trash) {
        Ok(Some(report)) if !report.finalized.is_empty() || !report.retryable.is_empty() => {
            tracing::debug!(
                finalized = report.finalized.len(),
                retryable = report.retryable.len(),
                outcome_unknown = report.outcome_unknown.len(),
                "delete finalization pass completed"
            );
        }
        Ok(Some(report)) if !report.outcome_unknown.is_empty() => {
            tracing::warn!(
                outcome_unknown = report.outcome_unknown.len(),
                "delete finalization reached an unknown system-trash outcome"
            );
        }
        Ok(Some(_) | None) => {}
        Err(error) => tracing::warn!(error = %error, "delete finalization pass failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::application::testing::scan_fixture::ScanFixture;
    use echo_core::application::testing::small_fakes::FakeTrash;

    #[test]
    fn pass_without_an_active_root_is_a_noop() {
        let fixture = ScanFixture::new();
        let trash = FakeTrash::new();
        assert!(finalize_active_root(&fixture.deps, &trash)
            .expect("no active root is safe")
            .is_none());
        assert!(trash.calls().is_empty());
    }

    #[test]
    fn worker_stops_and_joins_on_drop() {
        let fixture = ScanFixture::new();
        let trash: Arc<dyn SystemTrashPort> = Arc::new(FakeTrash::new());
        let worker = TrashFinalizationWorker::start_with_interval(
            fixture.deps,
            trash,
            Duration::from_millis(1),
        )
        .expect("worker thread");
        drop(worker);
    }
}
