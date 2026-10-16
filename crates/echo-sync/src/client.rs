//! HTTPS-only client for submitting host mutations.

use std::time::Duration;

use crate::pairing::{PairingBootstrapResponse, PairingCodeRequest};

use crate::protocol::{
    commit_digest, validate_mutation, validate_resource, validate_snapshot_page, CommitPageDto,
    MutationAck, MutationDto, ProtocolError, SnapshotPageDto, MAX_COMMIT_BYTES, MAX_PAGE_SIZE,
    MAX_SYNC_PAGE_BYTES,
};
use crate::resource_transfer::{
    PrepareResource, ResourceTransferPort, TransferProgress, MAX_RESOURCE_BYTES,
};
use echo_core::application::ports::{
    AppliedRemoteCursor, AppliedSnapshotPage, HostSnapshotRecord, RemoteApplyPort, RemoteCommit,
    ReplicaSnapshotApplyPort, ReplicaSnapshotPage,
};
use echo_core::domain::sync::{
    CommitCheckpoint, DeviceId, MutationCandidate, PlaylistMemberAlias, ResourceDigest,
    SharedResourceManifest,
};
use reqwest::{Client, StatusCode, Url};
use serde::de::DeserializeOwned;

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
#[path = "client_tls.rs"]
mod tls;

#[derive(Clone)]
pub struct LanClient {
    http: Client,
    bearer_token: String,
    certificate_hash: [u8; 32],
}

#[derive(Debug)]
pub enum ClientError {
    InvalidEndpoint,
    InvalidTlsConfiguration(rustls::Error),
    InvalidMutation(ProtocolError),
    InvalidSnapshot(ProtocolError),
    Transport(reqwest::Error),
    HostRejected(StatusCode),
    InvalidResponse(reqwest::Error),
    InvalidResponseBody,
    InvalidCommitChain,
    RemoteApply(echo_core::error::Error),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidEndpoint => f.write_str("sync endpoint must use HTTPS"),
            Self::InvalidTlsConfiguration(_) => f.write_str("invalid pinned sync host certificate"),
            Self::InvalidMutation(error) => write!(f, "invalid sync mutation: {error}"),
            Self::InvalidSnapshot(error) => write!(f, "invalid sync snapshot page: {error}"),
            Self::Transport(_) => f.write_str("sync host is unavailable"),
            Self::HostRejected(status) => write!(f, "sync host rejected request ({status})"),
            Self::InvalidResponse(_) => f.write_str("sync host returned an invalid response"),
            Self::InvalidResponseBody => {
                f.write_str("sync host returned an invalid or oversized response body")
            }
            Self::InvalidCommitChain => {
                f.write_str("sync host returned a forked or invalid commit chain")
            }
            Self::RemoteApply(_) => f.write_str("could not apply confirmed sync commits"),
        }
    }
}
impl std::error::Error for ClientError {}

