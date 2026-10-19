//! Peer-side resource publication methods split from the former
//! monolithic `sync_provisioner.rs`: cached publication, fresh uploads,
//! staging, snapshot enrollment and legacy-replica cleanup.

#[allow(clippy::wildcard_imports)] // 拆分自单文件：继承父模块导入域
use super::*;

impl NativeSyncProvisioner {
    pub(super) fn publish_cached_peer_resources(
        &self,
        root: LibraryRootId,
        binding: SyncBinding,
        resources: &Arc<FileResourceStore>,
    ) -> Result<(), SyncRuntimeError> {
        let sets = self
            .database
            .pending_peer_resource_sets(root)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        // Recover cache cleanup left behind by an earlier partial failure.
        // Keep hashes still awaiting publication; every other resource already
        // declared in the library has an authoritative local copy.
        let mut pending_hashes = HashMap::<String, usize>::new();
        for target in sets.iter().flat_map(|(_, targets)| targets.iter()) {
            *pending_hashes
                .entry(target.manifest.digest.to_hex())
                .or_default() += 1;
        }
        let published_hashes: Vec<String> = self
            .database
            .local_library_resource_manifests(root)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
            .into_iter()
            .map(|manifest| manifest.digest.to_hex())
            .filter(|hash| !pending_hashes.contains_key(hash))
            .collect();
        if let Err(error) = resources.cleanup_resources(binding, &published_hashes) {
            tracing::warn!(
                target: "echo_desktop::sync",
                %error,
                "previously published transfer resource cleanup failed"
            );
        }
        for (object, targets) in sets {
            let database = Arc::clone(&self.database);
            if let Err(error) = publish_remote_resource_set(
                self.publication.clone(),
                database.as_ref(),
                resources,
                binding,
                root,
                object,
                &targets,
            ) {
                // Each object has its own durable publication journal. An old
                // missing upload or a conflicting local path must not block
                // every later accepted song, nor prevent their metadata/UI
                // refresh. Keep this declaration pending for the next pass.
                tracing::warn!(
                    target: "echo_desktop::sync",
                    ?object,
                    ?error,
                    "peer resource publication deferred; continuing other objects"
                );
                continue;
            }
            let mut published_hashes = Vec::new();
            for target in &targets {
                let hash = target.manifest.digest.to_hex();
                if let Some(remaining) = pending_hashes.get_mut(&hash) {
                    *remaining -= 1;
                    if *remaining == 0 {
                        pending_hashes.remove(&hash);
                        published_hashes.push(hash);
                    }
                }
            }
            if let Err(error) = resources.cleanup_resources(binding, &published_hashes) {
                tracing::warn!(
                    target: "echo_desktop::sync",
                    %error,
                    "published transfer resource cleanup failed"
                );
            }
            self.pending_catalog_changed.store(true, Ordering::Release);
        }
        let local_owner = DeviceId::from_uuid(self.database.current_device_id().as_uuid());
        if let Err(error) = resources.cleanup_owner(binding, local_owner) {
            tracing::warn!(
                target: "echo_desktop::sync",
                %error,
                "host-local transfer cache cleanup failed"
            );
        }
        let referenced_hashes: Vec<String> = self
            .database
            .local_library_resource_manifests(root)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
            .into_iter()
            .map(|manifest| manifest.digest.to_hex())
            .collect();
        if let Err(error) = resources.cleanup_stale(
            binding,
            &referenced_hashes,
            Duration::from_secs(24 * 60 * 60),
        ) {
            tracing::warn!(
                target: "echo_desktop::sync",
                %error,
                "stale unreferenced transfer cache cleanup failed"
            );
        }
        Ok(())
    }

