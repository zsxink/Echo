//! HTTP boundary contracts using ports rather than a database or real library.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use axum::http::Request;
use echo_core::application::ports::StagedConfirmation;
use echo_core::domain::ids::SongId;
use echo_core::domain::sync::{
    AuthorityId, CommitCheckpoint, CommitSeq, Epoch, LibraryId, MutationId, ObjectKey,
};
use echo_core::error::Error;
use hyper::service::Service;

use super::*;

const TOKEN: &str = "0123456789abcdef0123456789abcdef";

#[derive(Default)]
struct Ledger {
    reads: AtomicUsize,
    staged: Mutex<Option<(MutationCandidate, StagedConfirmation)>>,
    published: AtomicUsize,
    commits: Mutex<Vec<PublishedCommit>>,
}

impl HostCommitLedgerPort for Ledger {
    fn stage_confirmation(
        &self,
        binding: SyncBinding,
        candidate: &MutationCandidate,
    ) -> Result<StagedConfirmation, Error> {
        let mut staged = self.staged.lock().unwrap();
        if let Some((prior, receipt)) = staged.as_ref() {
            if prior != candidate {
                return Err(Error::conflict("mutation identity reused"));
            }
            return Ok(receipt.clone());
        }
        let receipt = StagedConfirmation {
            mutation_id: candidate.mutation_id,
            outcome_json: "{\"accepted\":true}".to_owned(),
            checkpoint: Some(CommitCheckpoint {
                binding,
                sequence: CommitSeq::from_u64(4),
                digest: [4; 32],
            }),
        };
        *staged = Some((candidate.clone(), receipt.clone()));
        drop(staged);
        Ok(receipt)
    }

    fn mark_published(&self, _checkpoint: CommitCheckpoint) -> Result<(), Error> {
        self.published.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    fn pending_publications(&self, _limit: usize) -> Result<Vec<StagedConfirmation>, Error> {
        Ok(Vec::new())
    }

    fn published_commits_after(
        &self,
        _binding: SyncBinding,
        after: CommitSeq,
        expected_digest: [u8; 32],
        _limit: usize,
    ) -> Result<Vec<PublishedCommit>, Error> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        if after != CommitSeq::from_u64(3) || expected_digest != [3; 32] {
            return Err(Error::conflict("anchor checkpoint mismatch"));
        }
        Ok(self.commits.lock().unwrap().clone())
    }
}

#[derive(Default)]
struct Checkpoint {
    fail_once: AtomicBool,
}

impl PublicationCheckpointPort for Checkpoint {
    fn advance(&self, _checkpoint: CommitCheckpoint) -> Result<(), Error> {
        if self.fail_once.swap(false, Ordering::Relaxed) {
            return Err(Error::unavailable("checkpoint", "injected failure"));
        }
        Ok(())
    }
}

struct Authorization(AtomicBool);

impl HostPeerAuthorizationPort for Authorization {
    fn peer_is_authorized(&self, _binding: SyncBinding, _device: DeviceId) -> Result<bool, Error> {
        Ok(self.0.load(Ordering::Relaxed))
    }
}

struct Fixture {
    state: HostApiState,
    device: DeviceId,
    ledger: Arc<Ledger>,
    checkpoint: Arc<Checkpoint>,
    authorization: Arc<Authorization>,
    mutation: MutationDto,
}

impl Fixture {
    fn new() -> Self {
        let binding = SyncBinding {
            library_id: LibraryId::new(),
            authority_id: AuthorityId::new(),
            epoch: Epoch::INITIAL,
        };
        let device = DeviceId::new();
        let ledger = Arc::new(Ledger::default());
        let checkpoint = Arc::new(Checkpoint::default());
        let authorization = Arc::new(Authorization(AtomicBool::new(true)));
        let state = HostApiState::new(
            binding,
            vec![PeerCredential {
                device_id: device,
                bearer_token: TOKEN.as_bytes().to_vec(),
            }],
            ledger.clone(),
            checkpoint.clone(),
        )
        .unwrap()
        .with_authorization(authorization.clone());
        let mutation = MutationDto {
            protocol_version: PROTOCOL_VERSION,
            binding,
            origin_device_id: device,
            mutation_id: MutationId::new(),
            origin_seq: 1,
            previous_mutation_id: None,
            object: ObjectKey::Favorite(SongId::new()),
            base_object_version: None,
            changed_at_ms: None,
            payload: serde_json::json!({"favorite": true}),
            resource_refs: Vec::new(),
        };
        Self {
            state,
            device,
            ledger,
            checkpoint,
            authorization,
            mutation,
        }
    }

