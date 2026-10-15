//! Trusted native composition for opt-in library synchronization.

use std::fs::File;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use echo_core::application::ports::{
    AppliedRemoteCursor, DeviceIdProvider, HostCommitLedgerPort, LibraryRepository,
    LocalSyncQueuePort, RemoteApplyPort, RemoteCommit, ReplicaSnapshotApplyPort,
};
use echo_core::application::remote_resources::{PublishRemoteResources, RemoteResourcePublishDeps};
use echo_core::application::root_switch::derive_root_id;
use echo_core::application::sync::ConfirmHostMutation;
use echo_core::domain::entities::{LibraryRoot, RootAvailability};
use echo_core::domain::ids::{LibraryRootId, OperationId};
use echo_core::domain::sync::{
    AuthorityId, CommitCheckpoint, DeviceId, Epoch, LibraryId, MutationCandidate, MutationId,
    ObjectKey, SharedResourceKind, SharedResourceManifest, SyncBinding,
};
use echo_core::infrastructure::filesystem::staging::StagingMarker;
use echo_core::infrastructure::filesystem::staging::MARKER_FORMAT_VERSION;
use echo_core::infrastructure::sqlite::SqliteDatabase;
use echo_desktop::platform::sync_security::{
    OsSecureStore, SecretBytes, SecretHandle, SecurePublicationCheckpoint,
};
use echo_sync::client::{ClientError, LanClient};
use echo_sync::pairing::{
    PairingError, PairingGrantDto, PairingInvitation, PairingPort, PairingRegistry,
    PendingPairingRequest,
};
use echo_sync::protocol::ResourceRefDto;
use echo_sync::protocol::{CommitPageDto, SnapshotPageDto, MAX_RESOURCE_CHUNK_BYTES};
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
#[cfg(test)]
#[path = "sync_provisioner/tests.rs"]
mod tests;

use crate::sync_runtime::{
    ProvisionedHost, ProvisionedPeer, SyncProvisioner, SyncRole, SyncRuntimeError,
};

const EMPTY_LIBRARY_MANIFEST: &str = "manifest.json";
const EMPTY_LIBRARY_STAGING_MARKER: &str = ".echo-ownership-marker";

/// Selecting a fresh library root creates Echo's portable manifest and its
/// owned staging marker before pairing. Those files are setup metadata, not
/// user content; enrollment still rejects every other file or nonempty media
/// directory. Symlinks are rejected at every level.
fn is_empty_enrollment_root(root: &Path) -> bool {
    fn visit_echo(
        path: &Path,
        relative: &Path,
        library_root: &Path,
        manifest: &mut bool,
        marker: &mut bool,
    ) -> bool {
        let Ok(entries) = std::fs::read_dir(path) else {
            return false;
        };
        for entry in entries {
            let Ok(entry) = entry else { return false };
            let child = entry.path();
            let Ok(metadata) = std::fs::symlink_metadata(&child) else {
                return false;
            };
            if metadata.file_type().is_symlink() {
                return false;
            }
            let child_relative = relative.join(entry.file_name());
            if metadata.is_dir() {
                if !visit_echo(&child, &child_relative, library_root, manifest, marker) {
                    return false;
                }
                continue;
            }
            if !metadata.is_file() {
                return false;
            }
            if relative.as_os_str().is_empty()
                && child_relative == Path::new(EMPTY_LIBRARY_MANIFEST)
            {
                *manifest = true;
            } else if child_relative == Path::new("tmp").join(EMPTY_LIBRARY_STAGING_MARKER) {
                let Ok(contents) = std::fs::read_to_string(&child) else {
                    return false;
                };
                let Some(parsed) = StagingMarker::parse(&contents) else {
                    return false;
                };
                if parsed.root != derive_root_id(library_root)
                    || parsed.version != MARKER_FORMAT_VERSION
                {
                    return false;
                }
                *marker = true;
            } else {
                return false;
            }
        }
        true
    }

    let Ok(canonical_root) = root.canonicalize() else {
        return false;
    };
    let Ok(entries) = std::fs::read_dir(&canonical_root) else {
        return false;
    };
    let mut saw_echo = false;
    for entry in entries {
        let Ok(entry) = entry else { return false };
        let Ok(metadata) = std::fs::symlink_metadata(entry.path()) else {
            return false;
        };
        if metadata.file_type().is_symlink() {
            return false;
        }
        match entry.file_name().to_str() {
            Some("echo") if metadata.is_dir() => {
                saw_echo = true;
                let mut manifest = false;
                let mut marker = false;
                if !visit_echo(
                    &entry.path(),
                    Path::new(""),
                    &canonical_root,
                    &mut manifest,
                    &mut marker,
                ) || !manifest
                    || !marker
                {
                    return false;
                }
            }
            Some("media") if metadata.is_dir() => {
                let Ok(mut children) = std::fs::read_dir(entry.path()) else {
                    return false;
                };
                if children.next().is_some() {
                    return false;
                }
            }
            _ => return false,
        }
    }
    saw_echo
}

