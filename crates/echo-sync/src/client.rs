//! HTTPS-only client for submitting host mutations.

use std::time::Duration;

use crate::pairing::{PairingBootstrapResponse, PairingCodeRequest};

use crate::protocol::{
    commit_digest, validate_mutation, validate_resource, validate_snapshot_page, CommitPageDto,
    MutationAck, MutationDto, ProtocolError, SnapshotPageDto, MAX_COMMIT_BYTES, MAX_COMMIT_WAIT_MS,
    MAX_PAGE_SIZE, MAX_SYNC_PAGE_BYTES,
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

mod apply;
#[path = "client_pairing.rs"]
mod pairing;

#[allow(unused_imports, clippy::wildcard_imports)]
use apply::*;
#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
#[path = "client_tls.rs"]
mod tls;

#[derive(Clone)]
pub struct LanClient {
    http: Client,
    bearer_token: String,
}

#[derive(Debug)]
pub enum ClientError {
    InvalidEndpoint,
    InvalidTlsConfiguration(rustls::Error),
    InvalidMutation(ProtocolError),
    InvalidSnapshot(ProtocolError),
    Transport(reqwest::Error),
    HostRejected(StatusCode),
    /// A host rejection with its bounded, structured API error code.
    HostRejectedCode(StatusCode, String),
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
            Self::HostRejectedCode(status, code) => {
                write!(f, "sync host rejected request ({status}: {code})")
            }
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
        Ok(Self { http, bearer_token })
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
        let response = send_sync_request(
            self.http
                .post(url)
                .bearer_auth(&self.bearer_token)
                .json(mutation),
        )
        .await?;
        if !response.status().is_success() {
            return Err(host_rejected(response).await);
        }
        let ack: MutationAck = read_bounded_json(response).await?;
        validate_mutation_ack(&ack, mutation)?;
        Ok(ack)
    }

    /// Fetch and validate a page following the supplied commit checkpoint.
    ///
    /// `wait` asks the host to hold an empty page open until it publishes or
    /// the budget elapses; `Duration::ZERO` keeps the immediate response. The
    /// request timeout is extended past the wait so a held page is never cut
    /// off by the client default.
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
        wait: Duration,
    ) -> Result<CommitPageDto, ClientError> {
        if endpoint.scheme() != "https"
            || endpoint.username() != ""
            || endpoint.password().is_some()
            || limit == 0
            || limit > MAX_PAGE_SIZE
            || wait > Duration::from_millis(u64::from(MAX_COMMIT_WAIT_MS))
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
        if !wait.is_zero() {
            url.query_pairs_mut()
                .append_pair("wait_ms", &wait.as_millis().to_string());
        }
        let mut request = self.http.get(url).bearer_auth(&self.bearer_token);
        if !wait.is_zero() {
            // A held page outlives the client's default request timeout; the
            // margin covers the host's final recheck and response encoding.
            request = request.timeout(wait + Duration::from_secs(5));
        }
        let response = send_sync_request(request).await?;
        if !response.status().is_success() {
            return Err(host_rejected(response).await);
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
        let response =
            send_sync_request(self.http.post(url).bearer_auth(&self.bearer_token)).await?;
        if !response.status().is_success() {
            return Err(host_rejected(response).await);
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
        let response =
            send_sync_request(self.http.get(url).bearer_auth(&self.bearer_token)).await?;
        if !response.status().is_success() {
            return Err(host_rejected(response).await);
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
        let response = send_sync_request(
            self.http
                .post(url)
                .bearer_auth(&self.bearer_token)
                .header("x-echo-binding", binding_header(request.binding)?)
                .json(request),
        )
        .await?;
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
        let response = send_sync_request(
            self.http
                .put(url)
                .bearer_auth(&self.bearer_token)
                .header("x-echo-binding", binding_header(binding)?)
                .header("x-echo-chunk-hash", hash)
                .body(bytes),
        )
        .await?;
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
        let response = send_sync_request(
            self.http
                .post(url)
                .bearer_auth(&self.bearer_token)
                .header("x-echo-binding", binding_header(binding)?),
        )
        .await?;
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
        let mut response = send_sync_request(
            self.http
                .get(url)
                .bearer_auth(&self.bearer_token)
                .header("x-echo-binding", binding_header(binding)?)
                .header("if-match", format!("\"{}\"", resource.content_hash))
                .header("range", format!("bytes={offset}-{}", offset + length - 1)),
        )
        .await?;
        if response.status() != StatusCode::PARTIAL_CONTENT {
            return Err(host_rejected(response).await);
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

// Sync operations carry stable mutation IDs, snapshot tokens or chunk identities.
// Retry explicit throttling only; pairing and transport failures are not replayed.
async fn send_sync_request(
    request: reqwest::RequestBuilder,
) -> Result<reqwest::Response, ClientError> {
    send_sync_request_with_delay(request, Duration::from_secs(60)).await
}

async fn send_sync_request_with_delay(
    request: reqwest::RequestBuilder,
    delay: Duration,
) -> Result<reqwest::Response, ClientError> {
    for attempt in 0..=2 {
        let next = request
            .try_clone()
            .ok_or(ClientError::InvalidResponseBody)?;
        let response = next.send().await.map_err(ClientError::Transport)?;
        if response.status() != reqwest::StatusCode::TOO_MANY_REQUESTS || attempt == 2 {
            return Ok(response);
        }
        // The host uses a fixed 60-second device window. Dropping the enclosing
        // sync future on pause/stop also cancels this asynchronous wait.
        drop(response);
        tokio::time::sleep(delay).await;
    }
    unreachable!("bounded request retries always return a response")
}

#[cfg(all(test, feature = "server"))]
mod throttling_tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn throttled_sync_replays_identical_body_and_bounds_retries() {
        for (failures, failure_status, expected_attempts, final_status) in [
            (1, StatusCode::TOO_MANY_REQUESTS, 2, StatusCode::OK),
            (
                5,
                StatusCode::TOO_MANY_REQUESTS,
                3,
                StatusCode::TOO_MANY_REQUESTS,
            ),
            (1, StatusCode::FORBIDDEN, 1, StatusCode::FORBIDDEN),
        ] {
            let bodies = Arc::new(Mutex::new(Vec::new()));
            let received = Arc::clone(&bodies);
            let app = axum::Router::new().route(
                "/",
                axum::routing::post(move |body: String| {
                    let received = Arc::clone(&received);
                    async move {
                        let mut bodies = received.lock().unwrap();
                        bodies.push(body);
                        if bodies.len() <= failures {
                            failure_status
                        } else {
                            StatusCode::OK
                        }
                    }
                }),
            );
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
            let response = send_sync_request_with_delay(
                Client::new()
                    .post(format!("http://{address}/"))
                    .body("frozen-mutation-and-chunk"),
                Duration::from_millis(1),
            )
            .await
            .unwrap();
            assert_eq!(response.status(), final_status);
            assert_eq!(
                *bodies.lock().unwrap(),
                vec!["frozen-mutation-and-chunk"; expected_attempts]
            );
            server.abort();
        }
    }
}

async fn resource_progress(response: reqwest::Response) -> Result<TransferProgress, ClientError> {
    if !response.status().is_success() {
        return Err(host_rejected(response).await);
    }
    read_bounded_json(response).await
}

pub(super) async fn host_rejected(response: reqwest::Response) -> ClientError {
    #[derive(serde::Deserialize)]
    struct ErrorBody {
        error: String,
    }
    let status = response.status();
    match read_bounded_json::<ErrorBody>(response).await {
        Ok(body)
            if !body.error.is_empty()
                && body.error.len() <= 96
                && body.error.bytes().all(|byte| {
                    byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_'
                }) =>
        {
            ClientError::HostRejectedCode(status, body.error)
        }
        _ => ClientError::HostRejected(status),
    }
}
