//! Bounded HTTP API for host mutation confirmation.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::body::Body;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use echo_core::application::ports::{
    HostCommitLedgerPort, HostPeerAuthorizationPort, HostSnapshotPort, HostSnapshotRecord,
    PublicationCheckpointPort, PublishedCommit,
};
use echo_core::application::sync::ConfirmHostMutation;
use echo_core::domain::sync::{DeviceId, MutationCandidate, SyncBinding};
use hyper_util::rt::TokioIo;
use hyper_util::rt::TokioTimer;
use hyper_util::service::TowerToHyperService;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use serde::{Deserialize, Serialize};
use tokio::net::TcpListener;
use tokio::sync::Semaphore;
use tokio_rustls::TlsAcceptor;

mod resource_routes;
use resource_routes::{complete_resource, download_resource, prepare_resource, upload_chunk};

use crate::pairing::{
    PairingBootstrapResponse, PairingClaimDto, PairingCodeRequest, PairingError, PairingInvitation,
    PairingPort, PairingRegistry, PendingPairingRequest,
};

use crate::protocol::{
    commit_digest, validate_mutation, validate_resource, validate_snapshot_page, CheckpointDto,
    CommitDto, CommitPageDto, CommittedEntryDto, MemberAliasDto, MutationAck, MutationDto,
    ProtocolError, ResourceRefDto, SnapshotObjectDto, SnapshotPageDto, SongAliasDto, TypedAliasDto,
    MAX_COMMIT_BYTES, MAX_COMMIT_WAIT_MS, MAX_MUTATION_BYTES, MAX_PAGE_SIZE, MAX_SYNC_PAGE_BYTES,
    PROTOCOL_VERSION,
};
use crate::resource_transfer::{
    PrepareResource, ResourceTransferPort, TransferError, MAX_RESOURCE_BYTES,
};

type PeerCredentialHashes = Arc<Mutex<Vec<(DeviceId, [u8; 32])>>>;

#[derive(Clone)]
pub struct HostApiState {
    binding: SyncBinding,
    active: Arc<AtomicBool>,
    peer_credentials: PeerCredentialHashes,
    pairing: Option<(Arc<PairingRegistry>, Arc<dyn PairingPort>)>,
    pairing_window: Arc<Mutex<(Instant, u16)>>,
    pairing_approvals: Arc<Mutex<()>>,
    request_windows: Arc<Mutex<HashMap<DeviceId, (Instant, u16)>>>,
    /// Resource chunks are sized in bytes, not requests, so they are budgeted
    /// apart from the control traffic. Sharing one window meant a bulk upload
    /// could spend the whole allowance and answer the peer's long-poll with
    /// 429 for the rest of the minute.
    resource_windows: Arc<Mutex<HashMap<DeviceId, (Instant, u16)>>>,
    ledger: Arc<dyn HostCommitLedgerPort>,
    checkpoint: Arc<dyn PublicationCheckpointPort>,
    authorization: Option<Arc<dyn HostPeerAuthorizationPort>>,
    snapshots: Option<Arc<dyn HostSnapshotPort>>,
    resources: Option<Arc<dyn ResourceTransferPort>>,
}

impl HostApiState {
    /// Build host state from the binding, authorized peers, and commit ports.
    ///
    /// # Errors
    /// Rejects short tokens, duplicate devices or tokens, and more than 32 peers.
    pub fn new(
        binding: SyncBinding,
        peer_credentials: Vec<PeerCredential>,
        ledger: Arc<dyn HostCommitLedgerPort>,
        checkpoint: Arc<dyn PublicationCheckpointPort>,
    ) -> Result<Self, CredentialConfigError> {
        let mut credentials = Vec::with_capacity(peer_credentials.len());
        for credential in peer_credentials {
            if credential.bearer_token.len() < 32 {
                return Err(CredentialConfigError::TokenTooShort);
            }
            credentials.push((
                credential.device_id,
                *blake3::hash(&credential.bearer_token).as_bytes(),
            ));
        }
        if credentials.len() > 32 {
            return Err(CredentialConfigError::TooManyDevices);
        }
        if credentials
            .iter()
            .enumerate()
            .any(|(index, (device, token_hash))| {
                credentials[..index]
                    .iter()
                    .any(|(prior_device, prior_hash)| {
                        prior_device == device || prior_hash == token_hash
                    })
            })
        {
            return Err(CredentialConfigError::DuplicateDeviceOrToken);
        }
        Ok(Self {
            binding,
            active: Arc::new(AtomicBool::new(true)),
            peer_credentials: Arc::new(Mutex::new(credentials)),
            pairing: None,
            pairing_window: Arc::new(Mutex::new((Instant::now(), 0))),
            pairing_approvals: Arc::new(Mutex::new(())),
            request_windows: Arc::new(Mutex::new(HashMap::new())),
            resource_windows: Arc::new(Mutex::new(HashMap::new())),
            ledger,
            checkpoint,
            authorization: None,
            snapshots: None,
            resources: None,
        })
    }

