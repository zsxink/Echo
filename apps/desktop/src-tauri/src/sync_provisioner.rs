//! Trusted native composition for opt-in library synchronization.

use std::collections::HashSet;
use std::fs::File;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use echo_core::application::ports::{
    AppliedRemoteCursor, DeviceIdProvider, HostCommitLedgerPort, LibraryRepository,
    LocalSyncQueuePort, RemoteApplyPort, RemoteCommit, RemoteResourceTarget,
    ReplicaSnapshotApplyPort,
};
use echo_core::application::remote_resources::RemoteResourcePublishDeps;
use echo_core::application::scan::{ScanDeps, ScanSupervisor, StartScan};
use echo_core::application::sync::ConfirmHostMutation;
use echo_core::domain::entities::{LibraryRoot, RootAvailability};
use echo_core::domain::ids::LibraryRootId;
use echo_core::domain::sync::{
    AuthorityId, CommitCheckpoint, DeviceId, Epoch, LibraryId, MutationCandidate, MutationId,
    ObjectKey, SharedResourceKind, SharedResourceManifest, SyncBinding,
};
use echo_core::infrastructure::sqlite::SqliteDatabase;
use echo_desktop::platform::sync_security::{
    OsSecureStore, SecretBytes, SecretHandle, SecurePublicationCheckpoint,
};
use echo_sync::client::{ClientError, LanClient};
use echo_sync::pairing::{
    PairingError, PairingGrantDto, PairingInvitation, PairingPort, PairingRegistry,
    PendingPairingRequest,
};
#[cfg(test)]
use echo_sync::protocol::CommitPageDto;
use echo_sync::protocol::ResourceRefDto;
use echo_sync::protocol::MAX_RESOURCE_CHUNK_BYTES;
use echo_sync::resource_transfer::{FileResourceStore, PrepareResource, ResourceTransferPort};
use echo_sync::server::{tls13_server_config, HostApiState, PeerCredential};
use reqwest::Url;
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer};
use serde::{Deserialize, Serialize};

#[path = "sync_provisioner/pairing_store.rs"]
mod pairing_store;
use pairing_store::DurableHostPairing;
#[path = "sync_provisioner/remote_apply.rs"]
mod remote_apply;
#[allow(clippy::wildcard_imports)]
use remote_apply::*;
#[path = "sync_provisioner/enrollment.rs"]
mod enrollment;
use enrollment::prepare_enrollment_root;
#[cfg(test)]
use enrollment::{
    is_empty_enrollment_root, prepare_empty_enrollment_root, EMPTY_LIBRARY_STAGING_MARKER,
};
#[path = "sync_provisioner/resource_publication.rs"]
mod resource_publication;
use resource_publication::publish_remote_resource_set;
#[path = "sync_provisioner/lazy_host_resources.rs"]
mod lazy_host_resources;
#[cfg(test)]
#[path = "sync_provisioner/tests.rs"]
mod tests;

use crate::sync_runtime::{
    ProvisionedHost, ProvisionedPeer, SyncProvisioner, SyncRole, SyncRuntimeError, WatchOutcome,
};

/// No construction side effects: credentials and listening are opt-in.
pub struct NativeSyncProvisioner {
    database: Arc<SqliteDatabase>,
    private_resources: PathBuf,
    publication: RemoteResourcePublishDeps,
    scan_deps: Arc<ScanDeps>,
    scan_supervisor: ScanSupervisor,
    gate: Mutex<()>,
    // Retain applied work across a failed/cancelled cycle until a successful
    // completion can notify the UI. Empty polls leave this flag unset.
    pending_catalog_changed: Arc<AtomicBool>,
    host_seen_checkpoint: Mutex<Option<CommitCheckpoint>>,
    // The watch's own wake-up channel. The writer actor fans sync wakeups out
    // to every registered listener, so this channel is independent of the
    // runtime loop's: watching never steals the loop's local-write hints.
    local_wakes: Mutex<Option<LocalWakeChannel>>,
    // Resources this process has verified locally and that the peer has already
    // acknowledged: `prepare` reported the transfer complete, or
    // `complete_resource` succeeded. Every pending mutation re-declares its
    // resources on every cycle, so without this cache a 512-deep backlog
    // re-sends `prepare` for the whole queue each round (a guaranteed 429) and
    // re-reads each blob twice to re-derive BLAKE3.
    uploaded_resources: Mutex<HashSet<(SyncBinding, String)>>,
}