impl LanClient {
    /// Exchange a local six-digit pairing code before the first certificate
    /// pin exists. The returned certificate is pinned for all later traffic.
    /// This bootstrap is deliberately limited to the one pairing request.
    pub async fn pair_with_code(
        endpoint: &Url,
        code: &str,
        device_id: DeviceId,
        device_name: String,
    ) -> Result<PairingBootstrapResponse, ClientError> {
        if endpoint.scheme() != "https"
            || code.len() != 6
            || !code.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err(ClientError::InvalidEndpoint);
        }
        let http = Client::builder()
            .https_only(true)
            .danger_accept_invalid_certs(true)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(10))
            .connect_timeout(Duration::from_secs(5))
            .no_proxy()
            .build()
            .map_err(ClientError::Transport)?;
        let url = pairing_url(endpoint, "/v1/pairing/requests")?;
        let response = http
            .post(url)
            .json(&PairingCodeRequest {
                code: code.to_owned(),
                device_id,
                device_name,
            })
            .send()
            .await
            .map_err(ClientError::Transport)?;
        if !response.status().is_success() {
            return Err(ClientError::HostRejected(response.status()));
        }
        let pairing: PairingBootstrapResponse = read_bounded_json(response).await?;
        let invitation = &pairing.invitation;
        if invitation.invitation_secret != code
            || invitation.host_certificate_der.is_empty()
            || invitation.host_certificate_der.len() > 16 * 1024
            || invitation.expires_in_secs > 600
            || pairing.grant.binding != invitation.binding
            || pairing.grant.device_id != device_id
            || pairing.grant.bearer_token.len() < 32
            || pairing.grant.bearer_token.len() > 256
        {
            return Err(ClientError::InvalidResponseBody);
        }
        Ok(pairing)
    }

    /// Build a TLS 1.3 client pinned to the provisioned certificate's SPKI.
    /// The peer proves ownership of that key through the TLS handshake; DNS,
    /// public CAs, and system proxies cannot replace the paired identity. A
    /// renewed certificate using the same key remains valid for this pin.
    /// The token is kept only in this in-memory client and is never put in errors.
    ///
    /// # Errors
    /// Returns a TLS configuration error for malformed certificates, or a
    /// transport error if the HTTP client cannot be built.
    pub fn new(pinned_certificate_der: &[u8], bearer_token: String) -> Result<Self, ClientError> {
        let tls = tls::pinned_config(pinned_certificate_der)
            .map_err(ClientError::InvalidTlsConfiguration)?;
        let http = Client::builder()
            .https_only(true)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(15))
            .connect_timeout(Duration::from_secs(5))
            .no_proxy()
            .use_preconfigured_tls(tls)
            .build()
            .map_err(ClientError::Transport)?;
        Ok(Self {
            http,
            bearer_token,
            certificate_hash: *blake3::hash(pinned_certificate_der).as_bytes(),
        })
    }

    /// Submit a validated mutation to the HTTPS host endpoint.
    ///
    /// # Errors
    /// Rejects unsafe endpoints or invalid mutations and reports transport,
    /// host rejection, and response decoding failures.
    pub async fn submit_mutation(
        &self,
        endpoint: &Url,
        mutation: &MutationDto,
    ) -> Result<MutationAck, ClientError> {
        if endpoint.scheme() != "https"
            || endpoint.username() != ""
            || endpoint.password().is_some()
        {
            return Err(ClientError::InvalidEndpoint);
        }
        validate_mutation(mutation).map_err(ClientError::InvalidMutation)?;
        let mut url = endpoint.clone();
        let path = format!("{}/v1/mutations", endpoint.path().trim_end_matches('/'));
        url.set_path(&path);
        url.set_query(None);
        url.set_fragment(None);
        let response = self
            .http
            .post(url)
            .bearer_auth(&self.bearer_token)
            .json(mutation)
            .send()
            .await
            .map_err(ClientError::Transport)?;
        if !response.status().is_success() {
            return Err(ClientError::HostRejected(response.status()));
        }
        let ack: MutationAck = read_bounded_json(response).await?;
        validate_mutation_ack(&ack, mutation)?;
        Ok(ack)
    }

    /// Fetch and validate a page following the supplied commit checkpoint.
    ///
    /// # Errors
    /// Rejects invalid endpoints, limits, or commit chains and reports
    /// transport, host rejection, and response decoding failures.
    pub async fn pull_commits(
        &self,
        endpoint: &Url,
        after: u64,
        previous_digest: [u8; 32],
        limit: usize,
    ) -> Result<CommitPageDto, ClientError> {
        if endpoint.scheme() != "https"
            || endpoint.username() != ""
            || endpoint.password().is_some()
            || limit == 0
            || limit > MAX_PAGE_SIZE
        {
            return Err(ClientError::InvalidCommitChain);
        }
        let mut url = endpoint.clone();
        let path = format!("{}/v1/commits", endpoint.path().trim_end_matches('/'));
        url.set_path(&path);
        url.set_query(None);
        url.set_fragment(None);
        url.query_pairs_mut()
            .append_pair("after", &after.to_string())
            .append_pair("previous_digest", &hex_digest(&previous_digest))
            .append_pair("limit", &limit.to_string());
        let response = self
            .http
            .get(url)
            .bearer_auth(&self.bearer_token)
            .send()
            .await
            .map_err(ClientError::Transport)?;
        if !response.status().is_success() {
            return Err(ClientError::HostRejected(response.status()));
        }
        let page: CommitPageDto = read_bounded_json(response).await?;
        validate_commit_page(&page, after, previous_digest)?;
        Ok(page)
    }

    /// Create a fixed host snapshot and fetch its first page.
    ///
    /// # Errors
    /// Rejects unsafe endpoints and page sizes, then reports transport,
    /// response-bound, or snapshot validation failures.
    pub async fn begin_snapshot(
        &self,
        endpoint: &Url,
        page_size: usize,
    ) -> Result<SnapshotPageDto, ClientError> {
        if !valid_endpoint(endpoint) || page_size == 0 || page_size > MAX_PAGE_SIZE {
            return Err(ClientError::InvalidEndpoint);
        }
        let mut url = endpoint.clone();
        url.set_path(&format!(
            "{}/v1/snapshots",
            endpoint.path().trim_end_matches('/')
        ));
        url.set_query(None);
        url.set_fragment(None);
        url.query_pairs_mut()
            .append_pair("limit", &page_size.to_string());
        let response = self
            .http
            .post(url)
            .bearer_auth(&self.bearer_token)
            .send()
            .await
            .map_err(ClientError::Transport)?;
        if !response.status().is_success() {
            return Err(ClientError::HostRejected(response.status()));
        }
        let page: SnapshotPageDto = read_bounded_json(response).await?;
        validate_snapshot_page(&page).map_err(ClientError::InvalidSnapshot)?;
        Ok(page)
    }

    /// Fetch a later page from the same immutable host snapshot session.
    ///
    /// # Errors
    /// Rejects invalid handles/endpoints and reports transport, response-bound,
    /// expiration, or snapshot validation failures.
    pub async fn pull_snapshot_page(
        &self,
        endpoint: &Url,
        snapshot_token: &str,
        page_index: u32,
    ) -> Result<SnapshotPageDto, ClientError> {
        if !valid_endpoint(endpoint)
            || snapshot_token.len() != 32
            || !snapshot_token
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(ClientError::InvalidEndpoint);
        }
        let mut url = endpoint.clone();
        url.set_path(&format!(
            "{}/v1/snapshots",
            endpoint.path().trim_end_matches('/')
        ));
        url.set_query(None);
        url.set_fragment(None);
        url.query_pairs_mut()
            .append_pair("token", snapshot_token)
            .append_pair("page", &page_index.to_string());
        let response = self
            .http
            .get(url)
            .bearer_auth(&self.bearer_token)
            .send()
            .await
            .map_err(ClientError::Transport)?;
        if !response.status().is_success() {
            return Err(ClientError::HostRejected(response.status()));
        }
        let page: SnapshotPageDto = read_bounded_json(response).await?;
        validate_snapshot_page(&page).map_err(ClientError::InvalidSnapshot)?;
        if page.snapshot_token != snapshot_token || page.page_index != page_index {
            return Err(ClientError::InvalidCommitChain);
        }
        Ok(page)
    }

    /// Declare an immutable resource, or resume its owner-scoped upload.
    ///
    /// # Errors
    /// Rejects invalid metadata, endpoint, host response or transport failures.
    pub async fn prepare_resource(
        &self,
        endpoint: &Url,
        request: &PrepareResource,
    ) -> Result<TransferProgress, ClientError> {
        validate_resource(&request.resource).map_err(ClientError::InvalidMutation)?;
        if request.resource.length > MAX_RESOURCE_BYTES {
            return Err(ClientError::InvalidResponseBody);
        }
        let url = resource_url(endpoint, "/v1/resources/prepare")?;
        let response = self
            .http
            .post(url)
            .bearer_auth(&self.bearer_token)
            .header("x-echo-binding", binding_header(request.binding)?)
            .json(request)
            .send()
            .await
            .map_err(ClientError::Transport)?;
        let progress = resource_progress(response).await?;
        validate_transfer_progress(&progress, &request.resource)?;
        Ok(progress)
    }

    /// Upload one fixed-size chunk with an independent BLAKE3 digest.
    ///
    /// # Errors
    /// Rejects invalid chunks and mismatching upload acknowledgements.
    pub async fn upload_resource_chunk(
        &self,
        endpoint: &Url,
        binding: echo_core::domain::sync::SyncBinding,
        progress: &TransferProgress,
        index: u32,
        bytes: Vec<u8>,
    ) -> Result<TransferProgress, ClientError> {
        validate_transfer_progress(progress, &progress.resource)?;
        let offset = u64::from(index) * crate::protocol::MAX_RESOURCE_CHUNK_BYTES as u64;
        let expected = progress
            .resource
            .length
            .saturating_sub(offset)
            .min(crate::protocol::MAX_RESOURCE_CHUNK_BYTES as u64);
        if bytes.is_empty() || offset >= progress.resource.length || bytes.len() as u64 != expected
        {
            return Err(ClientError::InvalidResponseBody);
        }
        let hash = blake3::hash(&bytes).to_hex().to_string();
        let url = resource_url(
            endpoint,
            &format!("/v1/resources/{}/chunks/{index}", progress.upload_id),
        )?;
        let response = self
            .http
            .put(url)
            .bearer_auth(&self.bearer_token)
            .header("x-echo-binding", binding_header(binding)?)
            .header("x-echo-chunk-hash", hash)
            .body(bytes)
            .send()
            .await
            .map_err(ClientError::Transport)?;
        let updated = resource_progress(response).await?;
        validate_transfer_progress(&updated, &progress.resource)?;
        if updated.upload_id != progress.upload_id
            || (!updated.complete && !updated.confirmed_chunks.contains(&index))
        {
            return Err(ClientError::InvalidResponseBody);
        }
        Ok(updated)
    }

    /// Publish an upload only after the host verifies the full content digest.
    ///
    /// # Errors
    /// Reports rejected incomplete uploads or malformed acknowledgements.
    pub async fn complete_resource(
        &self,
        endpoint: &Url,
        binding: echo_core::domain::sync::SyncBinding,
        progress: &TransferProgress,
    ) -> Result<TransferProgress, ClientError> {
        validate_transfer_progress(progress, &progress.resource)?;
        let url = resource_url(
            endpoint,
            &format!("/v1/resources/{}/complete", progress.upload_id),
        )?;
        let response = self
            .http
            .post(url)
            .bearer_auth(&self.bearer_token)
            .header("x-echo-binding", binding_header(binding)?)
            .send()
            .await
            .map_err(ClientError::Transport)?;
        let updated = resource_progress(response).await?;
        validate_transfer_progress(&updated, &progress.resource)?;
        if updated.upload_id != progress.upload_id || !updated.complete {
            return Err(ClientError::InvalidResponseBody);
        }
        Ok(updated)
    }

    /// Fetch a bounded byte range, verifying its content version and chunk hash.
    ///
    /// # Errors
    /// Rejects inconsistent ranges, oversized bodies or digest mismatches.
    pub async fn download_resource_chunk(
        &self,
        endpoint: &Url,
        binding: echo_core::domain::sync::SyncBinding,
        resource: &crate::protocol::ResourceRefDto,
        index: u32,
    ) -> Result<Vec<u8>, ClientError> {
        validate_resource(resource).map_err(ClientError::InvalidMutation)?;
        if resource.length > MAX_RESOURCE_BYTES {
            return Err(ClientError::InvalidResponseBody);
        }
        let offset = u64::from(index) * crate::protocol::MAX_RESOURCE_CHUNK_BYTES as u64;
        let length = resource
            .length
            .saturating_sub(offset)
            .min(crate::protocol::MAX_RESOURCE_CHUNK_BYTES as u64);
        if length == 0 {
            return Err(ClientError::InvalidResponseBody);
        }
        let url = resource_url(
            endpoint,
            &format!("/v1/resources/{}", resource.content_hash),
        )?;
        let mut response = self
            .http
            .get(url)
            .bearer_auth(&self.bearer_token)
            .header("x-echo-binding", binding_header(binding)?)
            .header("if-match", format!("\"{}\"", resource.content_hash))
            .header("range", format!("bytes={offset}-{}", offset + length - 1))
            .send()
            .await
            .map_err(ClientError::Transport)?;
        if response.status() != StatusCode::PARTIAL_CONTENT {
            return Err(ClientError::HostRejected(response.status()));
        }
        let expected_range = format!("bytes {offset}-{}/{}", offset + length - 1, resource.length);
        let expected_etag = format!("\"{}\"", resource.content_hash);
        if response
            .headers()
            .get("content-range")
            .and_then(|value| value.to_str().ok())
            != Some(expected_range.as_str())
            || response
                .headers()
                .get("etag")
                .and_then(|value| value.to_str().ok())
                != Some(expected_etag.as_str())
            || response.content_length().is_some_and(|size| size != length)
        {
            return Err(ClientError::InvalidResponseBody);
        }
        let chunk_hash = response
            .headers()
            .get("x-echo-chunk-hash")
            .and_then(|value| value.to_str().ok())
            .ok_or(ClientError::InvalidResponseBody)?
            .to_owned();
        let bounded_length =
            usize::try_from(length).map_err(|_| ClientError::InvalidResponseBody)?;
        let mut bytes = Vec::with_capacity(bounded_length);
        while let Some(chunk) = response.chunk().await.map_err(ClientError::Transport)? {
            if bytes.len().saturating_add(chunk.len()) > bounded_length {
                return Err(ClientError::InvalidResponseBody);
            }
            bytes.extend_from_slice(&chunk);
        }
        if bytes.len() != bounded_length || blake3::hash(&bytes).to_hex().as_str() != chunk_hash {
            return Err(ClientError::InvalidResponseBody);
        }
        Ok(bytes)
    }

    /// Upload a library-authorized file using bounded reads and resumable chunks.
    /// The caller resolves this local path through Core's filesystem boundary.
    ///
    /// # Errors
    /// Reports changed source content, storage failures or rejected uploads.
    pub async fn upload_resource_file(
        &self,
        endpoint: &Url,
        request: &PrepareResource,
        path: std::path::PathBuf,
    ) -> Result<TransferProgress, ClientError> {
        let mut progress = self.prepare_resource(endpoint, request).await?;
        if progress.complete {
            return Ok(progress);
        }
        let chunks = request
            .resource
            .length
            .div_ceil(crate::protocol::MAX_RESOURCE_CHUNK_BYTES as u64);
        for index in 0..chunks {
            let index = u32::try_from(index).map_err(|_| ClientError::InvalidResponseBody)?;
            if progress.confirmed_chunks.contains(&index) {
                continue;
            }
            let source = path.clone();
            let total_length = request.resource.length;
            let bytes = tokio::task::spawn_blocking(move || {
                use std::io::{Read, Seek, SeekFrom};
                let mut file =
                    std::fs::File::open(source).map_err(|_| ClientError::InvalidResponseBody)?;
                if file
                    .metadata()
                    .map_err(|_| ClientError::InvalidResponseBody)?
                    .len()
                    != total_length
                {
                    return Err(ClientError::InvalidResponseBody);
                }
                let offset = u64::from(index) * crate::protocol::MAX_RESOURCE_CHUNK_BYTES as u64;
                let length =
                    (total_length - offset).min(crate::protocol::MAX_RESOURCE_CHUNK_BYTES as u64);
                file.seek(SeekFrom::Start(offset))
                    .map_err(|_| ClientError::InvalidResponseBody)?;
                let mut bytes =
                    vec![0; usize::try_from(length).map_err(|_| ClientError::InvalidResponseBody)?];
                file.read_exact(&mut bytes)
                    .map_err(|_| ClientError::InvalidResponseBody)?;
                Ok::<_, ClientError>(bytes)
            })
            .await
            .map_err(|_| ClientError::InvalidResponseBody)??;
            progress = self
                .upload_resource_chunk(endpoint, request.binding, &progress, index, bytes)
                .await?;
        }
        self.complete_resource(endpoint, request.binding, &progress)
            .await
    }

    /// Resume a resource into a durable local store and verify the full digest
    /// before marking it available. Only one bounded chunk is held in memory.
    ///
    /// # Errors
    /// Propagates transfer, local storage or full-file verification failures.
    pub async fn download_resource(
        &self,
        endpoint: &Url,
        owner: DeviceId,
        request: &PrepareResource,
        store: std::sync::Arc<dyn ResourceTransferPort>,
    ) -> Result<TransferProgress, ClientError> {
        let local_request = request.clone();
        let local_store = std::sync::Arc::clone(&store);
        let progress =
            tokio::task::spawn_blocking(move || local_store.prepare(owner, &local_request))
                .await
                .map_err(|_| ClientError::InvalidResponseBody)?
                .map_err(|_| ClientError::InvalidResponseBody)?;
        if progress.complete {
            return Ok(progress);
        }
        let chunks = request
            .resource
            .length
            .div_ceil(crate::protocol::MAX_RESOURCE_CHUNK_BYTES as u64);
        for index in 0..chunks {
            let index = u32::try_from(index).map_err(|_| ClientError::InvalidResponseBody)?;
            if progress.confirmed_chunks.contains(&index) {
                continue;
            }
            let bytes = self
                .download_resource_chunk(endpoint, request.binding, &request.resource, index)
                .await?;
            let hash = blake3::hash(&bytes).to_hex().to_string();
            let local_store = std::sync::Arc::clone(&store);
            let upload = progress.upload_id.clone();
            let binding = request.binding;
            tokio::task::spawn_blocking(move || {
                local_store.write_chunk(binding, owner, &upload, index, &hash, &bytes)
            })
            .await
            .map_err(|_| ClientError::InvalidResponseBody)?
            .map_err(|_| ClientError::InvalidResponseBody)?;
        }
        let binding = request.binding;
        tokio::task::spawn_blocking(move || store.complete(binding, owner, &progress.upload_id))
            .await
            .map_err(|_| ClientError::InvalidResponseBody)?
            .map_err(|_| ClientError::InvalidResponseBody)
    }

    /// Apply a verified host page through Core's no-outbox replica boundary.
    ///
    /// The supplied peer identity must be the already authenticated host for
    /// this binding; Core atomically persists the page and its cursor.
    ///
    /// # Errors
    /// Rejects invalid commit pages or propagates failures from local apply.
    pub fn apply_commit_page(
        page: &CommitPageDto,
        peer_device_id: DeviceId,
        local_apply: &dyn RemoteApplyPort,
    ) -> Result<AppliedRemoteCursor, ClientError> {
        apply_commit_page(page, peer_device_id, local_apply)
    }

    /// Apply one verified immutable snapshot page to an empty replica.
    ///
    /// Pages are durably staged by Core and become visible only with the final
    /// page, which advances the peer cursor to the frozen checkpoint.
    ///
    /// # Errors
    /// Rejects malformed pages or propagates an atomic Core staging failure.
    pub fn apply_snapshot_page(
        page: &SnapshotPageDto,
        peer_device_id: DeviceId,
        local_apply: &dyn ReplicaSnapshotApplyPort,
    ) -> Result<AppliedSnapshotPage, ClientError> {
        apply_snapshot_page(page, peer_device_id, local_apply)
    }
}

