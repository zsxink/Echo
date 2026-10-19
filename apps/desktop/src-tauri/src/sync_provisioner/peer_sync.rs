//! LAN peer synchronization cycle methods split from the former monolithic
//! `sync_provisioner.rs`: pull host changes, peer library merge and the
//! incremental sync cycle.

#[allow(clippy::wildcard_imports)] // 拆分自单文件：继承父模块导入域
use super::*;

impl NativeSyncProvisioner {
    pub(super) fn synchronize_peer(
        &self,
        root: LibraryRootId,
        cancelled: &Arc<AtomicBool>,
    ) -> Result<(), SyncRuntimeError> {
        let configuration = self
            .database
            .current_sync_configuration()
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
            .ok_or(SyncRuntimeError::Unconfigured)?;
        let stored = OsSecureStore
            .get(&peer_identity_handle(configuration.binding, root)?)
            .map_err(|_| SyncRuntimeError::Unconfigured)?;
        let identity: PeerIdentity =
            serde_json::from_slice(stored.expose()).map_err(|_| SyncRuntimeError::Unconfigured)?;
        if identity.binding != configuration.binding
            || identity.local_root != root
            || identity.local_device != configuration.local_device
        {
            return Err(SyncRuntimeError::Unconfigured);
        }
        validate_peer_endpoint(&identity.endpoint)?;
        let peer = ProvisionedPeer {
            binding: identity.binding,
            address: identity.endpoint,
            client: LanClient::new(&identity.host_certificate_der, identity.bearer_token)
                .map_err(|_| SyncRuntimeError::Unconfigured)?,
            host_device_id: identity.host_device,
        };
        let library = self
            .database
            .by_id(root)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
            .ok_or(SyncRuntimeError::LibraryUnavailable)?;
        if !library.is_active() || library.availability() != RootAvailability::Available {
            return Err(SyncRuntimeError::LibraryUnavailable);
        }
        // The cache is for received resources only. Local media remains in its
        // authoritative library and is opened directly when upload chunks are
        // requested.
        let resources = self.local_resource_store(peer.binding)?;
        let endpoint = Url::parse(&peer.address).map_err(|_| SyncRuntimeError::InvalidAddress)?;
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        let result = executor.block_on(async {
            tokio::select! {
                biased;
                () = wait_for_sync_cancel(Arc::clone(cancelled)) => Err(SyncRuntimeError::Busy),
                result = async {
                    self.merge_peer_library(&peer, &endpoint, &resources, root).await
                } => result,
            }
        });
        if let Err(error) = resources.clear_binding(peer.binding) {
            tracing::warn!(
                target: "echo_desktop::sync",
                %error,
                "peer sync transfer cache cleanup failed"
            );
        }
        self.uploaded_resources
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .retain(|(binding, _)| *binding != peer.binding);
        result
    }

    /// Register this provisioner's own sync wake-up channel on first use and
    /// drain whatever queued. The writer actor fans wakeups out to every
    /// listener, so this never consumes the runtime loop's hints.
    pub(super) fn take_local_wake(&self) -> bool {
        let mut channel = self
            .local_wakes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if channel.is_none() {
            let (sender, receiver) = mpsc::channel();
            self.database.register_sync_waker(sender.clone());
            *channel = Some(LocalWakeChannel {
                _sender: sender,
                receiver,
            });
        }
        channel
            .as_ref()
            .is_some_and(LocalWakeChannel::drain_pending)
    }