/// A registered sync wake-up channel kept alive for the watch. Dropping the
/// sender would disconnect the listener and let the writer actor prune it.
struct LocalWakeChannel {
    _sender: mpsc::Sender<()>,
    receiver: mpsc::Receiver<()>,
}

impl LocalWakeChannel {
    /// Drain queued wakeups; `true` when at least one was pending.
    fn drain_pending(&self) -> bool {
        let mut woke = false;
        while self.receiver.try_recv().is_ok() {
            woke = true;
        }
        woke
    }
}

// This document is stored only inside the operating system credential store.
// Never derive Debug or expose it through IPC.
#[derive(Serialize, Deserialize)]
struct HostIdentity {
    binding: SyncBinding,
    local_root: LibraryRootId,
    certificate_der: Vec<u8>,
    private_key_der: Vec<u8>,
}

#[derive(Serialize, Deserialize)]
struct PeerIdentity {
    binding: SyncBinding,
    local_root: LibraryRootId,
    local_device: DeviceId,
    host_device: DeviceId,
    generation: String,
    endpoint: String,
    host_certificate_der: Vec<u8>,
    bearer_token: String,
}

// The mapper only inspects the variant; taking the error by reference would
// force every `map_err` call site into a closure for no gain.
#[allow(clippy::needless_pass_by_value)]
fn map_sync_client_error(error: ClientError) -> SyncRuntimeError {
    match error {
        ClientError::Transport(_) => SyncRuntimeError::SyncConnectionFailed,
        ClientError::HostRejected(status) if status == reqwest::StatusCode::SERVICE_UNAVAILABLE => {
            SyncRuntimeError::SyncHostUnavailable
        }
        ClientError::HostRejected(status)
            if status == reqwest::StatusCode::UNAUTHORIZED
                || status == reqwest::StatusCode::FORBIDDEN =>
        {
            SyncRuntimeError::SyncAuthorizationFailed
        }
        ClientError::HostRejected(_) => SyncRuntimeError::SyncHostRejected,
        ClientError::InvalidEndpoint => SyncRuntimeError::InvalidAddress,
        ClientError::InvalidTlsConfiguration(_)
        | ClientError::InvalidMutation(_)
        | ClientError::InvalidSnapshot(_)
        | ClientError::InvalidResponse(_)
        | ClientError::InvalidResponseBody
        | ClientError::InvalidCommitChain => SyncRuntimeError::SyncProtocolFailed,
        ClientError::RemoteApply(_) => SyncRuntimeError::LibraryUnavailable,
    }
}

fn remove_peer_identity_if_generation(
    handle: &SecretHandle,
    identity: &PeerIdentity,
) -> Result<(), SyncRuntimeError> {
    let current = OsSecureStore
        .get(handle)
        .map_err(|_| SyncRuntimeError::Unconfigured)?;
    let current: PeerIdentity =
        serde_json::from_slice(current.expose()).map_err(|_| SyncRuntimeError::Unconfigured)?;
    if current.generation == identity.generation {
        OsSecureStore
            .delete(handle)
            .map_err(|_| SyncRuntimeError::Unconfigured)?;
    }
    Ok(())
}

impl NativeSyncProvisioner {
    pub fn new(
        database: Arc<SqliteDatabase>,
        private_resources: PathBuf,
        publication: RemoteResourcePublishDeps,
        scan_deps: Arc<ScanDeps>,
        scan_supervisor: ScanSupervisor,
    ) -> Self {
        Self {
            database,
            private_resources,
            publication,
            scan_deps,
            scan_supervisor,
            gate: Mutex::new(()),
            pending_catalog_changed: Arc::new(AtomicBool::new(false)),
            host_seen_checkpoint: Mutex::new(None),
            local_wakes: Mutex::new(None),
            uploaded_resources: Mutex::new(HashSet::new()),
        }
    }

    /// Mark a resource as uploaded for this session. A poisoned lock only means
    /// some other thread panicked; the cache is an optimization, so recovering
    /// the inner value is correct here.
    fn remember_uploaded(&self, binding: SyncBinding, content_hash: &str) {
        self.uploaded_resources
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert((binding, content_hash.to_owned()));
    }

    fn is_uploaded(&self, binding: SyncBinding, content_hash: &str) -> bool {
        self.uploaded_resources
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .contains(&(binding, content_hash.to_owned()))
    }