    /// Require a live authorization lookup so revocation applies to an
    /// already-running server and its existing authenticated connections.
    #[must_use]
    pub fn with_authorization(mut self, authorization: Arc<dyn HostPeerAuthorizationPort>) -> Self {
        self.authorization = Some(authorization);
        self
    }

    /// Enable short-code pairing backed by durable peer provisioning.
    #[must_use]
    pub fn with_pairing(
        mut self,
        registry: Arc<PairingRegistry>,
        port: Arc<dyn PairingPort>,
    ) -> Self {
        self.pairing = Some((registry, port));
        self
    }

    /// Create public pin material and a one-use invitation for trusted transfer.
    ///
    /// # Errors
    /// Rejects unavailable pairing, invalid lifetimes, or capacity.
    pub fn create_pairing_invitation(
        &self,
        host_device_id: DeviceId,
        certificate_der: Vec<u8>,
        ttl: Duration,
    ) -> Result<PairingInvitation, PairingError> {
        if !self.active.load(Ordering::Acquire) {
            return Err(PairingError::Unavailable);
        }
        let (registry, port) = self.pairing.as_ref().ok_or(PairingError::Unavailable)?;
        registry.cleanup_expired(port.as_ref())?;
        registry.create_invitation(host_device_id, certificate_der, ttl)
    }

    /// Validate the short code, provision the peer immediately, and return the
    /// certificate pin and bearer together for the joining device to persist.
    ///
    /// # Errors
    /// Rejects unavailable pairing, an expired or already-claimed code, the peer
    /// credential capacity limit, and any provisioning failure.
    pub fn pair_with_code(
        &self,
        request: &PairingCodeRequest,
    ) -> Result<PairingBootstrapResponse, PairingError> {
        let (registry, port) = self.pairing.as_ref().ok_or(PairingError::Unavailable)?;
        let (receipt, invitation) = registry.request_with_code(request)?;
        self.approve_pairing_request(&receipt.request_id)?;
        let grant = registry.claim(
            &PairingClaimDto {
                binding: receipt.binding,
                request_id: receipt.request_id,
                device_id: receipt.device_id,
                claim_secret: receipt.claim_secret,
            },
            port.as_ref(),
        )?;
        Ok(PairingBootstrapResponse { invitation, grant })
    }

    /// Return metadata for requests awaiting explicit host approval.
    ///
    /// # Errors
    /// Rejects unavailable pairing or registry failures.
    pub fn pending_pairing_requests(&self) -> Result<Vec<PendingPairingRequest>, PairingError> {
        if !self.active.load(Ordering::Acquire) {
            return Err(PairingError::Unavailable);
        }
        let (registry, port) = self.pairing.as_ref().ok_or(PairingError::Unavailable)?;
        registry.cleanup_expired(port.as_ref())?;
        registry.pending_requests()
    }

