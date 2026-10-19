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
#[path = "sync_provisioner/provisioner_trait.rs"]
mod provisioner_trait;
#[path = "sync_provisioner/resource_candidates.rs"]
mod resource_candidates;

#[allow(clippy::redundant_pub_crate)]
pub(crate) use resource_candidates::{
    local_resource_candidates, verified_local_resource, wait_for_sync_cancel,
};

#[cfg(test)]
#[path = "sync_provisioner/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "sync_provisioner/error_tests.rs"]
mod error_tests;

use crate::sync_runtime::{
    ProgressNotifier, ProvisionedHost, ProvisionedPeer, SyncProgress, SyncRole, SyncRuntimeError,
    WatchOutcome,
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