    fn host_identity(&self, root: LibraryRootId) -> Result<HostIdentity, SyncRuntimeError> {
        self.database
            .select_sync_context(root)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        let configuration = self
            .database
            .current_sync_configuration()
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        if let Some(configuration) = configuration {
            if !configuration.host || configuration.local_root != root {
                return Err(SyncRuntimeError::LibraryUnavailable);
            }
            let stored = OsSecureStore
                .get(&identity_handle(configuration.binding)?)
                .map_err(|_| SyncRuntimeError::Unconfigured)?;
            let identity: HostIdentity = serde_json::from_slice(stored.expose())
                .map_err(|_| SyncRuntimeError::Unconfigured)?;
            if identity.binding != configuration.binding || identity.local_root != root {
                return Err(SyncRuntimeError::Unconfigured);
            }
            return Ok(identity);
        }

        let binding = SyncBinding {
            library_id: LibraryId::new(),
            authority_id: AuthorityId::new(),
            epoch: Epoch::INITIAL,
        };
        let rcgen::CertifiedKey { cert, key_pair } =
            rcgen::generate_simple_self_signed(vec!["echo-host.local".to_owned()])
                .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        let identity = HostIdentity {
            binding,
            local_root: root,
            certificate_der: cert.der().to_vec(),
            private_key_der: key_pair.serialize_der(),
        };
        let secret = SecretBytes::new(
            serde_json::to_vec(&identity).map_err(|_| SyncRuntimeError::WorkerFailed)?,
        );
        // Durable independent state precedes the binding transaction. A crash
        // here leaves an unreachable slot, never a bound host without a key.
        SecurePublicationCheckpoint::initialize_new(binding)
            .map_err(|_| SyncRuntimeError::Unconfigured)?;
        OsSecureStore
            .set(&identity_handle(binding)?, &secret)
            .map_err(|_| SyncRuntimeError::Unconfigured)?;
        self.database
            .bind_sync_library(binding, true, root)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        Ok(identity)
    }

    fn recover_checkpoint(
        &self,
        binding: SyncBinding,
    ) -> Result<Arc<SecurePublicationCheckpoint>, SyncRuntimeError> {
        let checkpoint = Arc::new(
            SecurePublicationCheckpoint::open_existing(binding)
                .map_err(|_| SyncRuntimeError::Unconfigured)?,
        );
        let independent = checkpoint
            .read()
            .map_err(|_| SyncRuntimeError::Unconfigured)?;
        let ledger = self
            .database
            .latest_published_sync_checkpoint(binding)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        let pending = self
            .database
            .pending_publications(512)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        let staged: Vec<_> = pending
            .iter()
            .filter_map(|entry| entry.checkpoint)
            .collect();
        validate_startup_head(independent, ledger, &staged)?;
        let confirm = ConfirmHostMutation::new(self.database.as_ref(), checkpoint.as_ref());
        while confirm
            .recover_publications(512)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
            == 512
        {}
        let recovered = self
            .database
            .latest_published_sync_checkpoint(binding)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        if checkpoint
            .read()
            .map_err(|_| SyncRuntimeError::Unconfigured)?
            != recovered
        {
            return Err(SyncRuntimeError::LibraryUnavailable);
        }
        Ok(checkpoint)
    }

    fn confirm_local(&self, binding: SyncBinding) -> Result<(), SyncRuntimeError> {
        self.database
            .advance_rejected_local_mutations()
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        let configuration = self
            .database
            .current_sync_configuration()
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
            .ok_or(SyncRuntimeError::Unconfigured)?;
        if !configuration.host || configuration.binding != binding {
            return Err(SyncRuntimeError::LibraryUnavailable);
        }
        let checkpoint = self.recover_checkpoint(binding)?;
        let confirm = ConfirmHostMutation::new(self.database.as_ref(), checkpoint.as_ref());
        loop {
            let mutations = self
                .database
                .pending_local_mutations(512)
                .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
            let batch_size = mutations.len();
            for mutation in mutations {
                self.database
                    .advance_rejected_local_mutations()
                    .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
                ensure_candidate_root(self.database.as_ref(), configuration.local_root, &mutation)?;
                let outcome = confirm
                    .execute(binding, &mutation)
                    .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
                if outcome.checkpoint.is_none() {
                    if outcome.outcome_json.contains("OutOfOrderPredecessor") {
                        return Err(SyncRuntimeError::LibraryUnavailable);
                    }
                    self.database
                        .reject_local_mutation(mutation.mutation_id, outcome.outcome_json)
                        .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
                }
            }
            if batch_size < 512 {
                break;
            }
        }
        Ok(())
    }