    /// Durably approve then activate a peer in every clone of this running host.
    ///
    /// # Errors
    /// Fails closed for unavailable provisioning, expired requests, or conflicts.
    pub fn approve_pairing_request(&self, request_id: &str) -> Result<(), PairingError> {
        if !self.active.load(Ordering::Acquire) {
            return Err(PairingError::Unavailable);
        }
        let (registry, port) = self.pairing.as_ref().ok_or(PairingError::Unavailable)?;
        let _approval = self
            .pairing_approvals
            .lock()
            .map_err(|_| PairingError::Unavailable)?;
        let authorization = self
            .authorization
            .as_ref()
            .ok_or_else(|| pairing_provisioning_failure("authorization_port_missing"))?;
        registry.cleanup_expired(port.as_ref())?;
        {
            let mut credentials = self
                .peer_credentials
                .lock()
                .map_err(|_| PairingError::Unavailable)?;
            let mut active = Vec::new();
            for credential in credentials.iter().copied() {
                if authorization
                    .peer_is_authorized(self.binding, credential.0)
                    .map_err(|_| pairing_provisioning_failure("authorization_lookup"))?
                {
                    active.push(credential);
                }
            }
            *credentials = active;
        }
        let request = registry
            .pending_requests()?
            .into_iter()
            .find(|request| request.request_id == request_id)
            .ok_or(PairingError::Unavailable)?;
        let had_active_credential = {
            let credentials = self
                .peer_credentials
                .lock()
                .map_err(|_| PairingError::Unavailable)?;
            let device_already_active = credentials
                .iter()
                .any(|(device, _)| *device == request.device_id);
            if !device_already_active && credentials.len() >= 32 {
                return Err(PairingError::Capacity);
            }
            drop(credentials);
            device_already_active
        };
        let result = registry.approve(request_id, port.as_ref(), |grant| {
            if !self.active.load(Ordering::Acquire) {
                return Err(PairingError::Unavailable);
            }
            if grant.binding != self.binding {
                return Err(PairingError::InvalidRequest);
            }
            if !authorization
                .peer_is_authorized(self.binding, grant.device_id)
                .map_err(|_| pairing_provisioning_failure("authorization_activation_lookup"))?
            {
                return Err(pairing_provisioning_failure(
                    "authorization_activation_rejected",
                ));
            }
            let hash = *blake3::hash(grant.bearer_token.as_bytes()).as_bytes();
            let mut credentials = self
                .peer_credentials
                .lock()
                .map_err(|_| PairingError::Unavailable)?;
            if let Some((_, active_hash)) = credentials
                .iter()
                .find(|(device, _)| *device == grant.device_id)
            {
                if *active_hash == hash {
                    // Retried pairing reuses the same durable grant and is
                    // idempotent for the live authentication set.
                    return Ok(());
                }
                return Err(pairing_provisioning_failure("duplicate_active_credential"));
            }
            if credentials.iter().any(|(_, token)| *token == hash) {
                return Err(pairing_provisioning_failure("duplicate_active_credential"));
            }
            if credentials.len() >= 32 {
                return Err(PairingError::Capacity);
            }
            credentials.push((grant.device_id, hash));
            drop(credentials);
            Ok(())
        });
        if result.is_err() && !had_active_credential {
            self.peer_credentials
                .lock()
                .map_err(|_| PairingError::Unavailable)?
                .retain(|(device, _)| *device != request.device_id);
        }
        result
    }

    /// Reject a request through the local host UI.
    ///
    /// # Errors
    /// Rejects unavailable pairing or non-pending requests.
    pub fn deny_pairing_request(&self, request_id: &str) -> Result<(), PairingError> {
        self.pairing
            .as_ref()
            .ok_or(PairingError::Unavailable)?
            .0
            .deny(request_id)
    }

    /// Invalidate unfinished enrollments before stopping this host/root.
    ///
    /// # Errors
    /// Reports a failure to remove the exact unclaimed approval generation.
    pub fn cancel_pairing_requests(&self) -> Result<(), PairingError> {
        self.active.store(false, Ordering::Release);
        let Some((registry, port)) = self.pairing.as_ref() else {
            return Ok(());
        };
        registry.cancel_all(port.as_ref())
    }

    fn authenticated_device(&self, headers: &HeaderMap) -> Option<DeviceId> {
        let credentials = self.peer_credentials.lock().ok()?;
        authenticated_device(headers, &credentials)
    }

    /// Enable immutable snapshot enrollment endpoints.
    #[must_use]
    pub fn with_snapshots(mut self, snapshots: Arc<dyn HostSnapshotPort>) -> Self {
        self.snapshots = Some(snapshots);
        self
    }
    /// Enable durable immutable resource upload and download endpoints.
    #[must_use]
    pub fn with_resources(mut self, resources: Arc<dyn ResourceTransferPort>) -> Self {
        self.resources = Some(resources);
        self
    }
}

fn pairing_provisioning_failure(stage: &'static str) -> PairingError {
    tracing::warn!(target: "echo_sync::pairing", stage, "LAN pairing provisioning failed");
    PairingError::ProvisioningUnavailable
}