    async fn request(
        &self,
        method: &str,
        uri: &str,
        token: Option<&str>,
        body: String,
    ) -> Response {
        self.request_headers(method, uri, token, body, &[]).await
    }

    async fn request_headers(
        &self,
        method: &str,
        uri: &str,
        token: Option<&str>,
        body: String,
        headers: &[(&str, &str)],
    ) -> Response {
        let mut request = Request::builder().method(method).uri(uri);
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        for (key, value) in headers {
            request = request.header(*key, *value);
        }
        let request = request
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap();
        TowerToHyperService::new(host_router(self.state.clone()))
            .call(request)
            .await
            .unwrap()
    }

    async fn submit(&self, mutation: &MutationDto) -> Response {
        self.request(
            "POST",
            "/v1/mutations",
            Some(TOKEN),
            serde_json::to_string(mutation).unwrap(),
        )
        .await
    }
}

fn commits_uri(after: u64, digest: u8, limit: usize) -> String {
    format!(
        "/v1/commits?after={after}&previous_digest={}&limit={limit}",
        format!("{digest:02x}").repeat(32)
    )
}

/// One valid published commit following the fixture's `(3, [3;32])` anchor.
fn published_commit(mutation: &MutationDto) -> PublishedCommit {
    let commit = CommitDto {
        protocol_version: PROTOCOL_VERSION,
        binding: mutation.binding,
        commit_seq: CommitSeq::from_u64(4),
        previous_digest: [3; 32],
        mutation: mutation.clone(),
        aliases: Vec::new(),
        resource_refs: Vec::new(),
    };
    let digest = commit_digest(&commit).unwrap();
    PublishedCommit {
        sequence: CommitSeq::from_u64(4),
        previous_digest: [3; 32],
        digest,
        canonical_json: serde_json::to_string(&commit).unwrap(),
    }
}

async fn commit_page_body(response: Response) -> CommitPageDto {
    let bytes = axum::body::to_bytes(response.into_body(), MAX_SYNC_PAGE_BYTES)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[test]
fn resource_chunks_do_not_spend_the_control_budget() {
    let fixture = Fixture::new();
    // A bulk upload is sized in chunks, not in requests: one chunk is 4 MiB, so
    // spending the long-poll's allowance on transfers stops change discovery
    // for the rest of the window. The two classes must not share a counter.
    for _ in 0..CONTROL_REQUESTS_PER_WINDOW {
        assert!(allow_request(&fixture.state, fixture.device));
    }
    assert!(!allow_request(&fixture.state, fixture.device));
    assert!(
        allow_resource_request(&fixture.state, fixture.device),
        "a saturated control window must not throttle resource chunks"
    );
    for _ in 1..RESOURCE_REQUESTS_PER_WINDOW {
        assert!(allow_resource_request(&fixture.state, fixture.device));
    }
    assert!(!allow_resource_request(&fixture.state, fixture.device));
    assert!(
        !allow_request(&fixture.state, fixture.device),
        "resource pressure must not reopen the control window"
    );
}

#[tokio::test]
async fn held_commit_page_returns_as_soon_as_the_host_publishes() {
    let fixture = Fixture::new();
    let ledger = Arc::clone(&fixture.ledger);
    let commit = published_commit(&fixture.mutation);
    // The host publishes while the peer's page is being held open.
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(150)).await;
        ledger.commits.lock().unwrap().push(commit);
    });
    let started = Instant::now();
    let response = fixture
        .request(
            "GET",
            &format!("{}&wait_ms=8000", commits_uri(3, 3, 10)),
            Some(TOKEN),
            String::new(),
        )
        .await;
    let elapsed = started.elapsed();
    assert_eq!(response.status(), StatusCode::OK);
    let page = commit_page_body(response).await;
    assert_eq!(
        page.entries.len(),
        1,
        "a held page must deliver a commit published during the hold"
    );
    assert!(
        elapsed < Duration::from_secs(4),
        "the hold must end when the host publishes, not when the budget elapses: {elapsed:?}"
    );
    assert!(
        fixture.ledger.reads.load(Ordering::Relaxed) >= 2,
        "the hold must re-read the published watermark instead of returning at once"
    );
}