    fn seed_local_resources(
        &self,
        library: &LibraryRoot,
        binding: SyncBinding,
    ) -> Result<Arc<FileResourceStore>, SyncRuntimeError> {
        let resources = self.local_resource_store(binding)?;
        // Existing local resources are part of the enrollment baseline. Keep
        // seeding them so the first snapshot cannot silently discard local work.
        let manifests = self
            .database
            .local_sync_resource_manifests(library.id())
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        seed_audio_resources(
            resources.as_ref(),
            library,
            binding,
            DeviceId::from_uuid(self.database.current_device_id().as_uuid()),
            &manifests,
        )?;
        Ok(resources)
    }

    fn local_resource_store(
        &self,
        binding: SyncBinding,
    ) -> Result<Arc<FileResourceStore>, SyncRuntimeError> {
        let directory = self
            .private_resources
            .join(binding.library_id.as_uuid().to_string());
        std::fs::create_dir_all(&directory).map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        FileResourceStore::new(directory)
            .map(Arc::new)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)
    }
}

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
        let client = LanClient::new(&identity.host_certificate_der, identity.bearer_token)
            .map_err(|_| SyncRuntimeError::Unconfigured)?;
        Ok(ProvisionedPeer {
            binding: identity.binding,
            address: identity.endpoint,
            client,
            host_device_id: identity.host_device,
        })
    }

    fn sync_now(&self, role: SyncRole) -> Result<bool, SyncRuntimeError> {
        self.sync_now_cancellable(role, Arc::new(AtomicBool::new(false)))
    }

    fn sync_now_cancellable(
        &self,
        role: SyncRole,
        cancelled: Arc<AtomicBool>,
    ) -> Result<bool, SyncRuntimeError> {
        let _guard = self
            .gate
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        let configuration = self
            .database
            .current_sync_configuration()
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
            .ok_or(SyncRuntimeError::Unconfigured)?;
        if role == SyncRole::Peer && !configuration.host {
            self.synchronize_peer(configuration.local_root, &cancelled)?;
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
            return Err(SyncRuntimeError::Busy);
        }
        let resources = self.local_resource_store(configuration.binding)?;
        self.confirm_local(configuration.binding)?;
        self.publish_cached_peer_resources(
            configuration.local_root,
            configuration.binding,
            &resources,
        )?;
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
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        executor.block_on(self.watch_peer_commits(&peer, configuration.local_root, cancelled, wait))
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
        let _activity = echo_core::application::sync::exclusive_local_activity()
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
        if current_configuration.is_some() && !already_bound_to_this_host {
            if let Err(error) = self.leave_sync_locked(root) {
                remove_peer_identity_if_generation(&handle, &identity)?;
                return Err(error);
            }
        }
        if self
            .database
            .bind_sync_replica_for_enrollment(invitation.binding, root)
            .is_err()
        {
            remove_peer_identity_if_generation(&handle, &identity)?;
            return Err(SyncRuntimeError::LibraryUnavailable);
        }
        self.database
            .ensure_replica_sync_baseline(invitation.binding, root)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        Ok(())
    }
}

impl NativeSyncProvisioner {
    fn refresh_published_metadata(&self, root: LibraryRootId) {
        let songs = match self.database.pending_remote_song_metadata(root) {
            Ok(songs) => songs,
            Err(error) => {
                tracing::warn!(
                    target: "echo_desktop::sync",
                    error = %error,
                    "published song metadata indexing will retry next cycle"
                );
                return;
            }
        };
        let indexer = StartScan::new(&self.scan_deps, &self.scan_supervisor);
        for song in songs {
            if let Err(error) = indexer.refresh_published_song(root, song) {
                // File transfer has already committed. Keep the NULL scan
                // stamp and retry next cycle, without reporting a transport
                // failure or blocking other songs on one bad metadata read.
                tracing::warn!(
                    target: "echo_desktop::sync",
                    song = %song,
                    error = %error,
                    "published song metadata indexing will retry next cycle"
                );
            } else {
                self.pending_catalog_changed.store(true, Ordering::Release);
            }
        }
    }

