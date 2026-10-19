//! The periodic sync cycle driver.

#[allow(clippy::wildcard_imports)]
use super::*;

use super::control::{jittered_retry_delay, wait_for_sync_wakeup};

#[allow(clippy::too_many_lines)]
impl SyncRuntime {
    pub(super) fn start_sync_loop(&self, role: SyncRole) -> Result<(), SyncRuntimeError> {
        let cycle_port = self
            .cycle_port
            .clone()
            .ok_or(SyncRuntimeError::Unconfigured)?;
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        if inner.sync_loop.is_some() {
            inner.auto_sync = true;
            return Ok(());
        }
        if !inner.status.enabled || inner.status.role != role {
            return Err(SyncRuntimeError::Unconfigured);
        }
        let session_id = inner.status.session_id;
        let state = Arc::clone(&self.inner);
        let notifier = Arc::clone(&self.status_notifier);
        let cycle = Arc::clone(&self.cycle);
        let (wake, wakeups) = mpsc::channel();
        self.database.register_sync_waker(wake.clone());
        // Queue the first cycle before the worker starts so its initial drain
        // always sees it: sending after the spawn would race the worker's
        // first check, and a peer that went straight into a held watch would
        // then sit on this hint until the watch ended.
        let _ = wake.send(());
        let stop = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stop);
        let thread = thread::Builder::new()
            .name("echo-lan-sync-loop".into())
            .spawn(move || {
                // Local SQLite writes wake the loop immediately. Between
                // cycles a healthy peer holds one long-poll against the host,
                // which answers as soon as it publishes; the plain waker wait
                // remains for hosts, retrying peers, and hosts without the
                // wait capability.
                let mut retry_delay = RETRY_BASE_DELAY;
                let mut retry_attempts = 0_u32;
                let mut retrying = false;
                // A recycled keep-alive connection and an unreachable host are
                // one failed attempt each; only the second consecutive failure
                // is evidence of the latter, so the first one is retried
                // silently instead of being published as a sync failure.
                let mut watch_failures = 0_u32;
                loop {
                    // A pending local wakeup — including the initial one sent
                    // when the loop starts — is work right now. A retrying peer
                    // still lets its backoff run first, matching the plain
                    // waker's suppression of early retries.
                    let mut woke_locally = false;
                    if !retrying {
                        while wakeups.try_recv().is_ok() {
                            woke_locally = true;
                        }
                    }
                    if !woke_locally {
                        // Wait for work. Only a healthy peer watches: a host
                        // has nothing remote to discover, and a retrying peer
                        // must let its backoff run instead of holding a dead
                        // connection.
                        if role == SyncRole::Peer && !retrying {
                            match cycle_port
                                .watch_host_commits(Arc::clone(&worker_stop), PEER_WATCH_BUDGET)
                            {
                                Ok(WatchOutcome::Unsupported) => {
                                    watch_failures = 0;
                                    let delay = FALLBACK_POLL_INTERVAL;
                                    if !wait_for_sync_wakeup(
                                        &wakeups,
                                        delay,
                                        retrying,
                                        &worker_stop,
                                    ) {
                                        break;
                                    }
                                }
                                Ok(WatchOutcome::LocalWrite) => {
                                    watch_failures = 0;
                                    // The watch drained its own channel; clear
                                    // the loop's too so a later plain wait
                                    // cannot replay stale hints as extra cycles.
                                    while wakeups.try_recv().is_ok() {}
                                }
                                Ok(WatchOutcome::Advanced | WatchOutcome::BudgetElapsed) => {
                                    watch_failures = 0;
                                }
                                Err(SyncRuntimeError::Busy) => break,
                                Err(error) => {
                                    // Retry once without publishing: the host's
                                    // connection lifetime recycles idle
                                    // keep-alive sockets, which surfaces as a
                                    // transport error even though the peer is
                                    // healthy. A second consecutive failure is a
                                    // real outage and keeps the retry cadence.
                                    if watch_failures == 0 {
                                        watch_failures = 1;
                                        tracing::debug!(
                                            target: "echo_desktop::sync",
                                            error = ?error,
                                            "peer watch failed once; retrying before reporting"
                                        );
                                        continue;
                                    }
                                    // The host was unreachable: record it like a
                                    // failed cycle and let the retry cadence run.
                                    let keep_waiting = {
                                        let mut inner = state
                                            .lock()
                                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                                        if inner.status.session_id != session_id
                                            || !inner.status.enabled
                                        {
                                            false
                                        } else {
                                            inner.status.state = SyncState::Failed;
                                            inner.status.catalog_changed = false;
                                            inner.status.last_error =
                                                Some(error.message_key().into());
                                            inner.status.event_sequence += 1;
                                            true
                                        }
                                    };
                                    if !keep_waiting {
                                        break;
                                    }
                                    let status_notifier = notifier
                                        .lock()
                                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                                        .clone();
                                    if let Some(notifier) = status_notifier {
                                        let status = state
                                            .lock()
                                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                                            .status
                                            .clone();
                                        notifier(status);
                                    }
                                    retrying = true;
                                    if retry_attempts > 0 {
                                        retry_delay =
                                            (retry_delay * 2).min(Duration::from_secs(60));
                                    }
                                    retry_attempts = retry_attempts.saturating_add(1);
                                    continue;
                                }
                            }
                        } else {
                            let delay = if retrying {
                                jittered_retry_delay(retry_delay)
                            } else {
                                FALLBACK_POLL_INTERVAL
                            };
                            if !wait_for_sync_wakeup(&wakeups, delay, retrying, &worker_stop) {
                                break;
                            }
                        }
                    }
                    let should_run = {
                        let mut inner = state
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        if inner.status.session_id != session_id
                            || !inner.status.enabled
                            || inner.status.state == SyncState::Paused
                        {
                            false
                        } else {
                            inner.status.state = SyncState::Syncing;
                            inner.status.catalog_changed = false;
                            inner.status.last_error = None;
                            inner.status.event_sequence += 1;
                            true
                        }
                    };
                    if !should_run {
                        break;
                    }
                    let status_notifier = notifier
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clone();
                    if let Some(notifier) = status_notifier {
                        let status = state
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .status
                            .clone();
                        notifier(status);
                    }
                    if worker_stop.load(Ordering::Acquire) {
                        break;
                    }
                    let (result, progress) = {
                        let Ok(_cycle) = cycle.lock() else {
                            break;
                        };
                        if worker_stop.load(Ordering::Acquire) {
                            break;
                        }
                        run_observed_cycle(
                            cycle_port.as_ref(),
                            role,
                            Arc::clone(&worker_stop),
                            session_id,
                            Arc::clone(&state),
                            Arc::clone(&notifier),
                        )
                    };
                    if worker_stop.load(Ordering::Acquire) {
                        break;
                    }
                    {
                        let mut inner = state
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        if inner.status.session_id != session_id || !inner.status.enabled {
                            break;
                        }
                        inner.status.event_sequence += 1;
                        progress.apply(&mut inner.status);
                        match result {
                            Ok(catalog_changed) => {
                                inner.status.catalog_changed = catalog_changed;
                                inner.status.state = if role == SyncRole::Host {
                                    SyncState::Hosting
                                } else {
                                    SyncState::Connected
                                };
                                inner.status.last_error = None;
                                retrying = false;
                                retry_delay = RETRY_BASE_DELAY;
                                retry_attempts = 0;
                            }
                            Err(error) => {
                                inner.status.state = SyncState::Failed;
                                inner.status.catalog_changed = false;
                                inner.status.last_error = Some(error.message_key().into());
                                retrying = true;
                                if retry_attempts > 0 {
                                    retry_delay = (retry_delay * 2).min(Duration::from_secs(60));
                                }
                                retry_attempts = retry_attempts.saturating_add(1);
                            }
                        }
                        drop(inner);
                    }
                    let status_notifier = notifier
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clone();
                    if let Some(notifier) = status_notifier {
                        let status = state
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .status
                            .clone();
                        notifier(status);
                    }
                }
            })
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        inner.auto_sync = true;
        inner.sync_loop = Some(SyncLoopWorker {
            stop,
            wake,
            thread: Some(thread),
        });
        drop(inner);
        Ok(())
    }
}