    pub(super) async fn publish_remote_resources(
        &self,
        peer: &ProvisionedPeer,
        endpoint: &Url,
        root: LibraryRootId,
        resources: &Arc<FileResourceStore>,
    ) -> Result<(), SyncRuntimeError> {
        let sets = self
            .database
            .pending_remote_resource_sets(root)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        let library = self
            .database
            .by_id(root)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
            .ok_or(SyncRuntimeError::LibraryUnavailable)?;
        // Stage the whole page before publishing any of it, two resources at a
        // time: the design's "two resource streams per session". Downloading
        // one resource per object and publishing between them made a page of
        // small songs cost one round trip plus one journal append each, which
        // is what pushed media-bearing changes well past the one-second
        // discovery promise. Publication stays in page order because the
        // journal appends one operation per object. A staging failure leaves
        // the page unpublished for the next cycle, which is idempotent: the
        // blobs that did arrive come back as `prepare`-complete.
        let mut window = sets.iter().flat_map(|(_object, targets)| targets.iter());
        while let Some(first) = window.next() {
            if let Some(second) = window.next() {
                let (first, second) = tokio::join!(
                    self.stage_remote_resource(peer, endpoint, &library, first, resources),
                    self.stage_remote_resource(peer, endpoint, &library, second, resources),
                );
                first?;
                second?;
            } else {
                self.stage_remote_resource(peer, endpoint, &library, first, resources)
                    .await?;
            }
        }
        for (object, targets) in sets {
            let deps = self.publication.clone();
            let database = Arc::clone(&self.database);
            let store = Arc::clone(resources);
            let binding = peer.binding;
            let catalog_changed = Arc::clone(&self.pending_catalog_changed);
            tokio::task::spawn_blocking(move || {
                publish_remote_resource_set(
                    deps,
                    database.as_ref(),
                    &store,
                    binding,
                    root,
                    object,
                    &targets,
                )?;
                // Blocking publication may complete after the cancellable
                // transport future has been dropped. Retain that work too.
                catalog_changed.store(true, Ordering::Release);
                Ok::<(), SyncRuntimeError>(())
            })
            .await
            .map_err(|_| SyncRuntimeError::WorkerFailed)??;
        }
        Ok(())
    }

    /// A current declaration can come from a local tag edit whose bytes have
    /// not reached the host yet. Reuse only full length/hash verified local
    /// content; caching it never acknowledges its pending mutation or upload.
    pub(super) async fn stage_remote_resource(
        &self,
        peer: &ProvisionedPeer,
        endpoint: &Url,
        library: &LibraryRoot,
        target: &RemoteResourceTarget,
        resources: &Arc<FileResourceStore>,
    ) -> Result<(), SyncRuntimeError> {
        let owner = DeviceId::from_uuid(self.database.current_device_id().as_uuid());
        let binding = peer.binding;
        let database = Arc::clone(&self.database);
        let library = library.clone();
        let manifest = target.manifest.clone();
        let store = Arc::clone(resources);
        let reused = tokio::task::spawn_blocking(move || {
            let request = PrepareResource {
                binding,
                resource: ResourceRefDto::from(&manifest),
            };
            if store.verify_complete(binding, &request.resource).is_ok() {
                return Ok(true);
            }
            let sources = local_resource_candidates(database.as_ref(), &library, &manifest)?;
            let Some(mut source) =
                verified_local_resource(sources, &manifest.digest.to_hex(), manifest.length)?
            else {
                return Ok(false);
            };
            // Snapshot mutable library bytes before publishing a completion;
            // source changes must not poison resumable network checkpoints.
            store
                .import_local_reader(owner, &request, &mut source)
                .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
            Ok::<bool, SyncRuntimeError>(true)
        })
        .await
        .map_err(|_| SyncRuntimeError::WorkerFailed)??;
        if reused {
            return Ok(());
        }
        observe_transfer(&self.telemetry(), |telemetry| {
            telemetry.declare_download(&ResourceRefDto::from(&target.manifest));
        });
        let tracked: Arc<dyn ResourceTransferPort> = Arc::new(TrackedDownloadStore {
            store: Arc::clone(resources),
            telemetry: self.telemetry(),
        });
        peer.client
            .download_resource(
                endpoint,
                owner,
                &PrepareResource {
                    binding,
                    resource: ResourceRefDto::from(&target.manifest),
                },
                tracked,
            )
            .await
            .map_err(map_sync_client_error)?;
        Ok(())
    }