#[tokio::test]
async fn held_commit_page_returns_empty_after_the_wait_budget() {
    let fixture = Fixture::new();
    let started = Instant::now();
    let response = fixture
        .request(
            "GET",
            &format!("{}&wait_ms=700", commits_uri(3, 3, 10)),
            Some(TOKEN),
            String::new(),
        )
        .await;
    let elapsed = started.elapsed();
    assert_eq!(response.status(), StatusCode::OK);
    let page = commit_page_body(response).await;
    assert!(page.entries.is_empty());
    assert!(
        elapsed >= Duration::from_millis(650),
        "the requested wait budget must be honored before an empty page returns: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(3),
        "unexpected overrun: {elapsed:?}"
    );
    assert!(
        fixture.ledger.reads.load(Ordering::Relaxed) >= 2,
        "an idle hold must still recheck the watermark"
    );
}

#[tokio::test]
async fn held_commit_page_rejects_a_wait_above_the_bound_but_keeps_zero_immediate() {
    let fixture = Fixture::new();
    assert_error(
        fixture
            .request(
                "GET",
                &format!(
                    "{}&wait_ms={}",
                    commits_uri(3, 3, 10),
                    MAX_COMMIT_WAIT_MS + 1
                ),
                Some(TOKEN),
                String::new(),
            )
            .await,
        StatusCode::BAD_REQUEST,
        "invalid_wait",
    )
    .await;
    assert_eq!(
        fixture.ledger.reads.load(Ordering::Relaxed),
        0,
        "a rejected wait must not touch the ledger"
    );
    let started = Instant::now();
    let response = fixture
        .request(
            "GET",
            &format!("{}&wait_ms=0", commits_uri(3, 3, 10)),
            Some(TOKEN),
            String::new(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(commit_page_body(response).await.entries.is_empty());
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "wait_ms=0 must keep the immediate response"
    );
}

#[tokio::test]
async fn stopping_the_host_ends_a_held_commit_page() {
    let fixture = Fixture::new();
    let state = fixture.state.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(120)).await;
        state.active.store(false, Ordering::Release);
    });
    let started = Instant::now();
    let response = fixture
        .request(
            "GET",
            &format!("{}&wait_ms=8000", commits_uri(3, 3, 10)),
            Some(TOKEN),
            String::new(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert!(
        started.elapsed() >= Duration::from_millis(400),
        "the request must actually be held before the stop ends it"
    );
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "stopping the host must end the hold instead of serving the full budget"
    );
    assert_error(response, StatusCode::UNAUTHORIZED, "unauthorized").await;
}

async fn assert_error(response: Response, status: StatusCode, error: &str) {
    assert_eq!(response.status(), status);
    if status == StatusCode::UNAUTHORIZED {
        assert_eq!(response.headers()[axum::http::header::CONNECTION], "close");
    }
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
        serde_json::json!({"error": error})
    );
}

#[tokio::test]
async fn protected_routes_require_credentials_and_live_authorization() {
    let fixture = Fixture::new();
    let routes = [
        ("GET", commits_uri(3, 3, 10), String::new()),
        (
            "POST",
            "/v1/mutations".to_owned(),
            serde_json::to_string(&fixture.mutation).unwrap(),
        ),
        ("POST", "/v1/snapshots?limit=10".to_owned(), String::new()),
        (
            "GET",
            format!("/v1/snapshots?token={}&page=0", "a".repeat(32)),
            String::new(),
        ),
        (
            "GET",
            format!("/v1/resources/{}", "a".repeat(64)),
            String::new(),
        ),
    ];
    for (method, uri, body) in &routes {
        for token in [None, Some("invalid-token")] {
            assert_error(
                fixture.request(method, uri, token, body.clone()).await,
                StatusCode::UNAUTHORIZED,
                "unauthorized",
            )
            .await;
        }
    }
    fixture.authorization.0.store(false, Ordering::Relaxed);
    for (method, uri, body) in &routes {
        assert_error(
            fixture
                .request(method, uri, Some(TOKEN), body.clone())
                .await,
            StatusCode::UNAUTHORIZED,
            "unauthorized",
        )
        .await;
    }
    assert_eq!(fixture.ledger.reads.load(Ordering::Relaxed), 0);
    assert!(fixture.ledger.staged.lock().unwrap().is_none());
}