pub struct PeerCredential {
    pub device_id: DeviceId,
    pub bearer_token: Vec<u8>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CredentialConfigError {
    TokenTooShort,
    DuplicateDeviceOrToken,
    TooManyDevices,
}

impl std::fmt::Display for CredentialConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for CredentialConfigError {}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct ErrorBody {
    error: &'static str,
}

/// Build the host API. The router rejects unbounded request bodies and never
/// exposes transport or database implementation types in its responses.
pub fn host_router(state: HostApiState) -> Router {
    Router::new()
        .route("/v1/health", get(health))
        .route(
            "/v1/pairing/requests",
            post(request_pairing).layer(DefaultBodyLimit::max(8 * 1024)),
        )
        .route("/v1/mutations", post(submit_mutation))
        .route("/v1/commits", get(pull_commits))
        .route(
            "/v1/snapshots",
            get(pull_snapshot_page).post(begin_snapshot),
        )
        .route("/v1/resources/prepare", post(prepare_resource))
        .route("/v1/resources/{upload}/complete", post(complete_resource))
        .route(
            "/v1/resources/{upload}/chunks/{index}",
            axum::routing::put(upload_chunk).layer(DefaultBodyLimit::max(
                crate::protocol::MAX_RESOURCE_CHUNK_BYTES,
            )),
        )
        .route("/v1/resources/{hash}", get(download_resource))
        .layer(DefaultBodyLimit::max(MAX_MUTATION_BYTES))
        .with_state(state)
}

/// Build a TLS 1.3-only server configuration for the provisioned peer cert.
///
/// # Errors
/// Propagates invalid certificate chains, private keys, or key mismatches.
pub fn tls13_server_config(
    certificate_chain: Vec<CertificateDer<'static>>,
    private_key: PrivateKeyDer<'static>,
) -> Result<rustls::ServerConfig, rustls::Error> {
    let mut config =
        rustls::ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
            .with_no_client_auth()
            .with_single_cert(certificate_chain, private_key)?;
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(config)
}

/// Upper bound on one TCP connection's whole lifetime.
///
/// This is a guard against leaked half-open sockets, **not** a request
/// deadline. HTTP/1.1 keep-alive lets a healthy replica reuse a single
/// connection across many sync rounds (watch, pull, ack, chunk transfer), so a
/// short value here disconnects an idle-but-healthy peer and surfaces to it as
/// a transport failure. Every individual request is bounded by its handler
/// (a watch is at most 30s, a resource chunk is at most 4 MiB), and slow or
/// silent peers are bounded by [`HEADER_READ_TIMEOUT`] instead.
const CONNECTION_LIFETIME: Duration = Duration::from_secs(3_300);

/// Reap peers that complete the TLS handshake and then stop sending request
/// bytes. This, not the connection lifetime, is the slowloris bound.
const HEADER_READ_TIMEOUT: Duration = Duration::from_secs(15);

/// Serve the router over TLS 1.3. At most 64 TLS connections can be active;
/// excess sockets are dropped before a handshake allocates connection state.
///
/// # Errors
/// Propagates errors accepting connections from the listener.
pub async fn serve_tls(
    listener: TcpListener,
    config: rustls::ServerConfig,
    router: Router,
) -> std::io::Result<()> {
    let acceptor = TlsAcceptor::from(Arc::new(config));
    let connections = Arc::new(Semaphore::new(64));
    loop {
        let (socket, _) = listener.accept().await?;
        let Ok(permit) = Arc::clone(&connections).try_acquire_owned() else {
            drop(socket);
            continue;
        };
        let acceptor = acceptor.clone();
        let router = router.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let Ok(Ok(stream)) =
                tokio::time::timeout(Duration::from_secs(5), acceptor.accept(socket)).await
            else {
                return;
            };
            let service = TowerToHyperService::new(router);
            let connection = hyper::server::conn::http1::Builder::new()
                .header_read_timeout(HEADER_READ_TIMEOUT)
                .timer(TokioTimer::new())
                .serve_connection(TokioIo::new(stream), service);
            let _ = tokio::time::timeout(CONNECTION_LIFETIME, connection).await;
        });
    }
}

async fn health() -> &'static str {
    "echo-lan-sync-v1"
}

fn allow_pairing_request(state: &HostApiState) -> bool {
    if !state.active.load(Ordering::Acquire) {
        return false;
    }
    let Ok(mut window) = state.pairing_window.lock() else {
        return false;
    };
    if window.0.elapsed() >= Duration::from_secs(60) {
        *window = (Instant::now(), 0);
    }
    if window.1 >= 12 {
        return false;
    }
    window.1 += 1;
    true
}