fn resource_url(endpoint: &Url, path: &str) -> Result<Url, ClientError> {
    if !valid_endpoint(endpoint) {
        return Err(ClientError::InvalidEndpoint);
    }
    let mut url = endpoint.clone();
    url.set_path(&format!("{}{path}", endpoint.path().trim_end_matches('/')));
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}

fn binding_header(binding: echo_core::domain::sync::SyncBinding) -> Result<String, ClientError> {
    serde_json::to_string(&binding).map_err(|_| ClientError::InvalidResponseBody)
}

async fn resource_progress(response: reqwest::Response) -> Result<TransferProgress, ClientError> {
    if !response.status().is_success() {
        return Err(ClientError::HostRejected(response.status()));
    }
    read_bounded_json(response).await
}

fn validate_transfer_progress(
    progress: &TransferProgress,
    resource: &crate::protocol::ResourceRefDto,
) -> Result<(), ClientError> {
    if crate::resource_transfer::validate_hash(&progress.upload_id).is_err()
        || progress.resource != *resource
        || progress.chunk_size != crate::protocol::MAX_RESOURCE_CHUNK_BYTES
        || resource.length > MAX_RESOURCE_BYTES
    {
        return Err(ClientError::InvalidResponseBody);
    }
    let chunks = resource
        .length
        .div_ceil(crate::protocol::MAX_RESOURCE_CHUNK_BYTES as u64);
    if progress.confirmed_chunks.len() as u64 > chunks
        || progress
            .confirmed_chunks
            .iter()
            .any(|index| u64::from(*index) >= chunks)
        || progress
            .confirmed_chunks
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
    {
        return Err(ClientError::InvalidResponseBody);
    }
    Ok(())
}

