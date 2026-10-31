//! `SyncProvisioner` port impl for `NativeSyncProvisioner`.

#[allow(clippy::wildcard_imports, unused_imports)]
use super::*;
use crate::sync_runtime::{SyncCyclePort, SyncProvisioner};

impl SyncProvisioner for NativeSyncProvisioner {
    fn host(&self, root: LibraryRootId) -> Result<ProvisionedHost, SyncRuntimeError> {
        let _guard = self
            .gate
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        let library = self
            .database
            .by_id(root)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
            .ok_or(SyncRuntimeError::LibraryUnavailable)?;
        if !library.is_active() || library.availability() != RootAvailability::Available {
            return Err(SyncRuntimeError::LibraryUnavailable);
        }
        let identity = self.host_identity(root)?;
        let checkpoint = self.recover_checkpoint(identity.binding)?;
        self.database
            .ensure_host_sync_baseline(identity.binding, root)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        let mut credentials = Vec::new();
        for (device_id, handle) in self
            .database
            .authorized_sync_peer_handles()
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
        {
            let handle = SecretHandle::new(handle).map_err(|_| SyncRuntimeError::Unconfigured)?;
            let token = OsSecureStore
                .get(&handle)
                .map_err(|_| SyncRuntimeError::Unconfigured)?;
            credentials.push(PeerCredential {
                device_id,
                bearer_token: token.expose().to_vec(),
            });
        }
        // Open the private cache without reading every local audio file.
        // Resources are materialized and hash-verified on their first read.
        let resources = self.local_resource_store(identity.binding)?;
        self.confirm_local(identity.binding)?;
        self.publish_cached_peer_resources(root, identity.binding, &resources)?;
        let host_resources = Arc::new(lazy_host_resources::LazyHostResources::new(
            Arc::clone(&self.database),
            library,
            identity.binding,
            Arc::clone(&resources),
        ));
        let api = HostApiState::new(
            identity.binding,
            credentials,
            self.database.clone(),
            checkpoint,
        )
        .map_err(|_| SyncRuntimeError::Unconfigured)?
        .with_authorization(self.database.clone())
        .with_snapshots(self.database.clone())
        .with_resources(host_resources)
        .with_pairing(
            Arc::new(PairingRegistry::new(identity.binding)),
            Arc::new(DurableHostPairing::new(self.database.clone())),
        );
        let certificate_der = identity.certificate_der.clone();
        let tls = tls13_server_config(
            vec![CertificateDer::from(identity.certificate_der)],
            PrivatePkcs8KeyDer::from(identity.private_key_der).into(),
        )
        .map_err(|_| SyncRuntimeError::Unconfigured)?;
        let device_id = self.database.current_device_id().as_uuid().to_string();
        *self
            .host_seen_checkpoint
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)? = Some(
            self.database
                .latest_published_sync_checkpoint(identity.binding)
                .map_err(|_| SyncRuntimeError::LibraryUnavailable)?,
        );
        Ok(ProvisionedHost {
            binding: identity.binding,
            api,
            tls,
            certificate_der,
            advertisement_name: format!("Echo-{}", &device_id[..8]),
            device_id,
        })
    }

    fn peer(&self, root: LibraryRootId) -> Result<ProvisionedPeer, SyncRuntimeError> {
        let _guard = self
            .gate
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        self.database
            .select_sync_context(root)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        let configuration = self
            .database
            .current_sync_configuration()
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
            .ok_or(SyncRuntimeError::Unconfigured)?;
        if configuration.host || configuration.local_root != root {
            return Err(SyncRuntimeError::LibraryUnavailable);
        }
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
        Ok(ProvisionedPeer {
            binding: identity.binding,
            address: identity.endpoint,
            host_device_id: identity.host_device,
        })
    }

    fn revoke_device(&self, device_id: &str) -> Result<(), SyncRuntimeError> {
        let _guard = self
            .gate
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        let device_id: DeviceId =
            serde_json::from_value(serde_json::Value::String(device_id.to_owned()))
                .map_err(|_| SyncRuntimeError::Unconfigured)?;
        self.database
            .revoke_sync_peer(device_id)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)
    }

    fn leave_sync(&self, root: LibraryRootId) -> Result<(), SyncRuntimeError> {
        let _guard = self
            .gate
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        self.leave_sync_locked(root)
    }

    fn recover_restored_host(
        &self,
        root: LibraryRootId,
    ) -> Result<echo_core::application::ports::HostRecoveryArchive, SyncRuntimeError> {
        self.recover_restored_host(root)
    }

    #[allow(clippy::too_many_lines)]
    fn complete_peer_pairing(
        &self,
        root: LibraryRootId,
        invitation: &PairingInvitation,
        endpoint: &str,
        grant: &PairingGrantDto,
    ) -> Result<(), SyncRuntimeError> {
        let _guard = self
            .gate
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        let _activity = self
            .scan_deps
            .activity
            .exclusive()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        validate_peer_endpoint(endpoint)?;
        let local_device = DeviceId::from_uuid(self.database.current_device_id().as_uuid());
        if grant.binding != invitation.binding
            || grant.device_id != local_device
            || invitation.host_device_id == local_device
            || !(32..=256).contains(&grant.bearer_token.len())
            || !grant
                .bearer_token
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric())
        {
            return Err(SyncRuntimeError::Unconfigured);
        }
        let current_configuration = self
            .database
            .current_sync_configuration()
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        let library = self
            .database
            .by_id(root)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
            .ok_or(SyncRuntimeError::LibraryUnavailable)?;
        if !library.is_active() || library.availability() != RootAvailability::Available {
            return Err(SyncRuntimeError::LibraryUnavailable);
        }
        // Keep the local catalog and media in place throughout enrollment.
        prepare_enrollment_root(library.absolute_path(), root)?;
        LanClient::new(&invitation.host_certificate_der, grant.bearer_token.clone())
            .map_err(|_| SyncRuntimeError::Unconfigured)?;
        let identity = PeerIdentity {
            binding: invitation.binding,
            local_root: root,
            local_device,
            host_device: invitation.host_device_id,
            generation: invitation.invitation_id.clone(),
            endpoint: endpoint.to_owned(),
            host_certificate_der: invitation.host_certificate_der.clone(),
            bearer_token: grant.bearer_token.clone(),
        };
        let handle = peer_identity_handle(invitation.binding, root)?;
        let secret = SecretBytes::new(
            serde_json::to_vec(&identity).map_err(|_| SyncRuntimeError::Unconfigured)?,
        );
        OsSecureStore
            .set(&handle, &secret)
            .map_err(|_| SyncRuntimeError::Unconfigured)?;
        let already_bound_to_this_host = current_configuration.as_ref().is_some_and(|current| {
            !current.host && current.binding == invitation.binding && current.local_root == root
        });
        let replaced_existing_binding =
            current_configuration.is_some() && !already_bound_to_this_host;
        if replaced_existing_binding {
            if let Err(error) = self.leave_sync_locked(root) {
                if let Err(cleanup_error) = remove_peer_identity_if_generation(&handle, &identity) {
                    tracing::error!(
                        target: "echo_desktop::sync",
                        ?cleanup_error,
                        generation = %identity.generation,
                        "failed to compensate peer identity after replacing the sync binding failed"
                    );
                }
                return Err(error);
            }
        }
        if let Err(error) = self
            .database
            .bind_sync_replica_for_enrollment(invitation.binding, root)
        {
            if let Err(cleanup_error) = remove_peer_identity_if_generation(&handle, &identity) {
                tracing::error!(
                    target: "echo_desktop::sync",
                    ?cleanup_error,
                    generation = %identity.generation,
                    "failed to compensate peer identity after binding the sync replica failed"
                );
            }
            tracing::error!(
                target: "echo_desktop::sync",
                code = error.code(),
                root = %root,
                "binding the sync replica failed"
            );
            return Err(SyncRuntimeError::LibraryUnavailable);
        }
        if let Err(error) = self
            .database
            .ensure_replica_sync_baseline(invitation.binding, root)
        {
            // The identity is usable only after the local baseline transaction
            // succeeds. Keep the exact database binding so a retry can resume
            // without deleting local outbox rows or media.
            if let Err(cleanup_error) = remove_peer_identity_if_generation(&handle, &identity) {
                tracing::error!(
                    target: "echo_desktop::sync",
                    ?cleanup_error,
                    generation = %identity.generation,
                    "failed to compensate peer identity after baseline initialization failed"
                );
            }
            tracing::error!(
                target: "echo_desktop::sync",
                ?error,
                generation = %identity.generation,
                "replica baseline initialization failed"
            );
            return Err(SyncRuntimeError::LibraryUnavailable);
        }
        Ok(())
    }
}