async fn request_pairing(
    State(state): State<HostApiState>,
    Json(request): Json<PairingCodeRequest>,
) -> Response<Body> {
    if !allow_pairing_request(&state) {
        return api_error(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
    }
    if state.pairing.is_none() {
        return api_error(StatusCode::NOT_FOUND, "pairing_unavailable");
    }
    let result = tokio::task::spawn_blocking(move || state.pair_with_code(&request)).await;
    match result {
        Ok(Ok(pairing)) => (
            StatusCode::OK,
            [(axum::http::header::CACHE_CONTROL, "no-store")],
            Json(pairing),
        )
            .into_response(),
        Ok(Err(error)) => pairing_error(error),
        Err(_) => api_error(StatusCode::SERVICE_UNAVAILABLE, "pairing_unavailable"),
    }
}

fn pairing_error(error: PairingError) -> Response<Body> {
    match error {
        PairingError::InvalidRequest => {
            api_error(StatusCode::BAD_REQUEST, "invalid_pairing_request")
        }
        PairingError::Pending => api_error(StatusCode::CONFLICT, "approval_pending"),
        PairingError::Capacity => api_error(StatusCode::TOO_MANY_REQUESTS, "pairing_capacity"),
        PairingError::Unavailable => api_error(StatusCode::NOT_FOUND, "pairing_unavailable"),
        PairingError::ProvisioningUnavailable => api_error(
            StatusCode::SERVICE_UNAVAILABLE,
            "pairing_provisioning_unavailable",
        ),
    }
}

async fn submit_mutation(
    State(state): State<HostApiState>,
    headers: HeaderMap,
    Json(mutation): Json<MutationDto>,
) -> Response<Body> {
    let Some(authenticated_device) = state.authenticated_device(&headers) else {
        return api_error(StatusCode::UNAUTHORIZED, "unauthorized");
    };
    if !peer_is_currently_authorized(&state, authenticated_device).await {
        return api_error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    if !allow_request(&state, authenticated_device) {
        return api_error(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
    }
    if authenticated_device != mutation.origin_device_id {
        return api_error(StatusCode::FORBIDDEN, "origin_device_mismatch");
    }
    if mutation.binding != state.binding {
        return api_error(StatusCode::CONFLICT, "library_binding_mismatch");
    }
    if validate_mutation(&mutation).is_err() {
        return api_error(StatusCode::BAD_REQUEST, "invalid_mutation");
    }
    if !mutation.resource_refs.is_empty() {
        let Some(resources) = state.resources.as_ref().map(Arc::clone) else {
            return api_error(
                StatusCode::SERVICE_UNAVAILABLE,
                "resource_storage_unavailable",
            );
        };
        let binding = state.binding;
        let references = mutation.resource_refs.clone();
        match tokio::task::spawn_blocking(move || {
            references
                .iter()
                .try_for_each(|reference| resources.verify_complete(binding, reference))
        })
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(_)) => return api_error(StatusCode::CONFLICT, "resource_not_complete"),
            Err(_) => {
                return api_error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "resource_storage_unavailable",
                )
            }
        }
        // Revocation or stop may happen while a large file is being verified.
        if !peer_is_currently_authorized(&state, authenticated_device).await {
            return api_error(StatusCode::UNAUTHORIZED, "unauthorized");
        }
    }
    let Ok(resource_refs) = mutation
        .resource_refs
        .iter()
        .map(crate::protocol::ResourceRefDto::to_manifest)
        .collect::<Result<Vec<_>, _>>()
    else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_resource_reference");
    };
    let candidate = MutationCandidate {
        resource_refs,
        mutation_id: mutation.mutation_id,
        origin_device_id: mutation.origin_device_id,
        origin_seq: mutation.origin_seq,
        previous_mutation_id: mutation.previous_mutation_id,
        object: mutation.object,
        base_object_version: mutation.base_object_version,
        payload: mutation.payload,
    };
    let ledger = Arc::clone(&state.ledger);
    let checkpoint = Arc::clone(&state.checkpoint);
    let binding = state.binding;
    match tokio::task::spawn_blocking(move || {
        ConfirmHostMutation::new(ledger.as_ref(), checkpoint.as_ref()).execute(binding, &candidate)
    })
    .await
    {
        Ok(Ok(confirmation)) => {
            (StatusCode::OK, Json(MutationAck { confirmation })).into_response()
        }
        Ok(Err(_)) => api_error(StatusCode::CONFLICT, "mutation_rejected"),
        Err(_) => api_error(StatusCode::SERVICE_UNAVAILABLE, "host_unavailable"),
    }
}

#[derive(Deserialize)]
struct CommitQuery {
    after: u64,
    previous_digest: String,
    limit: usize,
    /// Optional bounded long-poll: hold an empty page open until the host
    /// publishes or the budget elapses. Omitted or zero keeps the immediate
    /// response; the bound is `MAX_COMMIT_WAIT_MS`.
    wait_ms: Option<u32>,
}

/// Internal cadence for re-reading the published watermark while a commit page
/// is held open. Each recheck is a rowid seek on `sync_commits`, so a slow
/// cadence keeps idle cost near zero without adding visible latency.
const COMMIT_RECHECK_INTERVAL: Duration = Duration::from_millis(500);