#[tokio::test]
async fn commit_anchor_forks_and_unknown_sequences_never_return_empty_success() {
    let fixture = Fixture::new();
    for (after, digest) in [(3, 4), (4, 3), (0, 0)] {
        assert_error(
            fixture
                .request(
                    "GET",
                    &commits_uri(after, digest, 10),
                    Some(TOKEN),
                    String::new(),
                )
                .await,
            StatusCode::CONFLICT,
            "commit_anchor_or_log_mismatch",
        )
        .await;
    }
    let response = fixture
        .request("GET", &commits_uri(3, 3, 10), Some(TOKEN), String::new())
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 4096)
        .await
        .unwrap();
    let page: CommitPageDto = serde_json::from_slice(&bytes).unwrap();
    assert!(page.entries.is_empty());
    assert_eq!(page.anchor_sequence, CommitSeq::from_u64(3));
    assert_eq!(page.anchor_digest, [3; 32]);
}

#[tokio::test]
async fn corrupt_published_commit_is_conflict() {
    let fixture = Fixture::new();
    fixture
        .ledger
        .commits
        .lock()
        .unwrap()
        .push(PublishedCommit {
            sequence: CommitSeq::from_u64(4),
            previous_digest: [3; 32],
            digest: [4; 32],
            canonical_json: "{}".to_owned(),
        });
    assert_error(
        fixture
            .request("GET", &commits_uri(3, 3, 10), Some(TOKEN), String::new())
            .await,
        StatusCode::CONFLICT,
        "commit_anchor_or_log_mismatch",
    )
    .await;
}

#[tokio::test]
async fn mutation_rejects_foreign_origin_binding_version_and_deep_json_before_write() {
    let fixture = Fixture::new();
    let mut mutation = fixture.mutation.clone();
    mutation.origin_device_id = DeviceId::new();
    assert_error(
        fixture.submit(&mutation).await,
        StatusCode::FORBIDDEN,
        "origin_device_mismatch",
    )
    .await;
    mutation = fixture.mutation.clone();
    mutation.binding.library_id = LibraryId::new();
    assert_error(
        fixture.submit(&mutation).await,
        StatusCode::CONFLICT,
        "library_binding_mismatch",
    )
    .await;
    mutation = fixture.mutation.clone();
    mutation.protocol_version = PROTOCOL_VERSION + 1;
    assert_error(
        fixture.submit(&mutation).await,
        StatusCode::BAD_REQUEST,
        "invalid_mutation",
    )
    .await;
    mutation = fixture.mutation.clone();
    for _ in 0..65 {
        mutation.payload = serde_json::json!([mutation.payload]);
    }
    assert_error(
        fixture.submit(&mutation).await,
        StatusCode::BAD_REQUEST,
        "invalid_mutation",
    )
    .await;
    assert!(fixture.ledger.staged.lock().unwrap().is_none());
}

#[tokio::test]
async fn publication_failure_can_retry_same_mutation_and_only_ack_after_publication() {
    let fixture = Fixture::new();
    fixture.checkpoint.fail_once.store(true, Ordering::Relaxed);
    assert_error(
        fixture.submit(&fixture.mutation).await,
        StatusCode::CONFLICT,
        "mutation_rejected",
    )
    .await;
    assert_eq!(fixture.ledger.published.load(Ordering::Relaxed), 0);
    let staged = fixture
        .ledger
        .staged
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .1
        .clone();
    for _ in 0..2 {
        let response = fixture.submit(&fixture.mutation).await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), 4096)
            .await
            .unwrap();
        let ack: MutationAck = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(ack.confirmation, staged);
    }
    assert_eq!(fixture.ledger.published.load(Ordering::Relaxed), 2);
}

#[tokio::test]
async fn pagination_body_size_and_rate_limits_are_enforced() {
    let fixture = Fixture::new();
    for limit in [0, MAX_PAGE_SIZE + 1] {
        assert_error(
            fixture
                .request("GET", &commits_uri(3, 3, limit), Some(TOKEN), String::new())
                .await,
            StatusCode::BAD_REQUEST,
            "invalid_page_size",
        )
        .await;
    }
    let invalid_digest = "/v1/commits?after=3&previous_digest=xyz&limit=10";
    assert_error(
        fixture
            .request("GET", invalid_digest, Some(TOKEN), String::new())
            .await,
        StatusCode::BAD_REQUEST,
        "invalid_anchor_digest",
    )
    .await;
    let response = fixture
        .request(
            "POST",
            "/v1/mutations",
            Some(TOKEN),
            " ".repeat(MAX_MUTATION_BYTES + 1),
        )
        .await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    let response = fixture
        .request("POST", "/v1/mutations", Some(TOKEN), "{invalid".to_owned())
        .await;
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    assert_eq!(fixture.ledger.reads.load(Ordering::Relaxed), 0);
    for _ in 3..120 {
        assert_eq!(
            fixture
                .request("GET", &commits_uri(3, 3, 10), Some(TOKEN), String::new())
                .await
                .status(),
            StatusCode::OK
        );
    }
    assert_error(
        fixture
            .request("GET", &commits_uri(3, 3, 10), Some(TOKEN), String::new())
            .await,
        StatusCode::TOO_MANY_REQUESTS,
        "rate_limited",
    )
    .await;
}