impl SyncCyclePort for NativeSyncProvisioner {
    fn sync_now(&self, role: SyncRole) -> Result<bool, SyncRuntimeError> {
        self.sync_now_cancellable(role, Arc::new(AtomicBool::new(false)))
    }

    fn sync_progress(&self, role: SyncRole) -> Result<SyncProgress, SyncRuntimeError> {
        let configuration = self
            .database
            .current_sync_configuration()
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
            .ok_or(SyncRuntimeError::Unconfigured)?;
        let (pending_changes, resources) = self
            .database
            .pending_local_sync_summary()
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        let mut progress = SyncProgress {
            pending_changes: Some(pending_changes),
            ..SyncProgress::default()
        };
        if role == SyncRole::Peer && !configuration.host {
            progress.pending_uploads = Some(
                resources
                    .iter()
                    .filter(|resource| {
                        !self.is_uploaded(configuration.binding, &resource.digest.to_hex())
                    })
                    .count() as u64,
            );
            // Enrollment has to finish before a local manifest inventory can
            // prove completeness. While pages remain unknown, leave both
            // download count and media readiness absent.
            let peer = self.peer(configuration.local_root)?;
            if self
                .database
                .replica_enrollment_complete(peer.host_device_id)
                .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
            {
                let sets = self
                    .database
                    .pending_remote_resource_sets(configuration.local_root)
                    .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
                progress.media_ready = Some(sets.is_empty());
                let store = self.local_resource_store(configuration.binding)?;
                let pending: HashSet<_> = sets
                    .iter()
                    .flat_map(|(_, targets)| targets)
                    .filter(|target| {
                        store
                            .verify_complete(
                                configuration.binding,
                                &ResourceRefDto::from(&target.manifest),
                            )
                            .is_err()
                    })
                    .map(|target| target.manifest.digest.to_hex())
                    .collect();
                progress.pending_downloads = Some(pending.len() as u64);
            }
            let (total_upload, uploaded, total_download, downloaded) = self
                .telemetry()
                .lock()
                .map_err(|_| SyncRuntimeError::WorkerFailed)?
                .byte_totals();
            progress.total_upload_bytes = Some(total_upload);
            progress.uploaded_bytes = Some(uploaded);
            progress.total_download_bytes = Some(total_download);
            progress.downloaded_bytes = Some(downloaded);
        }
        if role == SyncRole::Host && configuration.host {
            // This is the host's own media readiness. Accepted peer commits
            // can still be awaiting local publication even after their upload
            // and ledger confirmation have completed.
            progress.media_ready = Some(
                self.database
                    .pending_peer_resource_sets(configuration.local_root)
                    .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
                    .is_empty(),
            );
        }
        // A host cannot know which resources its peers still need. Host HTTP
        // traffic is not interpreted as a complete peer download inventory.
        Ok(progress)
    }