    pub(super) async fn merge_peer_library(
        &self,
        peer: &ProvisionedPeer,
        endpoint: &Url,
        resources: &Arc<FileResourceStore>,
        root: LibraryRootId,
    ) -> Result<(), SyncRuntimeError> {
        self.enroll_peer_snapshot(peer, endpoint).await?;
        let queue = ScopedLocalQueue {
            database: self.database.as_ref(),
            root,
        };
        loop {
            let pending_before = queue
                .pending_local_mutations(1)
                .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
                .first()
                .map(|mutation| mutation.mutation_id);
            // Merge the complete host history before publishing local
            // additions. With nothing local to publish the ordering cannot
            // matter, and this pull is the redundant half of an idle cycle:
            // `synchronize_once` pulls after pushing, and the closing pull
            // covers anything that landed after it. Skipping it drops a steady
            // idle cycle from three pulls to one.
            if pending_before.is_some() {
                self.pull_host_changes(peer, endpoint, &queue).await?;
            }
            self.publish_remote_resources(peer, endpoint, root, resources)
                .await?;
            self.upload_pending_resources(peer, endpoint, root).await?;
            let before = queue
                .replica_checkpoint(peer.host_device_id)
                .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
            let pushed = self
                .prepared_incremental_cycle(peer, endpoint, &queue)
                .await?;
            self.publish_remote_resources(peer, endpoint, root, resources)
                .await?;
            let pending_after = queue
                .pending_local_mutations(1)
                .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
            if pending_after.is_empty() {
                // A cycle that pushed nothing already applied the post-push
                // head, which is what the closing pull would re-read. A
                // saturated page or a dropped ACK is what still needs it.
                if pushed {
                    self.pull_host_changes(peer, endpoint, &queue).await?;
                    self.publish_remote_resources(peer, endpoint, root, resources)
                        .await?;
                }
                break;
            }
            // A terminally rejected mutation is removed from the pending queue
            // even when the host commit checkpoint does not change. Treat that
            // as progress so the next mutation can continue its origin chain.
            if pending_before != pending_after.first().map(|mutation| mutation.mutation_id) {
                continue;
            }
            let after = queue
                .replica_checkpoint(peer.host_device_id)
                .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
            if after == before {
                if let Some(mutation) = pending_after.first() {
                    tracing::warn!(
                        target: "echo_desktop::sync",
                        mutation_id = ?mutation.mutation_id,
                        origin_device_id = ?mutation.origin_device_id,
                        origin_seq = mutation.origin_seq,
                        previous_mutation_id = ?mutation.previous_mutation_id,
                        host_checkpoint = after.sequence.as_u64(),
                        "peer sync made no progress; host did not accept the pending mutation"
                    );
                }
                return Err(SyncRuntimeError::SyncPredecessorPending);
            }
        }
        if let Err(error) = resources.cleanup_completed(peer.binding) {
            tracing::warn!(
                target: "echo_desktop::sync",
                %error,
                "completed transfer resource cleanup failed"
            );
        }
        self.reconcile_remote_delete_isolations(root);
        Ok(())
    }

    /// Apply every published host page. Catalog pages are staged immediately:
    /// Core persists the objects, their shared manifests and the cursor in one
    /// transaction, and materializes confirmed songs as unavailable. Media is
    /// attached afterwards from those durable manifests
    /// (`publish_remote_resources`), so a slow or stalled transfer can never
    /// park an object update behind it — the guarantee the "对象先于媒体到达"
    /// scenario requires, and the order snapshot enrollment already uses.
    pub(super) async fn pull_host_changes(
        &self,
        peer: &ProvisionedPeer,
        endpoint: &Url,
        queue: &ScopedLocalQueue<'_>,
    ) -> Result<(), SyncRuntimeError> {
        loop {
            let checkpoint = queue
                .replica_checkpoint(peer.host_device_id)
                .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
            let page = peer
                .client
                .pull_commits(
                    endpoint,
                    checkpoint.sequence.as_u64(),
                    checkpoint.digest,
                    512,
                    Duration::ZERO,
                )
                .await
                .map_err(map_sync_client_error)?;
            if page.binding != peer.binding {
                return Err(SyncRuntimeError::LibraryUnavailable);
            }
            let finished = page.entries.is_empty();
            let prepared = PreparedRemoteApply {
                database: self.database.as_ref(),
                host: peer.host_device_id,
                changed: AtomicBool::new(false),
                catalog_changed: &self.pending_catalog_changed,
            };
            LanClient::apply_commit_page(&page, peer.host_device_id, &prepared)
                .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
            if finished {
                return Ok(());
            }
        }
    }