struct Resources {
    calls: AtomicUsize,
}

impl ResourceTransferPort for Resources {
    fn prepare(
        &self,
        _owner: DeviceId,
        _request: &PrepareResource,
    ) -> Result<crate::resource_transfer::TransferProgress, TransferError> {
        Err(TransferError::Unknown)
    }

    fn write_chunk(
        &self,
        _binding: SyncBinding,
        _owner: DeviceId,
        _upload: &str,
        _index: u32,
        _hash: &str,
        _bytes: &[u8],
    ) -> Result<crate::resource_transfer::TransferProgress, TransferError> {
        Err(TransferError::Unknown)
    }

    fn complete(
        &self,
        _binding: SyncBinding,
        _owner: DeviceId,
        _upload: &str,
    ) -> Result<crate::resource_transfer::TransferProgress, TransferError> {
        Err(TransferError::Unknown)
    }

    fn download(
        &self,
        _binding: SyncBinding,
        hash: &str,
        offset: u64,
        length: usize,
    ) -> Result<crate::resource_transfer::DownloadChunk, TransferError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if hash != blake3::hash(b"audio").to_hex().as_str() || offset != 1 || length != 3 {
            return Err(TransferError::Invalid);
        }
        Ok(crate::resource_transfer::DownloadChunk {
            bytes: b"udi".to_vec(),
            total_length: 5,
            content_hash: hash.to_owned(),
            chunk_hash: blake3::hash(b"udi").to_hex().to_string(),
        })
    }
}

#[tokio::test]
async fn resource_range_requires_matching_library_and_bounded_explicit_offsets() {
    let mut fixture = Fixture::new();
    let resources = Arc::new(Resources {
        calls: AtomicUsize::new(0),
    });
    fixture.state = fixture.state.with_resources(resources.clone());
    let hash = blake3::hash(b"audio").to_hex().to_string();
    let uri = format!("/v1/resources/{hash}");
    let binding = serde_json::to_string(&fixture.state.binding).unwrap();
    assert_error(
        fixture
            .request("GET", &uri, Some(TOKEN), String::new())
            .await,
        StatusCode::CONFLICT,
        "library_binding_mismatch",
    )
    .await;
    for range in [
        None,
        Some("bytes=1-"),
        Some("bytes=-3"),
        Some("bytes=2-1"),
        Some("bytes=0-4194304"),
        Some("bytes=0-1,3-4"),
    ] {
        let mut headers = vec![("x-echo-binding", binding.as_str())];
        if let Some(range) = range {
            headers.push(("range", range));
        }
        let response = fixture
            .request_headers("GET", &uri, Some(TOKEN), String::new(), &headers)
            .await;
        assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    }
    assert_error(
        fixture
            .request_headers(
                "GET",
                &uri,
                Some(TOKEN),
                String::new(),
                &[
                    ("x-echo-binding", &binding),
                    ("range", "bytes=1-3"),
                    ("if-match", "\"wrong\""),
                ],
            )
            .await,
        StatusCode::PRECONDITION_FAILED,
        "resource_version_mismatch",
    )
    .await;
    assert_eq!(resources.calls.load(Ordering::Relaxed), 0);
    let response = fixture
        .request_headers(
            "GET",
            &uri,
            Some(TOKEN),
            String::new(),
            &[("x-echo-binding", &binding), ("range", "bytes=1-3")],
        )
        .await;
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(response.headers()["content-range"], "bytes 1-3/5");
    assert_eq!(response.headers()["etag"], format!("\"{hash}\""));
    assert_eq!(
        response.headers()["x-echo-chunk-hash"],
        blake3::hash(b"udi").to_hex().to_string()
    );
    assert_eq!(
        axum::body::to_bytes(response.into_body(), 10)
            .await
            .unwrap()
            .as_ref(),
        b"udi"
    );
    assert_eq!(resources.calls.load(Ordering::Relaxed), 1);
}

#[path = "server_security_tests.rs"]
mod security;