    fn sync_now_cancellable(
        &self,
        role: SyncRole,
        cancelled: Arc<AtomicBool>,
    ) -> Result<bool, SyncRuntimeError> {
        self.sync_now_with_progress(role, cancelled, None)
    }

    fn sync_now_with_progress(
        &self,
        role: SyncRole,
        cancelled: Arc<AtomicBool>,
        progress: Option<ProgressNotifier>,
    ) -> Result<bool, SyncRuntimeError> {
        let _guard = self
            .gate
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        // A cancelled download may still finish its blocking store write.
        // Give every cycle a separate collector so a late checkpoint cannot
        // contaminate a subsequent cycle or a different library root.
        *self
            .transfer_telemetry
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)? =
            Arc::new(Mutex::new(TransferTelemetry::new(progress)));
        let configuration = self
            .database
            .current_sync_configuration()
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
            .ok_or(SyncRuntimeError::Unconfigured)?;
        if role == SyncRole::Peer && !configuration.host {
            self.retry_retired_cache_cleanups();
            self.synchronize_peer(configuration.local_root, &cancelled)?;
            self.reconcile_remote_delete_isolations(configuration.local_root);
            self.refresh_published_metadata(configuration.local_root);
            return Ok(self.pending_catalog_changed.swap(false, Ordering::AcqRel));
        }
        if role != SyncRole::Host || !configuration.host {
            return Err(SyncRuntimeError::LibraryUnavailable);
        }
        let library = self
            .database
            .by_id(configuration.local_root)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
            .ok_or(SyncRuntimeError::LibraryUnavailable)?;
        if !library.is_active() || library.availability() != RootAvailability::Available {
            return Err(SyncRuntimeError::LibraryUnavailable);
        }
        if cancelled.load(Ordering::Acquire) {
            if let Ok(resources) = self.local_resource_store(configuration.binding) {
                if let Err(error) = resources.clear_binding(configuration.binding) {
                    tracing::warn!(
                        target: "echo_desktop::sync",
                        %error,
                        "cancelled host sync transfer cache cleanup failed"
                    );
                }
            }
            return Err(SyncRuntimeError::Busy);
        }
        let resources = self.local_resource_store(configuration.binding)?;
        let result = (|| {
            // Converge any cache partition whose binding was already retired
            // by an earlier detach, including one from a previous process.
            self.retry_retired_cache_cleanups();
            self.confirm_local(configuration.binding)?;
            self.publish_cached_peer_resources(
                configuration.local_root,
                configuration.binding,
                &resources,
            )?;
            self.reconcile_remote_delete_isolations(configuration.local_root);
            self.refresh_published_metadata(configuration.local_root);
            let checkpoint = self
                .database
                .latest_published_sync_checkpoint(configuration.binding)
                .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
            {
                let mut seen = self
                    .host_seen_checkpoint
                    .lock()
                    .map_err(|_| SyncRuntimeError::WorkerFailed)?;
                if seen
                    .as_ref()
                    .is_some_and(|previous| *previous != checkpoint)
                {
                    self.pending_catalog_changed.store(true, Ordering::Release);
                }
                *seen = Some(checkpoint);
            }
            Ok(self.pending_catalog_changed.swap(false, Ordering::AcqRel))
        })();
        result
    }

    /// Hold one long-poll against the paired host until it publishes new
    /// commits, a local write queues work, the budget elapses, or cancellation
    /// is requested. This is a change detector only: the caller runs a full
    /// cycle afterwards, whose catch-up pull applies whatever the host
    /// published. Peer-only — a host has nothing remote to discover because
    /// peers push their own work to it.
    fn watch_host_commits(
        &self,
        cancelled: Arc<AtomicBool>,
        wait: Duration,
    ) -> Result<WatchOutcome, SyncRuntimeError> {
        let configuration = self
            .database
            .current_sync_configuration()
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
            .ok_or(SyncRuntimeError::Unconfigured)?;
        if configuration.host {
            return Ok(WatchOutcome::Unsupported);
        }
        let peer = self.peer(configuration.local_root)?;
        let stored = OsSecureStore
            .get(&peer_identity_handle(
                configuration.binding,
                configuration.local_root,
            )?)
            .map_err(|_| SyncRuntimeError::Unconfigured)?;
        let identity: PeerIdentity =
            serde_json::from_slice(stored.expose()).map_err(|_| SyncRuntimeError::Unconfigured)?;
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        let client = LanClient::new(&identity.host_certificate_der, identity.bearer_token)
            .map_err(|_| SyncRuntimeError::Unconfigured)?;
        executor.block_on(self.watch_peer_commits(
            &client,
            &peer,
            configuration.local_root,
            cancelled,
            wait,
        ))
    }
}