    fn leave_sync_locked(&self, root: LibraryRootId) -> Result<(), SyncRuntimeError> {
        self.database
            .select_sync_context(root)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        let configuration = self
            .database
            .current_sync_configuration()
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        let mut credentials = Vec::new();
        if let Some(configuration) = configuration {
            if configuration.host {
                for (_, handle) in self
                    .database
                    .authorized_sync_peer_handles()
                    .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
                {
                    credentials.push(
                        SecretHandle::new(handle).map_err(|_| SyncRuntimeError::Unconfigured)?,
                    );
                }
                credentials.push(identity_handle(configuration.binding)?);
                credentials.push(
                    SecretHandle::new(format!(
                        "checkpoint-{}-{}-{}",
                        configuration.binding.library_id.as_uuid(),
                        configuration.binding.authority_id.as_uuid(),
                        configuration.binding.epoch.as_u64()
                    ))
                    .map_err(|_| SyncRuntimeError::Unconfigured)?,
                );
            } else {
                credentials.push(peer_identity_handle(configuration.binding, root)?);
            }
        }
        // Revoke database authorization before deleting credentials. If secure
        // cleanup fails, stale credentials still cannot authenticate.
        self.database
            .detach_lan_sync_context(root)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        for handle in credentials {
            match OsSecureStore.delete(&handle) {
                Ok(()) => {}
                // The vault delete API reports missing and unreadable alike;
                // the binding is already detached, so leave remains safe.
                Err(_) => {
                    tracing::warn!(target: "echo_desktop::sync", "stale sync credential cleanup failed");
                }
            }
        }
        Ok(())
    }

    fn synchronize_peer(
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
        let resources = self.seed_local_resources(&library, peer.binding)?;
        let endpoint = Url::parse(&peer.address).map_err(|_| SyncRuntimeError::InvalidAddress)?;
        let executor = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        executor.block_on(async {
            tokio::select! {
                biased;
                () = wait_for_sync_cancel(Arc::clone(cancelled)) => Err(SyncRuntimeError::Busy),
                result = async {
                    self.merge_peer_library(&peer, &endpoint, &resources, root).await
                } => result,
            }
        })
    }