/// No construction side effects: credentials and listening are opt-in.
pub struct NativeSyncProvisioner {
    database: Arc<SqliteDatabase>,
    private_resources: PathBuf,
    publication: RemoteResourcePublishDeps,
    gate: Mutex<()>,
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

impl NativeSyncProvisioner {
    pub const fn new(
        database: Arc<SqliteDatabase>,
        private_resources: PathBuf,
        publication: RemoteResourcePublishDeps,
    ) -> Self {
        Self {
            database,
            private_resources,
            publication,
            gate: Mutex::new(()),
        }
    }

    fn host_identity(&self, root: LibraryRootId) -> Result<HostIdentity, SyncRuntimeError> {
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
        let directory = self
            .private_resources
            .join(binding.library_id.as_uuid().to_string());
        std::fs::create_dir_all(&directory).map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        let resources = Arc::new(
            FileResourceStore::new(directory).map_err(|_| SyncRuntimeError::LibraryUnavailable)?,
        );
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
        if !library.is_active()
            || !library.write_capable()
            || library.availability() != RootAvailability::Available
        {
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
        let resources = self.seed_local_resources(&library, identity.binding)?;
        // No mutation is advertised before all frozen audio references have
        // immutable, verified bytes available from the native resource store.
        self.confirm_local(identity.binding)?;
        let api = HostApiState::new(
            identity.binding,
            credentials,
            self.database.clone(),
            checkpoint,
        )
        .map_err(|_| SyncRuntimeError::Unconfigured)?
        .with_authorization(self.database.clone())
        .with_snapshots(self.database.clone())
        .with_resources(resources)
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

    fn sync_now(&self, role: SyncRole) -> Result<(), SyncRuntimeError> {
        self.sync_now_cancellable(role, Arc::new(AtomicBool::new(false)))
    }

    fn sync_now_cancellable(
        &self,
        role: SyncRole,
        cancelled: Arc<AtomicBool>,
    ) -> Result<(), SyncRuntimeError> {
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
            return self.synchronize_peer(configuration.local_root, &cancelled);
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
        self.seed_local_resources(&library, configuration.binding)?;
        self.confirm_local(configuration.binding)
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
        if self
            .database
            .current_sync_configuration()
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
            .is_some()
        {
            return Err(SyncRuntimeError::LibraryUnavailable);
        }
        let library = self
            .database
            .by_id(root)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
            .ok_or(SyncRuntimeError::LibraryUnavailable)?;
        if !library.is_active()
            || !library.write_capable()
            || library.availability() != RootAvailability::Available
        {
            return Err(SyncRuntimeError::LibraryUnavailable);
        }
        // First enrollment never adopts unrelated files or overwrites another
        // library. Registered recovery uses the existing binding and vault slot.
        if !is_empty_enrollment_root(library.absolute_path()) {
            return Err(SyncRuntimeError::LibraryUnavailable);
        }
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
        if self
            .database
            .bind_empty_sync_replica(invitation.binding, root)
            .is_err()
        {
            // This adapter serializes local pairing. Compare the generation
            // before cleanup so a different successful enrollment is retained.
            let current = OsSecureStore
                .get(&handle)
                .map_err(|_| SyncRuntimeError::Unconfigured)?;
            let current: PeerIdentity = serde_json::from_slice(current.expose())
                .map_err(|_| SyncRuntimeError::Unconfigured)?;
            if current.generation == identity.generation {
                OsSecureStore
                    .delete(&handle)
                    .map_err(|_| SyncRuntimeError::Unconfigured)?;
            }
            return Err(SyncRuntimeError::LibraryUnavailable);
        }
        Ok(())
    }
}

impl NativeSyncProvisioner {
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
                    self.enroll_peer_snapshot(&peer, &endpoint, resources.clone()).await?;
                    self.upload_pending_resources(&peer, &endpoint, resources.as_ref(), root).await?;
                    let queue = ScopedLocalQueue { database: self.database.as_ref(), root };
                    self.prepared_incremental_cycle(&peer, &endpoint, &queue, resources.clone()).await?;
                    self.publish_remote_resources(&peer, &endpoint, root, resources.clone()).await?;
                    Ok(())
                } => result,
            }
        })
    }

    async fn prepared_incremental_cycle(
        &self,
        peer: &ProvisionedPeer,
        endpoint: &Url,
        queue: &ScopedLocalQueue<'_>,
        resources: Arc<FileResourceStore>,
    ) -> Result<(), SyncRuntimeError> {
        // A push can append accepted commits after the first prefetch. Retry
        // that exact anchor with the idempotent queue, after preparing the new
        // complete page; never apply a different unprepared range.
        for _ in 0..3 {
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
                )
                .await
                .map_err(|_| SyncRuntimeError::WorkerFailed)?;
            if page.binding != peer.binding {
                return Err(SyncRuntimeError::LibraryUnavailable);
            }
            let owner = DeviceId::from_uuid(self.database.current_device_id().as_uuid());
            for resource in page
                .entries
                .iter()
                .flat_map(|entry| &entry.commit.resource_refs)
            {
                peer.client
                    .download_resource(
                        endpoint,
                        owner,
                        &PrepareResource {
                            binding: peer.binding,
                            resource: resource.clone(),
                        },
                        resources.clone(),
                    )
                    .await
                    .map_err(|_| SyncRuntimeError::WorkerFailed)?;
            }
            let prepared = PreparedRemoteApply {
                database: self.database.as_ref(),
                host: peer.host_device_id,
                page,
                resources: resources.as_ref(),
                changed: std::sync::atomic::AtomicBool::new(false),
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
            if result.is_ok() {
                return Ok(());
            }
            if !prepared.changed.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(SyncRuntimeError::WorkerFailed);
            }
        }
        Err(SyncRuntimeError::Busy)
    }

    async fn publish_remote_resources(
        &self,
        peer: &ProvisionedPeer,
        endpoint: &Url,
        root: LibraryRootId,
        resources: Arc<FileResourceStore>,
    ) -> Result<(), SyncRuntimeError> {
        let sets = self
            .database
            .pending_remote_resource_sets(root)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        let owner = DeviceId::from_uuid(self.database.current_device_id().as_uuid());
        for (object, targets) in sets {
            for target in &targets {
                peer.client
                    .download_resource(
                        endpoint,
                        owner,
                        &PrepareResource {
                            binding: peer.binding,
                            resource: ResourceRefDto::from(&target.manifest),
                        },
                        resources.clone(),
                    )
                    .await
                    .map_err(|_| SyncRuntimeError::WorkerFailed)?;
            }
            let deps = self.publication.clone();
            let store = resources.clone();
            let binding = peer.binding;
            tokio::task::spawn_blocking(move || {
                let mut existing = None;
                for operation in deps
                    .journal
                    .incomplete(root)
                    .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
                {
                    let publication = deps
                        .journal
                        .load(root, operation)
                        .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
                    if publication.object == object
                        && publication.items.len() == targets.len()
                        && publication
                            .items
                            .iter()
                            .zip(&targets)
                            .all(|(item, target)| item.target.manifest == target.manifest)
                    {
                        existing = Some(operation);
                        break;
                    }
                }
                let publisher = PublishRemoteResources::new(deps);
                let operation = existing.unwrap_or_else(OperationId::new);
                if existing.is_none() {
                    publisher
                        .prepare(root, operation, object, targets.clone())
                        .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
                }
                for (index, target) in targets.iter().enumerate() {
                    store
                        .verify_complete(binding, &ResourceRefDto::from(&target.manifest))
                        .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
                    let mut reader = PrivateResourceReader {
                        store: store.clone(),
                        binding,
                        hash: target.manifest.digest.to_hex(),
                        length: target.manifest.length,
                        offset: 0,
                    };
                    publisher
                        .stage(root, operation, index, &mut reader)
                        .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
                }
                let outcome = publisher
                    .resume(root, operation)
                    .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
                if !outcome.media_ready {
                    return Err(SyncRuntimeError::Busy);
                }
                Ok(())
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
        resources: Arc<FileResourceStore>,
    ) -> Result<(), SyncRuntimeError> {
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
                Err(ClientError::HostRejected(reqwest::StatusCode::CONFLICT)) => {
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
        .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        loop {
            if page.checkpoint.binding != peer.binding {
                return Err(SyncRuntimeError::LibraryUnavailable);
            }
            self.cache_snapshot_resources(peer, endpoint, &page, resources.clone())
                .await?;
            let applied =
                LanClient::apply_snapshot_page(&page, peer.host_device_id, self.database.as_ref())
                    .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
            let Some(index) = applied.next_page_index else {
                break;
            };
            page = peer
                .client
                .pull_snapshot_page(endpoint, &page.snapshot_token, index)
                .await
                .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        }
        // Private cache is not library publication. Catalog media remains
        // unavailable until the safe file publication journal completes.
        Ok(())
    }

    async fn cache_snapshot_resources(
        &self,
        peer: &ProvisionedPeer,
        endpoint: &Url,
        page: &SnapshotPageDto,
        resources: Arc<FileResourceStore>,
    ) -> Result<(), SyncRuntimeError> {
        let owner = DeviceId::from_uuid(self.database.current_device_id().as_uuid());
        for resource in page.objects.iter().flat_map(|object| &object.resources) {
            let request = PrepareResource {
                binding: peer.binding,
                resource: resource.clone(),
            };
            peer.client
                .download_resource(endpoint, owner, &request, resources.clone())
                .await
                .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        }
        Ok(())
    }

    async fn upload_pending_resources(
        &self,
        peer: &ProvisionedPeer,
        endpoint: &Url,
        resources: &FileResourceStore,
        root: LibraryRootId,
    ) -> Result<(), SyncRuntimeError> {
        let queue = ScopedLocalQueue {
            database: self.database.as_ref(),
            root,
        };
        let mutations = queue
            .pending_local_mutations(512)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        for resource in mutations
            .iter()
            .flat_map(|mutation| &mutation.resource_refs)
        {
            let request = PrepareResource {
                binding: peer.binding,
                resource: ResourceRefDto::from(resource),
            };
            resources
                .verify_complete(peer.binding, &request.resource)
                .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
            let mut progress = peer
                .client
                .prepare_resource(endpoint, &request)
                .await
                .map_err(|_| SyncRuntimeError::WorkerFailed)?;
            if progress.complete {
                continue;
            }
            let chunks = resource.length.div_ceil(MAX_RESOURCE_CHUNK_BYTES as u64);
            for index in 0..chunks {
                let index =
                    u32::try_from(index).map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
                if progress.confirmed_chunks.contains(&index) {
                    continue;
                }
                let offset = u64::from(index) * MAX_RESOURCE_CHUNK_BYTES as u64;
                let length = usize::try_from(
                    (resource.length - offset).min(MAX_RESOURCE_CHUNK_BYTES as u64),
                )
                .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
                let chunk = resources
                    .download(peer.binding, &resource.digest.to_hex(), offset, length)
                    .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
                progress = peer
                    .client
                    .upload_resource_chunk(endpoint, peer.binding, &progress, index, chunk.bytes)
                    .await
                    .map_err(|_| SyncRuntimeError::WorkerFailed)?;
            }
            peer.client
                .complete_resource(endpoint, peer.binding, &progress)
                .await
                .map_err(|_| SyncRuntimeError::WorkerFailed)?;
        }
        Ok(())
    }
}

async fn wait_for_sync_cancel(cancelled: Arc<AtomicBool>) {
    while !cancelled.load(Ordering::Acquire) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
