//! Requests held across authorization changes must not disclose their result.

use super::*;

struct SelectiveAuthorization(Mutex<Vec<DeviceId>>);

impl HostPeerAuthorizationPort for SelectiveAuthorization {
    fn peer_is_authorized(&self, _binding: SyncBinding, device: DeviceId) -> Result<bool, Error> {
        Ok(!self.0.lock().unwrap().contains(&device))
    }
}

#[tokio::test]
async fn revocation_ends_held_page_before_disclosure_and_preserves_other_peers() {
    let mut fixture = Fixture::new();
    let authorization = Arc::new(SelectiveAuthorization(Mutex::new(Vec::new())));
    fixture.state = fixture.state.with_authorization(authorization.clone());
    let other = DeviceId::new();
    let other_token = "other-device-token-0123456789abcdef";
    fixture
        .state
        .peer_credentials
        .lock()
        .unwrap()
        .push((other, *blake3::hash(other_token.as_bytes()).as_bytes()));
    let fixture = Arc::new(fixture);
    let held_fixture = Arc::clone(&fixture);
    let held = tokio::spawn(async move {
        held_fixture
            .request(
                "GET",
                &format!("{}&wait_ms=8000", commits_uri(3, 3, 10)),
                Some(TOKEN),
                String::new(),
            )
            .await
    });
    // Wait for the first successful authorized ledger read before revoking.
    tokio::time::timeout(Duration::from_secs(2), async {
        while fixture.ledger.reads.load(Ordering::Relaxed) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    authorization.0.lock().unwrap().push(fixture.device);
    fixture
        .ledger
        .commits
        .lock()
        .unwrap()
        .push(published_commit(&fixture.mutation));
    let response = tokio::time::timeout(Duration::from_secs(2), held)
        .await
        .unwrap()
        .unwrap();
    assert_error(response, StatusCode::UNAUTHORIZED, "unauthorized").await;
    assert_eq!(fixture.ledger.reads.load(Ordering::Relaxed), 1);
    let response = fixture
        .request(
            "GET",
            &commits_uri(3, 3, 10),
            Some(other_token),
            String::new(),
        )
        .await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(commit_page_body(response).await.entries.len(), 1);
}

struct RevokingResources {
    authorization: Arc<Authorization>,
}

impl ResourceTransferPort for RevokingResources {
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
        _offset: u64,
        _length: usize,
    ) -> Result<crate::resource_transfer::DownloadChunk, TransferError> {
        // The revocation commits while the bounded disk read is in flight.
        self.authorization.0.store(false, Ordering::Relaxed);
        Ok(crate::resource_transfer::DownloadChunk {
            bytes: b"private-audio".to_vec(),
            total_length: 13,
            content_hash: hash.into(),
            chunk_hash: blake3::hash(b"private-audio").to_hex().to_string(),
        })
    }
}

#[tokio::test]
async fn revocation_during_resource_read_discards_the_in_flight_body() {
    let mut fixture = Fixture::new();
    fixture.state = fixture.state.with_resources(Arc::new(RevokingResources {
        authorization: Arc::clone(&fixture.authorization),
    }));
    let binding = serde_json::to_string(&fixture.state.binding).unwrap();
    let response = fixture
        .request_headers(
            "GET",
            &format!("/v1/resources/{}", "a".repeat(64)),
            Some(TOKEN),
            String::new(),
            &[("x-echo-binding", &binding), ("range", "bytes=0-12")],
        )
        .await;
    assert_error(response, StatusCode::UNAUTHORIZED, "unauthorized").await;
}