    pub(super) async fn enroll_peer_snapshot(
        &self,
        peer: &ProvisionedPeer,
        endpoint: &Url,
    ) -> Result<(), SyncRuntimeError> {
        self.clear_legacy_replica_media_archive_intent()?;
        if self
            .database
            .replica_enrollment_complete(peer.host_device_id)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
        {
            return Ok(());
        }
        let progress = self
            .database
            .replica_snapshot_progress(peer.host_device_id)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        let mut page = if let Some((token, index)) = progress {
            match peer
                .client
                .pull_snapshot_page(endpoint, &token, index)
                .await
            {
                Err(ClientError::HostRejected(status))
                    if status == reqwest::StatusCode::CONFLICT
                        || status == reqwest::StatusCode::GONE =>
                {
                    // The host no longer retains this frozen session. Drop
                    // only its incomplete local staging; confirmed state and
                    // durable local edits are never reset by enrollment retry.
                    self.database
                        .discard_snapshot(peer.host_device_id, &token)
                        .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
                    peer.client.begin_snapshot(endpoint, 128).await
                }
                result => result,
            }
        } else {
            peer.client.begin_snapshot(endpoint, 128).await
        }
        .map_err(map_sync_client_error)?;
        loop {
            if page.checkpoint.binding != peer.binding {
                return Err(SyncRuntimeError::LibraryUnavailable);
            }
            // Stage catalog pages before downloading audio: the frozen host
            // snapshot expires after 15 minutes, while media transfer can take
            // longer. Publication downloads from the durable merged manifests.
            let applied =
                LanClient::apply_snapshot_page(&page, peer.host_device_id, self.database.as_ref())
                    .map_err(|error| match error {
                        ClientError::RemoteApply(echo_core::error::Error::Conflict {
                            what,
                            ..
                        }) if what == "snapshot object identity belongs to another local root" => {
                            SyncRuntimeError::LibraryConflict
                        }
                        error => {
                            tracing::error!(
                                target: "echo_desktop::sync",
                                page_index = page.page_index,
                                error = %error,
                                "failed to apply remote library snapshot page"
                            );
                            SyncRuntimeError::LibraryUnavailable
                        }
                    })?;
            let Some(index) = applied.next_page_index else {
                self.pending_catalog_changed.store(true, Ordering::Release);
                break;
            };
            page = peer
                .client
                .pull_snapshot_page(endpoint, &page.snapshot_token, index)
                .await
                .map_err(map_sync_client_error)?;
        }
        // Private cache is not library publication. Catalog media remains
        // unavailable until the safe file publication journal completes.
        self.clear_legacy_replica_media_archive_intent()?;
        Ok(())
    }

