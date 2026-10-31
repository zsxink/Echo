//! Exit-only fence, serialized with every producer admission.
use super::{
    LibraryRootId, Ordering, RestoreRetryWorker, SyncRuntime, SyncRuntimeError, SyncSessionIntent,
    SyncStatus,
};

pub(super) enum RestoreRetryProgress {
    Deferred,
    Complete,
    Failed,
}

impl SyncRuntime {
    pub(super) fn ensure_running(&self) -> Result<(), SyncRuntimeError> {
        if self.shutting_down.load(Ordering::Acquire) {
            Err(SyncRuntimeError::Busy)
        } else {
            Ok(())
        }
    }

    pub(super) fn running_transition(
        &self,
    ) -> Result<std::sync::MutexGuard<'_, ()>, SyncRuntimeError> {
        let transition = self
            .transition
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        self.ensure_running()?;
        Ok(transition)
    }

    // Lifecycle transitions cancel and join this worker while holding the
    // transition lock. An automatic attempt must never wait for that lock.
    pub(super) fn try_restore_retry_transition(
        &self,
        generation: u64,
        root: LibraryRootId,
    ) -> Result<Option<std::sync::MutexGuard<'_, ()>>, SyncRuntimeError> {
        let transition = match self.transition.try_lock() {
            Ok(transition) => transition,
            Err(std::sync::TryLockError::WouldBlock) => {
                #[cfg(test)]
                self.restore_retry_contentions
                    .fetch_add(1, Ordering::AcqRel);
                return Ok(None);
            }
            Err(std::sync::TryLockError::Poisoned(_)) => {
                return Err(SyncRuntimeError::WorkerFailed)
            }
        };
        self.ensure_running()?;
        if !self.restore_retry_is_current(generation, root) {
            return Err(SyncRuntimeError::Busy);
        }
        Ok(Some(transition))
    }

    // One admitted attempt owns the transition through both restoration and
    // result publication. Contention cannot consume the finite attempt budget.
    pub(super) fn run_restore_retry_attempt(
        &self,
        generation: u64,
        root: LibraryRootId,
        attempt: u32,
        max_attempts: u32,
    ) -> RestoreRetryProgress {
        let _transition = match self.try_restore_retry_transition(generation, root) {
            Ok(Some(transition)) => transition,
            Ok(None) => return RestoreRetryProgress::Deferred,
            Err(SyncRuntimeError::Busy) => return RestoreRetryProgress::Complete,
            Err(error) => {
                self.notify_status(self.report_error(error));
                self.mark_restore_exhausted();
                return RestoreRetryProgress::Complete;
            }
        };
        self.record_restore_attempt(attempt);
        let restored = self.restore_locked();
        if !self.restore_retry_is_current(generation, root) {
            return RestoreRetryProgress::Complete;
        }
        match restored {
            Ok(_) => {
                self.clear_restore_retry_state();
                RestoreRetryProgress::Complete
            }
            Err(error) => {
                self.notify_status(self.report_restore_failure(error, attempt, max_attempts));
                if attempt == max_attempts {
                    self.mark_restore_exhausted();
                    RestoreRetryProgress::Complete
                } else {
                    RestoreRetryProgress::Failed
                }
            }
        }
    }

    /// Permanently fence producer admission before stopping the current session.
    /// A failed cleanup keeps the fence set; calling shutdown again retries cleanup.
    /// Ordinary `stop` remains restartable and does not set this fence.
    pub fn shutdown(&self) -> Result<SyncStatus, SyncRuntimeError> {
        let transition = self
            .transition
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        self.shutting_down.store(true, Ordering::Release);
        // A retry worker may be waiting for `transition`. Wake and detach it
        // before stop_inner, then join only after releasing that same mutex.
        let retry = self.take_restore_retry();
        let persisted = self
            .database
            .current_sync_configuration()
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)
            .and_then(|configuration| {
                if configuration.is_some() {
                    self.persist_session_intent(SyncSessionIntent::Stopped)
                } else {
                    Ok(())
                }
            });
        let stopped = self.stop_inner();
        drop(transition);
        let joined = retry.map_or(Ok(()), |worker| {
            worker
                .thread
                .join()
                .map_err(|_| SyncRuntimeError::WorkerFailed)
        });
        stopped?;
        joined?;
        persisted?;
        Ok(self.status())
    }

    pub(super) fn take_restore_retry(&self) -> Option<RestoreRetryWorker> {
        let mut retry = self.restore_retry.lock().ok()?;
        retry.generation.fetch_add(1, Ordering::AcqRel);
        retry.attempts = 0;
        let mut worker = retry.worker.take();
        drop(retry);
        if let Some(worker) = &mut worker {
            worker.wake.cancel();
        }
        worker
    }
}