async fn pull_commits(
    State(state): State<HostApiState>,
    headers: HeaderMap,
    Query(query): Query<CommitQuery>,
) -> Response<Body> {
    let Some(device) = state.authenticated_device(&headers) else {
        return api_error(StatusCode::UNAUTHORIZED, "unauthorized");
    };
    if !peer_is_currently_authorized(&state, device).await {
        return api_error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    if !allow_request(&state, device) {
        return api_error(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
    }
    if query.limit == 0 || query.limit > MAX_PAGE_SIZE {
        return api_error(StatusCode::BAD_REQUEST, "invalid_page_size");
    }
    let wait = match query.wait_ms {
        Some(ms) if ms > MAX_COMMIT_WAIT_MS => {
            return api_error(StatusCode::BAD_REQUEST, "invalid_wait");
        }
        Some(ms) => Duration::from_millis(u64::from(ms)),
        None => Duration::ZERO,
    };
    let Ok(anchor_digest) = parse_digest(&query.previous_digest) else {
        return api_error(StatusCode::BAD_REQUEST, "invalid_anchor_digest");
    };
    let mut page = match read_commit_page(&state, query.after, anchor_digest, query.limit).await {
        Ok(page) => page,
        Err((status, message)) => return api_error(status, message),
    };
    // Holding an empty page open is the peer's only timely way to learn about
    // commits published by another device: the host cannot push to a
    // listening-less peer. The held request already consumed its rate-limit
    // slot, and stopping the host ends the hold on the next recheck.
    if page.entries.is_empty() && !wait.is_zero() {
        let deadline = Instant::now() + wait;
        while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
            if !state.active.load(Ordering::Acquire) {
                break;
            }
            tokio::time::sleep(remaining.min(COMMIT_RECHECK_INTERVAL)).await;
            if !state.active.load(Ordering::Acquire) {
                break;
            }
            page = match read_commit_page(&state, query.after, anchor_digest, query.limit).await {
                Ok(page) => page,
                Err((status, message)) => return api_error(status, message),
            };
            if !page.entries.is_empty() {
                break;
            }
        }
    }
    (StatusCode::OK, Json(page)).into_response()
}

/// Read one bounded commit page after the peer's checkpoint. The page build
/// runs on the blocking pool: the ledger port takes a pooled read connection,
/// which may queue behind a UI query, and a held long-poll rechecks this every
/// [`COMMIT_RECHECK_INTERVAL`] on the async runtime. Errors map to the same
/// host responses as the handler's first read.
async fn read_commit_page(
    state: &HostApiState,
    after: u64,
    anchor_digest: [u8; 32],
    limit: usize,
) -> Result<CommitPageDto, (StatusCode, &'static str)> {
    let ledger = Arc::clone(&state.ledger);
    let binding = state.binding;
    match tokio::task::spawn_blocking(move || {
        let after = echo_core::domain::sync::CommitSeq::from_u64(after);
        let commits = ledger
            .published_commits_after(binding, after, anchor_digest, limit)
            .map_err(|_| ())?;
        let mut page = CommitPageDto {
            binding,
            anchor_sequence: after,
            anchor_digest,
            entries: Vec::with_capacity(commits.len()),
        };
        let mut encoded_len = serde_json::to_vec(&page).map_err(|_| ())?.len();
        for PublishedCommit {
            sequence,
            previous_digest,
            digest,
            canonical_json,
        } in commits
        {
            let commit: CommitDto = serde_json::from_str(&canonical_json).map_err(|_| ())?;
            if commit.binding != binding
                || commit.commit_seq != sequence
                || commit.previous_digest != previous_digest
                || validate_mutation(&commit.mutation).is_err()
                || serde_json::to_vec(&commit).map_err(|_| ())?.len() > MAX_COMMIT_BYTES
                || commit
                    .resource_refs
                    .iter()
                    .any(|resource| validate_resource(resource).is_err())
                || commit_digest(&commit).map_err(|_| ())? != digest
            {
                return Err(());
            }
            let entry = CommittedEntryDto { commit, digest };
            let entry_len = serde_json::to_vec(&entry).map_err(|_| ())?.len();
            let extra = entry_len + usize::from(!page.entries.is_empty());
            if encoded_len.saturating_add(extra) > MAX_SYNC_PAGE_BYTES {
                if page.entries.is_empty() {
                    return Err(());
                }
                break;
            }
            encoded_len += extra;
            page.entries.push(entry);
        }
        Ok::<_, ()>(page)
    })
    .await
    {
        Ok(Ok(page)) => Ok(page),
        Ok(Err(())) => Err((StatusCode::CONFLICT, "commit_anchor_or_log_mismatch")),
        Err(_) => Err((StatusCode::SERVICE_UNAVAILABLE, "host_unavailable")),
    }
}

#[derive(Deserialize)]
struct BeginSnapshotQuery {
    limit: usize,
}

async fn begin_snapshot(
    State(state): State<HostApiState>,
    headers: HeaderMap,
    Query(query): Query<BeginSnapshotQuery>,
) -> Response<Body> {
    let Some(device) = state.authenticated_device(&headers) else {
        return api_error(StatusCode::UNAUTHORIZED, "unauthorized");
    };
    if !peer_is_currently_authorized(&state, device).await {
        return api_error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    if !allow_request(&state, device) {
        return api_error(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
    }
    if query.limit == 0 || query.limit > MAX_PAGE_SIZE {
        return api_error(StatusCode::BAD_REQUEST, "invalid_page_size");
    }
    let Some(snapshots) = state.snapshots.as_ref().map(Arc::clone) else {
        return api_error(StatusCode::NOT_IMPLEMENTED, "snapshots_unavailable");
    };
    let binding = state.binding;
    match tokio::task::spawn_blocking(move || {
        snapshots.create_host_snapshot(binding, query.limit, 15 * 60 * 1000)
    })
    .await
    {
        Ok(Ok(session)) => {
            snapshot_page_response(state.binding, session.token, 0, state.snapshots.clone()).await
        }
        Ok(Err(_)) => api_error(StatusCode::CONFLICT, "snapshot_unavailable"),
        Err(_) => api_error(StatusCode::SERVICE_UNAVAILABLE, "host_unavailable"),
    }
}

#[derive(Deserialize)]
struct SnapshotQuery {
    token: String,
    page: u32,
}

async fn pull_snapshot_page(
    State(state): State<HostApiState>,
    headers: HeaderMap,
    Query(query): Query<SnapshotQuery>,
) -> Response<Body> {
    let Some(device) = state.authenticated_device(&headers) else {
        return api_error(StatusCode::UNAUTHORIZED, "unauthorized");
    };
    if !peer_is_currently_authorized(&state, device).await {
        return api_error(StatusCode::UNAUTHORIZED, "unauthorized");
    }
    if !allow_request(&state, device) {
        return api_error(StatusCode::TOO_MANY_REQUESTS, "rate_limited");
    }
    if !valid_snapshot_token(&query.token) {
        return api_error(StatusCode::BAD_REQUEST, "invalid_snapshot_token");
    }
    snapshot_page_response(
        state.binding,
        query.token,
        query.page,
        state.snapshots.clone(),
    )
    .await
}

async fn peer_is_currently_authorized(state: &HostApiState, device: DeviceId) -> bool {
    if !state.active.load(Ordering::Acquire) {
        return false;
    }
    let Some(authorization) = state.authorization.as_ref().map(Arc::clone) else {
        return false;
    };
    let binding = state.binding;
    tokio::task::spawn_blocking(move || authorization.peer_is_authorized(binding, device))
        .await
        .is_ok_and(|result| result.unwrap_or(false))
}

async fn snapshot_page_response(
    binding: echo_core::domain::sync::SyncBinding,
    token: String,
    page_index: u32,
    snapshots: Option<Arc<dyn HostSnapshotPort>>,
) -> Response<Body> {
    let Some(snapshots) = snapshots else {
        return api_error(StatusCode::NOT_IMPLEMENTED, "snapshots_unavailable");
    };
    if !valid_snapshot_token(&token) {
        return api_error(StatusCode::CONFLICT, "snapshot_unavailable");
    }
    let result = tokio::task::spawn_blocking(move || {
        snapshots.host_snapshot_page(binding, &token, page_index)
    })
    .await;
    match result {
        Ok(Ok(page)) => {
            let mut objects = Vec::new();
            let mut aliases = Vec::new();
            for record in page.records {
                match record {
                    HostSnapshotRecord::Object {
                        candidate,
                        version,
                        resources: manifests,
                    } => {
                        objects.push(SnapshotObjectDto {
                            mutation: MutationDto {
                                resource_refs: candidate
                                    .resource_refs
                                    .iter()
                                    .map(crate::protocol::ResourceRefDto::from)
                                    .collect(),
                                protocol_version: PROTOCOL_VERSION,
                                binding,
                                origin_device_id: candidate.origin_device_id,
                                mutation_id: candidate.mutation_id,
                                origin_seq: candidate.origin_seq,
                                previous_mutation_id: candidate.previous_mutation_id,
                                object: candidate.object,
                                base_object_version: candidate.base_object_version,
                                payload: candidate.payload,
                            },
                            version,
                            resources: manifests
                                .into_iter()
                                .map(|resource| ResourceRefDto {
                                    kind: resource.kind,
                                    content_hash: resource.digest.to_hex(),
                                    length: resource.length,
                                    media_type: resource.media_type,
                                    relative_path: resource.relative_path.as_str().to_owned(),
                                })
                                .collect(),
                        });
                    }
                    HostSnapshotRecord::SongAlias { alias, canonical } => {
                        aliases.push(TypedAliasDto::Song(SongAliasDto { alias, canonical }));
                    }
                    HostSnapshotRecord::PlaylistMemberAlias(alias) => {
                        aliases.push(TypedAliasDto::PlaylistMember(MemberAliasDto {
                            playlist_id: alias.playlist_id,
                            alias: alias.alias_member_id,
                            canonical: alias.canonical_member_id,
                        }));
                    }
                }
            }
            let dto = SnapshotPageDto {
                snapshot_token: page.session.token,
                checkpoint: CheckpointDto {
                    binding,
                    commit_seq: page.session.checkpoint.sequence,
                    digest: page.session.checkpoint.digest,
                },
                page_index: page.page_index,
                next_page_index: page.next_page_index,
                objects,
                aliases,
            };
            if validate_snapshot_page(&dto).is_err()
                || serde_json::to_vec(&dto).map_or(true, |body| body.len() > MAX_SYNC_PAGE_BYTES)
            {
                api_error(StatusCode::CONFLICT, "invalid_snapshot_page")
            } else {
                (StatusCode::OK, Json(dto)).into_response()
            }
        }
        Ok(Err(_)) => api_error(StatusCode::GONE, "snapshot_expired_or_unknown"),
        Err(_) => api_error(StatusCode::SERVICE_UNAVAILABLE, "host_unavailable"),
    }
}

fn valid_snapshot_token(token: &str) -> bool {
    token.len() == 32
        && token
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

fn parse_digest(value: &str) -> Result<[u8; 32], ProtocolError> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(ProtocolError::InvalidResource);
    }
    let mut digest = [0; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16)
            .map_err(|_| ProtocolError::InvalidResource)?;
    }
    Ok(digest)
}

fn authenticated_device(headers: &HeaderMap, peers: &[(DeviceId, [u8; 32])]) -> Option<DeviceId> {
    let value = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|header| header.to_str().ok())?;
    let provided = value.strip_prefix("Bearer ")?;
    let digest = blake3::hash(provided.as_bytes());
    peers.iter().find_map(|(device, expected)| {
        constant_time_eq(digest.as_bytes(), expected).then_some(*device)
    })
}