    pub(super) fn clear_legacy_replica_media_archive_intent(&self) -> Result<(), SyncRuntimeError> {
        let _activity = echo_core::application::sync::exclusive_local_activity()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        let configuration = self
            .database
            .current_sync_configuration()
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
            .ok_or(SyncRuntimeError::Unconfigured)?;
        let root = configuration.local_root;
        let Some(token) = self
            .database
            .pending_replica_media_archive(root)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
        else {
            return Ok(());
        };
        // Previous releases used this intent to move local files out of the
        // scanner's media tree during enrollment. Consume the stale intent
        // without moving anything; local files always stay in place.
        self.database
            .complete_replica_media_archive(root, token)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)
    }

    pub(super) async fn upload_pending_resources(
        &self,
        peer: &ProvisionedPeer,
        endpoint: &Url,
        root: LibraryRootId,
    ) -> Result<(), SyncRuntimeError> {
        let queue = ScopedLocalQueue {
            database: self.database.as_ref(),
            root,
        };
        let library = self
            .database
            .by_id(root)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
            .ok_or(SyncRuntimeError::LibraryUnavailable)?;
        let mutations = queue
            .pending_local_mutations(512)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        // Two upload streams per session, mirroring the download side. Each
        // resource is read directly from the library and keeps its own ordered
        // chunk sequence; only distinct resources overlap.
        let pending: Vec<&SharedResourceManifest> = mutations
            .iter()
            .flat_map(|mutation| &mutation.resource_refs)
            .filter(|resource| !self.is_uploaded(peer.binding, &resource.digest.to_hex()))
            .collect();
        observe_transfer(&self.telemetry(), |telemetry| {
            for resource in &pending {
                telemetry.declare_upload(&ResourceRefDto::from(*resource));
            }
        });
        let mut window = pending.into_iter();
        while let Some(first) = window.next() {
            if let Some(second) = window.next() {
                let (first, second) = tokio::join!(
                    self.upload_one_resource(peer, endpoint, &library, first),
                    self.upload_one_resource(peer, endpoint, &library, second),
                );
                first?;
                second?;
            } else {
                self.upload_one_resource(peer, endpoint, &library, first)
                    .await?;
            }
        }
        Ok(())
    }

    pub(super) async fn upload_one_resource(
        &self,
        peer: &ProvisionedPeer,
        endpoint: &Url,
        library: &LibraryRoot,
        resource: &SharedResourceManifest,
    ) -> Result<(), SyncRuntimeError> {
        let content_hash = resource.digest.to_hex();
        let request = PrepareResource {
            binding: peer.binding,
            resource: ResourceRefDto::from(resource),
        };
        let sources = local_resource_candidates(self.database.as_ref(), library, resource)?;
        if sources.is_empty() {
            tracing::warn!(
                target: "echo_desktop::sync",
                content_hash = %content_hash,
                "local resource is unavailable; upload deferred"
            );
            return Ok(());
        }
        let expected_hash = content_hash.clone();
        let expected_length = resource.length;
        let source = tokio::task::spawn_blocking(move || {
            verified_local_resource(sources, &expected_hash, expected_length)
        })
        .await
        .map_err(|_| SyncRuntimeError::WorkerFailed)??;
        let Some(mut source) = source else {
            tracing::warn!(
                target: "echo_desktop::sync",
                content_hash = %content_hash,
                "local resource changed since indexing; upload deferred"
            );
            return Ok(());
        };
        let mut progress = peer
            .client
            .prepare_resource(endpoint, &request)
            .await
            .map_err(map_sync_client_error)?;
        observe_transfer(&self.telemetry(), |telemetry| telemetry.upload(&progress));
        if progress.complete {
            self.remember_uploaded(peer.binding, &content_hash);
            return Ok(());
        }
        let chunks = resource.length.div_ceil(MAX_RESOURCE_CHUNK_BYTES as u64);
        for index in 0..chunks {
            let index = u32::try_from(index).map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
            if progress.confirmed_chunks.contains(&index) {
                continue;
            }
            let offset = u64::from(index) * MAX_RESOURCE_CHUNK_BYTES as u64;
            let length =
                usize::try_from((resource.length - offset).min(MAX_RESOURCE_CHUNK_BYTES as u64))
                    .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
            let bytes = source
                .seek(SeekFrom::Start(offset))
                .and_then(|_| {
                    let mut bytes = vec![0; length];
                    source.read_exact(&mut bytes)?;
                    Ok(bytes)
                })
                .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
            progress = peer
                .client
                .upload_resource_chunk(endpoint, peer.binding, &progress, index, bytes)
                .await
                .map_err(map_sync_client_error)?;
            observe_transfer(&self.telemetry(), |telemetry| telemetry.upload(&progress));
        }
        let progress = peer
            .client
            .complete_resource(endpoint, peer.binding, &progress)
            .await
            .map_err(map_sync_client_error)?;
        observe_transfer(&self.telemetry(), |telemetry| telemetry.upload(&progress));
        self.remember_uploaded(peer.binding, &content_hash);
        Ok(())
    }
}