    /// Register this provisioner's own sync wake-up channel on first use and
    /// drain whatever queued. The writer actor fans wakeups out to every
    /// listener, so this never consumes the runtime loop's hints.
    fn take_local_wake(&self) -> bool {
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

    async fn merge_peer_library(
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
            self.upload_pending_resources(peer, endpoint, resources, root)
                .await?;
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
        Ok(())
    }

    /// Apply every published host page. Catalog pages are staged immediately:
    /// Core persists the objects, their shared manifests and the cursor in one
    /// transaction, and materializes confirmed songs as unavailable. Media is
    /// attached afterwards from those durable manifests
    /// (`publish_remote_resources`), so a slow or stalled transfer can never
    /// park an object update behind it — the guarantee the "对象先于媒体到达"
    /// scenario requires, and the order snapshot enrollment already uses.
    async fn pull_host_changes(
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
    async fn watch_peer_commits(
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
                echo_sync::replica::ReplicaSyncError::Core(_) => {
                    SyncRuntimeError::LibraryUnavailable
                }
                _ => SyncRuntimeError::SyncProtocolFailed,
            }),
        }
    }

    fn publish_cached_peer_resources(
        &self,
        root: LibraryRootId,
        binding: SyncBinding,
        resources: &Arc<FileResourceStore>,
    ) -> Result<(), SyncRuntimeError> {
        let sets = self
            .database
            .pending_peer_resource_sets(root)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        for (object, targets) in sets {
            publish_remote_resource_set(
                self.publication.clone(),
                resources,
                binding,
                root,
                object,
                &targets,
            )?;
            self.pending_catalog_changed.store(true, Ordering::Release);
        }
        Ok(())
    }

    async fn publish_remote_resources(
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
        let owner = DeviceId::from_uuid(self.database.current_device_id().as_uuid());
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
                    stage_remote_resource(peer, endpoint, owner, first, resources),
                    stage_remote_resource(peer, endpoint, owner, second, resources),
                );
                first?;
                second?;
            } else {
                stage_remote_resource(peer, endpoint, owner, first, resources).await?;
            }
        }
        for (object, targets) in sets {
            let deps = self.publication.clone();
            let store = Arc::clone(resources);
            let binding = peer.binding;
            let catalog_changed = Arc::clone(&self.pending_catalog_changed);
            tokio::task::spawn_blocking(move || {
                publish_remote_resource_set(deps, &store, binding, root, object, &targets)?;
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

    async fn enroll_peer_snapshot(
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

    fn clear_legacy_replica_media_archive_intent(&self) -> Result<(), SyncRuntimeError> {
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

    async fn upload_pending_resources(
        &self,
        peer: &ProvisionedPeer,
        endpoint: &Url,
        resources: &Arc<FileResourceStore>,
        root: LibraryRootId,
    ) -> Result<(), SyncRuntimeError> {
        let queue = ScopedLocalQueue {
            database: self.database.as_ref(),
            root,
        };
        let mutations = queue
            .pending_local_mutations(512)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        // Two upload streams per session, mirroring the download side. Each
        // resource keeps its own ordered chunk sequence; only distinct
        // resources overlap. Resources staged earlier in this session are
        // dropped before they can cost another chunk.
        let pending: Vec<&SharedResourceManifest> = mutations
            .iter()
            .flat_map(|mutation| &mutation.resource_refs)
            .filter(|resource| !self.is_uploaded(peer.binding, &resource.digest.to_hex()))
            .collect();
        let mut window = pending.into_iter();
        while let Some(first) = window.next() {
            if let Some(second) = window.next() {
                let (first, second) = tokio::join!(
                    self.upload_one_resource(peer, endpoint, resources, first),
                    self.upload_one_resource(peer, endpoint, resources, second),
                );
                first?;
                second?;
            } else {
                self.upload_one_resource(peer, endpoint, resources, first)
                    .await?;
            }
        }
        Ok(())
    }

    async fn upload_one_resource(
        &self,
        peer: &ProvisionedPeer,
        endpoint: &Url,
        resources: &Arc<FileResourceStore>,
        resource: &SharedResourceManifest,
    ) -> Result<(), SyncRuntimeError> {
        let content_hash = resource.digest.to_hex();
        let request = PrepareResource {
            binding: peer.binding,
            resource: ResourceRefDto::from(resource),
        };
        // `verify_complete` re-reads the whole blob to prove its digest: tens of
        // milliseconds of hashing for a lossless track, and it used to run
        // inline on the runtime thread. It runs only on resources this session
        // has not already uploaded, and a missing or unreadable local blob
        // skips that one resource instead of failing the cycle, so the rest of
        // the queue can still drain and the stuck mutation stays visible as a
        // no-progress predecessor.
        let verified = {
            let store = Arc::clone(resources);
            let binding = peer.binding;
            let resource = request.resource.clone();
            tokio::task::spawn_blocking(move || store.verify_complete(binding, &resource))
                .await
                .map_err(|_| SyncRuntimeError::WorkerFailed)?
        };
        if verified.is_err() {
            tracing::warn!(
                target: "echo_desktop::sync",
                content_hash = %content_hash,
                "local resource is not complete; upload deferred"
            );
            return Ok(());
        }
        let mut progress = peer
            .client
            .prepare_resource(endpoint, &request)
            .await
            .map_err(map_sync_client_error)?;
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
            let chunk = resources
                .download(peer.binding, &content_hash, offset, length)
                .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
            progress = peer
                .client
                .upload_resource_chunk(endpoint, peer.binding, &progress, index, chunk.bytes)
                .await
                .map_err(map_sync_client_error)?;
        }
        peer.client
            .complete_resource(endpoint, peer.binding, &progress)
            .await
            .map_err(map_sync_client_error)?;
        self.remember_uploaded(peer.binding, &content_hash);
        Ok(())
    }
}

/// Stage one remote resource into the local cache. Two of these run at once per
/// session (`publish_remote_resources`), so the host sees two interleaved chunk
/// streams; the chunks of a single file stay ordered.
async fn stage_remote_resource(
    peer: &ProvisionedPeer,
    endpoint: &Url,
    owner: DeviceId,
    target: &RemoteResourceTarget,
    resources: &Arc<FileResourceStore>,
) -> Result<(), SyncRuntimeError> {
    peer.client
        .download_resource(
            endpoint,
            owner,
            &PrepareResource {
                binding: peer.binding,
                resource: ResourceRefDto::from(&target.manifest),
            },
            resources.clone() as Arc<dyn ResourceTransferPort>,
        )
        .await
        .map_err(map_sync_client_error)?;
    Ok(())
}

async fn wait_for_sync_cancel(cancelled: Arc<AtomicBool>) {
    while !cancelled.load(Ordering::Acquire) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