fn validate_mutation_ack(ack: &MutationAck, mutation: &MutationDto) -> Result<(), ClientError> {
    #[derive(serde::Deserialize)]
    struct Outcome {
        accepted: bool,
        reason: Option<String>,
    }
    let confirmation = &ack.confirmation;
    let outcome: Outcome = serde_json::from_str(&confirmation.outcome_json)
        .map_err(|_| ClientError::InvalidResponseBody)?;
    if confirmation.mutation_id != mutation.mutation_id
        || outcome.accepted != confirmation.checkpoint.is_some()
        || (outcome.accepted && outcome.reason.is_some())
        || (!outcome.accepted && outcome.reason.as_ref().is_none_or(String::is_empty))
        || confirmation.checkpoint.is_some_and(|checkpoint| {
            checkpoint.binding != mutation.binding || checkpoint.sequence.as_u64() == 0
        })
    {
        return Err(ClientError::InvalidResponseBody);
    }
    Ok(())
}

fn valid_endpoint(endpoint: &Url) -> bool {
    endpoint.scheme() == "https" && endpoint.username().is_empty() && endpoint.password().is_none()
}

async fn read_bounded_json<T: DeserializeOwned>(
    mut response: reqwest::Response,
) -> Result<T, ClientError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_SYNC_PAGE_BYTES as u64)
    {
        return Err(ClientError::InvalidResponseBody);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(ClientError::Transport)? {
        if body.len().saturating_add(chunk.len()) > MAX_SYNC_PAGE_BYTES {
            return Err(ClientError::InvalidResponseBody);
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| ClientError::InvalidResponseBody)
}

/// Apply a verified host page through Core's no-outbox replica boundary.
///
/// The supplied peer identity must be the already authenticated host for this
/// binding; Core atomically persists the page and its cursor.
///
/// # Errors
/// Rejects invalid commit pages or propagates failures from local apply.
pub fn apply_commit_page(
    page: &CommitPageDto,
    peer_device_id: DeviceId,
    local_apply: &dyn RemoteApplyPort,
) -> Result<AppliedRemoteCursor, ClientError> {
    validate_commit_page(page, page.anchor_sequence.as_u64(), page.anchor_digest)?;
    let commits = page
        .entries
        .iter()
        .map(|entry| {
            let mutation = &entry.commit.mutation;
            let payload_json = serde_json::to_string(&mutation.payload)
                .map_err(|_| ClientError::InvalidCommitChain)?;
            let canonical_json = serde_json::to_string(&entry.commit)
                .map_err(|_| ClientError::InvalidCommitChain)?;
            Ok(RemoteCommit {
                sequence: entry.commit.commit_seq,
                previous_digest: entry.commit.previous_digest,
                digest: entry.digest,
                origin_device_id: mutation.origin_device_id,
                mutation_id: mutation.mutation_id,
                object: mutation.object,
                is_tombstone: mutation.payload == serde_json::json!({"tombstone": true}),
                payload_json,
                canonical_json,
            })
        })
        .collect::<Result<Vec<_>, ClientError>>()?;
    local_apply
        .apply_remote_batch(
            page.binding,
            peer_device_id,
            echo_core::domain::sync::CommitCheckpoint {
                binding: page.binding,
                sequence: page.anchor_sequence,
                digest: page.anchor_digest,
            },
            &commits,
        )
        .map_err(ClientError::RemoteApply)
}

/// Convert a validated wire page into Core's durable snapshot enrollment API.
///
/// # Errors
/// Rejects malformed resource metadata or Core enrollment failures.
pub fn apply_snapshot_page(
    page: &SnapshotPageDto,
    peer_device_id: DeviceId,
    local_apply: &dyn ReplicaSnapshotApplyPort,
) -> Result<AppliedSnapshotPage, ClientError> {
    validate_snapshot_page(page).map_err(ClientError::InvalidSnapshot)?;
    let records = page
        .objects
        .iter()
        .map(|object| {
            let mutation = &object.mutation;
            let resources = object
                .resources
                .iter()
                .map(|resource| {
                    let digest = parse_digest(&resource.content_hash)
                        .ok_or(ClientError::InvalidSnapshot(ProtocolError::InvalidResource))?;
                    SharedResourceManifest::new(
                        resource.kind,
                        ResourceDigest::from_bytes(digest),
                        resource.length,
                        resource.media_type.clone(),
                        &resource.relative_path,
                    )
                    .map_err(|_| ClientError::InvalidSnapshot(ProtocolError::InvalidResource))
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(HostSnapshotRecord::Object {
                candidate: MutationCandidate {
                    resource_refs: resources.clone(),
                    mutation_id: mutation.mutation_id,
                    origin_device_id: mutation.origin_device_id,
                    origin_seq: mutation.origin_seq,
                    previous_mutation_id: mutation.previous_mutation_id,
                    object: mutation.object,
                    base_object_version: mutation.base_object_version,
                    payload: mutation.payload.clone(),
                },
                version: object.version,
                resources,
            })
        })
        .collect::<Result<Vec<_>, ClientError>>()?;
    let mut records = records;
    records.extend(page.aliases.iter().map(|alias| match alias {
        crate::protocol::TypedAliasDto::Song(alias) => HostSnapshotRecord::SongAlias {
            alias: alias.alias,
            canonical: alias.canonical,
        },
        crate::protocol::TypedAliasDto::PlaylistMember(alias) => {
            HostSnapshotRecord::PlaylistMemberAlias(PlaylistMemberAlias {
                playlist_id: alias.playlist_id,
                alias_member_id: alias.alias,
                canonical_member_id: alias.canonical,
            })
        }
    }));
    let replica_page = ReplicaSnapshotPage {
        snapshot_token: page.snapshot_token.clone(),
        checkpoint: CommitCheckpoint {
            binding: page.checkpoint.binding,
            sequence: page.checkpoint.commit_seq,
            digest: page.checkpoint.digest,
        },
        page_index: page.page_index,
        next_page_index: page.next_page_index,
        records,
    };
    local_apply
        .apply_snapshot_page(peer_device_id, &replica_page)
        .map_err(ClientError::RemoteApply)
}

fn parse_digest(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return None;
    }
    let mut digest = [0; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(digest)
}

fn validate_commit_page(
    page: &CommitPageDto,
    after: u64,
    previous_digest: [u8; 32],
) -> Result<(), ClientError> {
    if page.anchor_sequence.as_u64() != after
        || page.anchor_digest != previous_digest
        || page.entries.len() > MAX_PAGE_SIZE
    {
        return Err(ClientError::InvalidCommitChain);
    }
    let mut sequence = after;
    let mut digest = previous_digest;
    for entry in &page.entries {
        validate_mutation(&entry.commit.mutation).map_err(ClientError::InvalidMutation)?;
        if entry.commit.mutation.binding != page.binding
            || serde_json::to_vec(&entry.commit)
                .map_err(|_| ClientError::InvalidCommitChain)?
                .len()
                > MAX_COMMIT_BYTES
        {
            return Err(ClientError::InvalidCommitChain);
        }
        if !entry.commit.mutation.resource_refs.is_empty()
            && entry.commit.resource_refs != entry.commit.mutation.resource_refs
        {
            return Err(ClientError::InvalidCommitChain);
        }
        for resource in &entry.commit.resource_refs {
            validate_resource(resource).map_err(ClientError::InvalidMutation)?;
        }
        sequence = sequence
            .checked_add(1)
            .ok_or(ClientError::InvalidCommitChain)?;
        if entry.commit.binding != page.binding
            || entry.commit.commit_seq.as_u64() != sequence
            || entry.commit.previous_digest != digest
            || commit_digest(&entry.commit).map_err(ClientError::InvalidMutation)? != entry.digest
        {
            return Err(ClientError::InvalidCommitChain);
        }
        digest = entry.digest;
    }
    Ok(())
}

fn hex_digest(digest: &[u8; 32]) -> String {
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

fn pairing_url(endpoint: &Url, suffix: &str) -> Result<Url, ClientError> {
    if endpoint.scheme() != "https" || endpoint.username() != "" || endpoint.password().is_some() {
        return Err(ClientError::InvalidEndpoint);
    }
    let mut url = endpoint.clone();
    url.set_path(&format!(
        "{}{suffix}",
        endpoint.path().trim_end_matches('/')
    ));
    url.set_query(None);
    url.set_fragment(None);
    Ok(url)
}