    /// Hold one long-poll against an already provisioned host until it
    /// publishes, a local write queues work, the budget elapses, or
    /// cancellation is requested. Only the peer role watches: a host has
    /// nothing remote to discover. The page is a change detector — the next
    /// cycle's catch-up pull applies what the host published.
    pub(super) async fn watch_peer_commits(
        &self,
        peer: &ProvisionedPeer,
        root: LibraryRootId,
        cancelled: Arc<AtomicBool>,
        wait: Duration,
    ) -> Result<WatchOutcome, SyncRuntimeError> {
        let queue = ScopedLocalQueue {
            database: self.database.as_ref(),
            root,
        };
        let checkpoint = queue
            .replica_checkpoint(peer.host_device_id)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        if checkpoint.binding != peer.binding {
            return Err(SyncRuntimeError::LibraryUnavailable);
        }
        // A local write that committed before the watch started is work too:
        // consume it now instead of holding the connection for the budget.
        if self.take_local_wake() {
            return Ok(WatchOutcome::LocalWrite);
        }
        let endpoint = Url::parse(&peer.address).map_err(|_| SyncRuntimeError::InvalidAddress)?;
        let mut watch = Box::pin(peer.client.pull_commits(
            &endpoint,
            checkpoint.sequence.as_u64(),
            checkpoint.digest,
            1,
            wait,
        ));
        let started = Instant::now();
        let mut ticker = tokio::time::interval(Duration::from_millis(250));
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                biased;
                () = wait_for_sync_cancel(Arc::clone(&cancelled)) => {
                    return Err(SyncRuntimeError::Busy);
                }
                _ = ticker.tick() => {
                    if self.take_local_wake() {
                        return Ok(WatchOutcome::LocalWrite);
                    }
                }
                page = &mut watch => {
                    let page = page.map_err(map_sync_client_error)?;
                    if page.entries.is_empty() && started.elapsed() < wait / 2 {
                        // The host answered long before the budget: it does not
                        // hold the page. Report the missing capability so the
                        // caller returns to plain polling instead of
                        // re-issuing immediately.
                        return Ok(WatchOutcome::Unsupported);
                    }
                    return Ok(if page.entries.is_empty() {
                        WatchOutcome::BudgetElapsed
                    } else {
                        WatchOutcome::Advanced
                    });
                }
            }
        }
    }

    /// Run one push/pull cycle whose pull page is applied by `PreparedRemoteApply`.
    ///
    /// Returns whether the cycle submitted anything to the host — an accepted or
    /// rejected mutation grows the host log under our own ACK, and a saturated
    /// page leaves more to read — which is what tells the caller a closing pull
    /// is still required.
    async fn prepared_incremental_cycle(
        &self,
        peer: &ProvisionedPeer,
        endpoint: &Url,
        queue: &ScopedLocalQueue<'_>,
    ) -> Result<bool, SyncRuntimeError> {
        // A push can append commits while the batch uploads. `synchronize_once`
        // pulls and applies the resulting page itself, so no second prefetch is
        // needed to stage it.
        let prepared = PreparedRemoteApply {
            database: self.database.as_ref(),
            host: peer.host_device_id,
            changed: std::sync::atomic::AtomicBool::new(false),
            catalog_changed: &self.pending_catalog_changed,
        };
        let result = echo_sync::replica::synchronize_once(
            &peer.client,
            endpoint,
            peer.binding,
            peer.host_device_id,
            queue,
            &prepared,
            512,
        )
        .await;
        match result {
            Ok(report) => Ok(report.mutations_confirmed > 0
                || report.mutations_rejected > 0
                || report.pull_may_continue),
            Err(_) if prepared.changed.load(Ordering::Relaxed) => {
                // Apply newly accepted commits before selecting another push
                // batch, so accepted local mutations are not uploaded twice.
                self.pull_host_changes(peer, endpoint, queue).await?;
                Ok(true)
            }
            Err(error) => Err(match error {
                echo_sync::replica::ReplicaSyncError::Client(error) => map_sync_client_error(error),
                echo_sync::replica::ReplicaSyncError::Core(error) => {
                    // A local ledger failure is not a missing music directory.
                    // Collapsing it into `LibraryUnavailable` used to leave the
                    // user with a wrong instruction and no log line at all, so
                    // the actual cause was unrecoverable after the fact.
                    tracing::warn!(
                        target: "echo_desktop::sync",
                        %error,
                        "local sync ledger failed during a peer cycle"
                    );
                    SyncRuntimeError::SyncLocalStateFailed
                }
                other => {
                    tracing::warn!(
                        target: "echo_desktop::sync",
                        error = %other,
                        "peer sync cycle failed"
                    );
                    SyncRuntimeError::SyncProtocolFailed
                }
            }),
        }
    }
}
