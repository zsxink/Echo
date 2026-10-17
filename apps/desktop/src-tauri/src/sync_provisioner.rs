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
use echo_core::application::remote_resources::RemoteResourcePublishDeps;
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
use echo_sync::protocol::ResourceRefDto;
use echo_sync::protocol::{CommitPageDto, MAX_RESOURCE_CHUNK_BYTES};
use echo_sync::resource_transfer::{FileResourceStore, PrepareResource, ResourceTransferPort};
use echo_sync::server::{HostApiState, PeerCredential, tls13_server_config};
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
    EMPTY_LIBRARY_STAGING_MARKER, is_empty_enrollment_root, prepare_empty_enrollment_root,
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
    ProvisionedHost, ProvisionedPeer, SyncProvisioner, SyncRole, SyncRuntimeError,
};

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
            library.clone(),
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
        let resources = self.local_resource_store(configuration.binding)?;
        self.confirm_local(configuration.binding)?;
        self.publish_cached_peer_resources(
            configuration.local_root,
            configuration.binding,
            &resources,
        )
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
                    tracing::warn!(target: "echo_desktop::sync", "stale sync credential cleanup failed")
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
            // Merge the complete host history before publishing local additions.
            self.pull_host_changes(peer, endpoint, &queue, resources)
                .await?;
            self.publish_remote_resources(peer, endpoint, root, resources)
                .await?;
            self.upload_pending_resources(peer, endpoint, resources.as_ref(), root)
                .await?;
            let before = queue
                .replica_checkpoint(peer.host_device_id)
                .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
            self.prepared_incremental_cycle(peer, endpoint, &queue, resources)
                .await?;
            self.publish_remote_resources(peer, endpoint, root, resources)
                .await?;
            if queue
                .pending_local_mutations(1)
                .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
                .is_empty()
            {
                self.pull_host_changes(peer, endpoint, &queue, resources)
                    .await?;
                self.publish_remote_resources(peer, endpoint, root, resources)
                    .await?;
                break;
            }
            let after = queue
                .replica_checkpoint(peer.host_device_id)
                .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
            if after == before {
                return Err(SyncRuntimeError::Busy);
            }
        }
        Ok(())
    }

    async fn pull_host_changes(
        &self,
        peer: &ProvisionedPeer,
        endpoint: &Url,
        queue: &ScopedLocalQueue<'_>,
        resources: &Arc<FileResourceStore>,
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
                )
                .await
                .map_err(map_sync_client_error)?;
            if page.binding != peer.binding {
                return Err(SyncRuntimeError::LibraryUnavailable);
            }
            let finished = page.entries.is_empty();
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
                    .map_err(map_sync_client_error)?;
            }
            let prepared = PreparedRemoteApply {
                database: self.database.as_ref(),
                host: peer.host_device_id,
                page,
                resources: resources.as_ref(),
                changed: AtomicBool::new(false),
            };
            LanClient::apply_commit_page(&prepared.page, peer.host_device_id, &prepared)
                .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
            if finished {
                return Ok(());
            }
        }
    }

    async fn prepared_incremental_cycle(
        &self,
        peer: &ProvisionedPeer,
        endpoint: &Url,
        queue: &ScopedLocalQueue<'_>,
        resources: &Arc<FileResourceStore>,
    ) -> Result<(), SyncRuntimeError> {
        // A push can append commits after prefetch. Prepare the changed page
        // before applying it, then acknowledge accepted local queue entries.
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
            .map_err(map_sync_client_error)?;
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
                .map_err(map_sync_client_error)?;
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
        match result {
            Ok(_) => Ok(()),
            Err(_) if prepared.changed.load(Ordering::Relaxed) => {
                // Apply newly accepted commits before selecting another push
                // batch, so accepted local mutations are not uploaded twice.
                self.pull_host_changes(peer, endpoint, queue, resources)
                    .await
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
                    .map_err(map_sync_client_error)?;
            }
            let deps = self.publication.clone();
            let store = Arc::clone(resources);
            let binding = peer.binding;
            tokio::task::spawn_blocking(move || {
                publish_remote_resource_set(deps, &store, binding, root, object, &targets)
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
                .map_err(map_sync_client_error)?;
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
                    .map_err(map_sync_client_error)?;
            }
            peer.client
                .complete_resource(endpoint, peer.binding, &progress)
                .await
                .map_err(map_sync_client_error)?;
        }
        Ok(())
    }
}

async fn wait_for_sync_cancel(cancelled: Arc<AtomicBool>) {
    while !cancelled.load(Ordering::Acquire) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
