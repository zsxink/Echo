//! Trusted native composition for opt-in library synchronization.

use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, Weak};
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

#[path = "sync_provisioner/host_recovery.rs"]
mod host_recovery;
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
#[path = "sync_provisioner/host_lifecycle.rs"]
mod host_lifecycle;
#[path = "sync_provisioner/lazy_host_resources.rs"]
mod lazy_host_resources;
#[path = "sync_provisioner/peer_publication.rs"]
mod peer_publication;
#[path = "sync_provisioner/peer_sync.rs"]
mod peer_sync;
#[path = "sync_provisioner/progress.rs"]
mod progress;
use progress::{observe_transfer, TrackedDownloadStore, TransferTelemetry};
#[cfg(test)]
#[path = "sync_provisioner/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "sync_provisioner/error_tests.rs"]
mod error_tests;

use crate::sync_runtime::{
    ProgressNotifier, ProvisionedHost, ProvisionedPeer, SyncProgress, SyncProvisioner, SyncRole,
    SyncRuntimeError, WatchOutcome,
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
    // The HTTP host and the sync worker must share one store instance: its
    // mutex serializes cleanup against in-flight prepare/write/complete calls.
    resource_stores: Mutex<HashMap<SyncBinding, Weak<FileResourceStore>>>,
    transfer_telemetry: Mutex<Arc<Mutex<TransferTelemetry>>>,
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
        ClientError::HostRejectedCode(status, code)
            if status == reqwest::StatusCode::SERVICE_UNAVAILABLE =>
        {
            tracing::warn!(target: "echo_desktop::sync", %status, %code, "sync host unavailable");
            SyncRuntimeError::SyncHostUnavailable
        }
        ClientError::HostRejected(status)
            if status == reqwest::StatusCode::UNAUTHORIZED
                || status == reqwest::StatusCode::FORBIDDEN =>
        {
            SyncRuntimeError::SyncAuthorizationFailed
        }
        ClientError::HostRejectedCode(status, code)
            if status == reqwest::StatusCode::UNAUTHORIZED
                || status == reqwest::StatusCode::FORBIDDEN =>
        {
            tracing::warn!(target: "echo_desktop::sync", %status, %code, "sync host authorization rejected");
            SyncRuntimeError::SyncAuthorizationFailed
        }
        ClientError::HostRejectedCode(_, code) if code == "library_binding_mismatch" => {
            tracing::warn!(target: "echo_desktop::sync", %code, "sync host rejected a mismatched library binding");
            SyncRuntimeError::SyncBindingMismatch
        }
        ClientError::HostRejectedCode(_, code) if code == "resource_not_found" => {
            tracing::warn!(target: "echo_desktop::sync", %code, "sync media resource is unavailable");
            SyncRuntimeError::SyncResourceUnavailable
        }
        ClientError::HostRejectedCode(_, code) if code == "resource_content_mismatch" => {
            tracing::warn!(target: "echo_desktop::sync", %code, "sync resource content does not match the host declaration");
            SyncRuntimeError::SyncResourceMismatch
        }
        ClientError::HostRejectedCode(_, code)
            if matches!(
                code.as_str(),
                "resource_not_complete"
                    | "invalid_resource_reference"
                    | "resource_version_mismatch"
            ) =>
        {
            tracing::warn!(target: "echo_desktop::sync", %code, "sync host could not verify the referenced media resource");
            SyncRuntimeError::SyncResourceMismatch
        }
        ClientError::HostRejectedCode(_, code) if code == "mutation_rejected" => {
            tracing::warn!(target: "echo_desktop::sync", %code, "sync host rejected the submitted mutation");
            SyncRuntimeError::SyncMutationRejected
        }
        ClientError::HostRejectedCode(_, code) if code == "rate_limited" => {
            tracing::warn!(target: "echo_desktop::sync", %code, "sync host rate limited this device");
            SyncRuntimeError::SyncRateLimited
        }
        ClientError::HostRejectedCode(_, code) if code == "commit_anchor_or_log_mismatch" => {
            tracing::warn!(target: "echo_desktop::sync", %code, "peer and host sync checkpoints do not match");
            SyncRuntimeError::SyncCheckpointMismatch
        }
        ClientError::HostRejected(_) => SyncRuntimeError::SyncHostRejected,
        ClientError::HostRejectedCode(_, code) => {
            tracing::warn!(target: "echo_desktop::sync", %code, "sync host rejected request");
            SyncRuntimeError::SyncHostRejected
        }
        ClientError::InvalidEndpoint => SyncRuntimeError::InvalidAddress,
        ClientError::InvalidTlsConfiguration(_)
        | ClientError::InvalidMutation(_)
        | ClientError::InvalidSnapshot(_)
        | ClientError::InvalidResponse(_)
        | ClientError::InvalidResponseBody
        | ClientError::InvalidCommitChain => SyncRuntimeError::SyncProtocolFailed,
        ClientError::RemoteApply(error) => {
            // Applying host commits is a local write against the catalog. It
            // says nothing about whether the music root is reachable, so
            // reporting "library unavailable" here sends the user to the wrong
            // place and drops the only evidence of what actually failed.
            tracing::warn!(
                target: "echo_desktop::sync",
                %error,
                "host commits could not be applied to the local catalog"
            );
            SyncRuntimeError::SyncRemoteApplyFailed
        }
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
            transfer_telemetry: Mutex::new(Arc::new(Mutex::new(TransferTelemetry::default()))),
            resource_stores: Mutex::new(HashMap::new()),
        }
    }

    fn telemetry(&self) -> Arc<Mutex<TransferTelemetry>> {
        Arc::clone(
            &self
                .transfer_telemetry
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        )
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
            if configuration.local_root != root {
                return Err(SyncRuntimeError::LibraryUnavailable);
            }
            if configuration.host {
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
            // Role switch: a replica-bound library becoming host mirrors the
            // automatic re-enrollment `complete_peer_pairing` performs when a
            // bound library joins another host. The old peer identity is
            // detached first — local catalog and media are kept — so switching
            // between host and replica roles needs no manual unbind. Without
            // this the switch failed with `LibraryUnavailable`, telling the
            // user to check a music directory that was never the problem.
            self.leave_sync_locked(root)?;
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
        let mut reanchored = false;
        loop {
            let mutations = self
                .database
                .pending_local_mutations(512)
                .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
            let batch_size = mutations.len();
            let mut rebase = false;
            for mutation in mutations {
                self.database
                    .advance_rejected_local_mutations()
                    .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
                ensure_candidate_root(self.database.as_ref(), configuration.local_root, &mutation)?;
                let changed_at_ms = self
                    .database
                    .local_mutation_changed_at_ms(mutation.mutation_id)
                    .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
                let outcome = confirm
                    .execute_at(binding, &mutation, changed_at_ms)
                    .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
                if outcome.checkpoint.is_none() {
                    if outcome.outcome_json.contains("OutOfOrderPredecessor") {
                        // The host owns this sequence space: its confirmed
                        // cursor is the authority, so the pending chain is
                        // re-anchored onto it instead of blocking the library's
                        // own writes forever.
                        if reanchored {
                            return Err(SyncRuntimeError::SyncPredecessorPending);
                        }
                        let anchor = self
                            .database
                            .local_origin_anchor()
                            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
                        self.database
                            .rebase_local_chain(anchor)
                            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
                        reanchored = true;
                        rebase = true;
                        break;
                    }
                    self.database
                        .reject_local_mutation(mutation.mutation_id, outcome.outcome_json)
                        .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
                }
            }
            if rebase {
                continue;
            }
            if batch_size < 512 {
                break;
            }
        }
        Ok(())
    }

    fn local_resource_store(
        &self,
        binding: SyncBinding,
    ) -> Result<Arc<FileResourceStore>, SyncRuntimeError> {
        let mut stores = self
            .resource_stores
            .lock()
            .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        stores.retain(|_, store| store.strong_count() > 0);
        if let Some(store) = stores.get(&binding).and_then(Weak::upgrade) {
            return Ok(store);
        }
        let directory = self
            .private_resources
            .join(binding.library_id.as_uuid().to_string());
        std::fs::create_dir_all(&directory).map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        let store = FileResourceStore::new(directory)
            .map(Arc::new)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        stores.insert(binding, Arc::downgrade(&store));
        drop(stores);
        Ok(store)
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

fn local_resource_candidates(
    database: &SqliteDatabase,
    library: &LibraryRoot,
    resource: &SharedResourceManifest,
) -> Result<Vec<File>, SyncRuntimeError> {
    let catalog_path = if resource.kind == SharedResourceKind::Audio {
        database
            .local_library_audio_path_for_resource(
                library.id(),
                &resource.digest.to_hex(),
                resource.length,
            )
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
    } else {
        None
    };
    let published_path = database
        .local_library_resource_path_for_manifest(library.id(), resource)
        .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
    let paths = catalog_path
        .iter()
        .chain(published_path.iter())
        .map(String::as_str)
        .chain(std::iter::once(resource.relative_path.as_str()));
    let mut files = Vec::new();
    for relative_path in paths {
        let Some(path) = authorized_source(library.absolute_path(), relative_path)? else {
            continue;
        };
        match File::open(path) {
            Ok(file) => {
                let metadata = file
                    .metadata()
                    .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
                if metadata.is_file() && metadata.len() == resource.length {
                    files.push(file);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(SyncRuntimeError::LibraryUnavailable),
        }
    }
    Ok(files)
}

fn verified_local_resource(
    candidates: Vec<File>,
    expected_hash: &str,
    expected_length: u64,
) -> Result<Option<File>, SyncRuntimeError> {
    for mut file in candidates {
        if verify_local_resource(&mut file, expected_hash, expected_length)? {
            return Ok(Some(file));
        }
    }
    Ok(None)
}

fn verify_local_resource(
    file: &mut File,
    expected_hash: &str,
    expected_length: u64,
) -> Result<bool, SyncRuntimeError> {
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0; MAX_RESOURCE_CHUNK_BYTES];
    let mut total = 0_u64;
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or(SyncRuntimeError::LibraryUnavailable)?;
        hasher.update(&buffer[..read]);
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
    Ok(total == expected_length && hasher.finalize().to_hex().as_str() == expected_hash)
}

async fn wait_for_sync_cancel(cancelled: Arc<AtomicBool>) {
    while !cancelled.load(Ordering::Acquire) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