/// Per-device request budgets, one window each for the two traffic classes.
///
/// Control traffic is timing-sensitive: a peer holds a long-poll and re-issues
/// it every budget (~25 s), so a minute of `429` there reads as an outage and
/// stops change discovery entirely. Resource traffic is throughput-bound: one
/// chunk carries `MAX_RESOURCE_CHUNK_BYTES`, so a request-counted allowance is
/// really a bandwidth cap and belongs in its own window.
const CONTROL_REQUESTS_PER_WINDOW: u16 = 120;
/// `900 * 4 MiB / 60 s` ≈ 60 MiB/s, above what the Wi-Fi links this feature
/// targets sustain, so the transport — not the limiter — is the constraint.
const RESOURCE_REQUESTS_PER_WINDOW: u16 = 900;
const REQUEST_WINDOW: Duration = Duration::from_secs(60);

fn allow_request(state: &HostApiState, device: DeviceId) -> bool {
    allow_request_in(&state.request_windows, device, CONTROL_REQUESTS_PER_WINDOW)
}

fn allow_resource_request(state: &HostApiState, device: DeviceId) -> bool {
    allow_request_in(
        &state.resource_windows,
        device,
        RESOURCE_REQUESTS_PER_WINDOW,
    )
}

fn allow_request_in(
    windows: &Mutex<HashMap<DeviceId, (Instant, u16)>>,
    device: DeviceId,
    budget: u16,
) -> bool {
    let now = Instant::now();
    let Ok(mut windows) = windows.lock() else {
        return false;
    };
    let (started, count) = windows.entry(device).or_insert((now, 0));
    if now.duration_since(*started) >= REQUEST_WINDOW {
        *started = now;
        *count = 0;
    }
    if *count >= budget {
        return false;
    }
    *count += 1;
    true
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    for index in 0..left.len().max(right.len()) {
        difference |= usize::from(
            left.get(index).copied().unwrap_or(0) ^ right.get(index).copied().unwrap_or(0),
        );
    }
    difference == 0
}

fn api_error(status: StatusCode, error: &'static str) -> Response<Body> {
    (status, Json(ErrorBody { error })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_comparison_rejects_prefix_length_and_content_mismatches() {
        let token = b"0123456789abcdef0123456789abcdef";
        let device = DeviceId::new();
        let peers = [(device, *blake3::hash(token).as_bytes())];
        let mut headers = HeaderMap::new();
        headers.insert(
            "authorization",
            "Bearer 0123456789abcdef0123456789abcdef".parse().unwrap(),
        );
        assert_eq!(authenticated_device(&headers, &peers), Some(device));
        headers.insert(
            "authorization",
            "Bearer 0123456789abcdef0123456789abcdeg".parse().unwrap(),
        );
        assert_eq!(authenticated_device(&headers, &peers), None);
        headers.insert(
            "authorization",
            "Basic 0123456789abcdef0123456789abcdef".parse().unwrap(),
        );
        assert_eq!(authenticated_device(&headers, &peers), None);
    }
}

#[cfg(test)]
#[path = "server_tests.rs"]
mod contract_tests;
