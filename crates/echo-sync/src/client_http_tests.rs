//! Real TLS contracts for redirects and resumable upload acknowledgements.

use super::{decode_hex, mutation, CertificateResolver, CERTIFICATE, PRIVATE_KEY};
use crate::client::{ClientError, LanClient};
use crate::protocol::{
    CommitPageDto, ResourceRefDto, MAX_COMMIT_WAIT_MS, MAX_RESOURCE_CHUNK_BYTES,
};
use crate::resource_transfer::{PrepareResource, TransferProgress};
use echo_core::domain::sync::SharedResourceKind;
use reqwest::{StatusCode, Url};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::sign::CertifiedKey;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

struct Exchange {
    method_path: String,
    binding: Option<String>,
    body: Vec<u8>,
    response: Option<Vec<u8>>,
}

fn json_response(value: &impl serde::Serialize) -> Vec<u8> {
    let body = serde_json::to_vec(value).unwrap();
    let mut response = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
    response.extend(body);
    response
}

fn read_request(
    stream: &mut rustls::StreamOwned<rustls::ServerConnection, std::net::TcpStream>,
) -> Vec<u8> {
    let mut request = Vec::new();
    let mut chunk = [0; 8192];
    loop {
        let length = stream.read(&mut chunk).unwrap();
        assert_ne!(length, 0, "client closed before sending a full request");
        request.extend_from_slice(&chunk[..length]);
        if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
            let headers = std::str::from_utf8(&request[..end]).unwrap();
            let length = header(headers, "content-length")
                .map_or(0, |value| value.parse::<usize>().unwrap());
            if request.len() == end + 4 + length {
                return request;
            }
        }
        assert!(request.len() < MAX_RESOURCE_CHUNK_BYTES + 8192);
    }
}

fn header<'a>(headers: &'a str, name: &str) -> Option<&'a str> {
    headers.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.eq_ignore_ascii_case(name).then(|| value.trim())
    })
}

/// Each exchange uses a new TLS connection, emulating reconnection after loss.
fn scripted_host(exchanges: Vec<Exchange>) -> (Url, JoinHandle<()>) {
    let certificate = CertificateDer::from(decode_hex(CERTIFICATE));
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(decode_hex(PRIVATE_KEY)));
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let signing_key = provider.key_provider.load_private_key(key).unwrap();
    let certified_key = Arc::new(CertifiedKey::new(vec![certificate], signing_key));
    let config = Arc::new(
        rustls::ServerConfig::builder_with_provider(provider)
            .with_protocol_versions(&[&rustls::version::TLS13])
            .unwrap()
            .with_no_client_auth()
            .with_cert_resolver(Arc::new(CertificateResolver(certified_key))),
    );
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = Url::parse(&format!("https://{}/", listener.local_addr().unwrap())).unwrap();
    let worker = std::thread::spawn(move || {
        for exchange in exchanges {
            let (socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            socket
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let connection = rustls::ServerConnection::new(config.clone()).unwrap();
            let mut stream = rustls::StreamOwned::new(connection, socket);
            let request = read_request(&mut stream);
            let end = request
                .windows(4)
                .position(|bytes| bytes == b"\r\n\r\n")
                .unwrap();
            let headers = std::str::from_utf8(&request[..end]).unwrap();
            assert_eq!(
                headers.lines().next().unwrap(),
                format!("{} HTTP/1.1", exchange.method_path)
            );
            assert_eq!(
                header(headers, "authorization"),
                Some("Bearer paired-test-token")
            );
            if let Some(binding) = exchange.binding {
                assert_eq!(header(headers, "x-echo-binding"), Some(binding.as_str()));
            }
            assert_eq!(&request[end + 4..], exchange.body);
            if exchange.method_path.starts_with("PUT ") {
                let expected_hash = blake3::hash(&exchange.body).to_hex().to_string();
                assert_eq!(
                    header(headers, "x-echo-chunk-hash"),
                    Some(expected_hash.as_str())
                );
            }
            if let Some(response) = exchange.response {
                stream.write_all(&response).unwrap();
                stream.flush().unwrap();
            }
        }
    });
    (endpoint, worker)
}

#[tokio::test]
async fn redirect_response_is_rejected_without_contacting_destination() {
    let destination = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let location = format!("https://{}/stolen", destination.local_addr().unwrap());
    let candidate = mutation();
    let response = format!("HTTP/1.1 307 Temporary Redirect\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").into_bytes();
    let (endpoint, worker) = scripted_host(vec![Exchange {
        method_path: "POST /v1/mutations".to_owned(),
        binding: None,
        body: serde_json::to_vec(&candidate).unwrap(),
        response: Some(response),
    }]);
    let client = LanClient::new(&decode_hex(CERTIFICATE), "paired-test-token".to_owned()).unwrap();
    assert!(matches!(
        client.submit_mutation(&endpoint, &candidate).await,
        Err(ClientError::HostRejected(StatusCode::TEMPORARY_REDIRECT))
    ));
    worker.join().unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(200), destination.accept())
            .await
            .is_err(),
        "redirect destination must receive neither a connection nor credentials"
    );
}

#[tokio::test]
async fn held_pull_carries_the_wait_parameter_and_rejects_a_wait_above_the_bound() {
    use echo_core::domain::sync::CommitSeq;

    let client = LanClient::new(&decode_hex(CERTIFICATE), "paired-test-token".to_owned()).unwrap();
    // Above the protocol bound the client must refuse before any traffic.
    assert!(matches!(
        client
            .pull_commits(
                &Url::parse("https://127.0.0.1:1/").unwrap(),
                3,
                [3; 32],
                1,
                Duration::from_millis(u64::from(MAX_COMMIT_WAIT_MS) + 1),
            )
            .await,
        Err(ClientError::InvalidCommitChain)
    ));
    // A wait within the bound travels as one extra query parameter, and the
    // page validation still runs on the held response.
    let page = CommitPageDto {
        binding: mutation().binding,
        anchor_sequence: CommitSeq::from_u64(3),
        anchor_digest: [3; 32],
        entries: Vec::new(),
    };
    let (endpoint, worker) = scripted_host(vec![Exchange {
        method_path: format!(
            "GET /v1/commits?after=3&previous_digest={}&limit=1&wait_ms=1500",
            "03".repeat(32)
        ),
        binding: None,
        body: Vec::new(),
        response: Some(json_response(&page)),
    }]);
    let pulled = client
        .pull_commits(&endpoint, 3, [3; 32], 1, Duration::from_millis(1500))
        .await
        .unwrap();
    assert!(pulled.entries.is_empty());
    assert_eq!(pulled.anchor_sequence, CommitSeq::from_u64(3));
    worker.join().unwrap();
}

#[tokio::test]
async fn immediate_pull_omits_the_wait_parameter() {
    use echo_core::domain::sync::CommitSeq;

    let page = CommitPageDto {
        binding: mutation().binding,
        anchor_sequence: CommitSeq::from_u64(3),
        anchor_digest: [3; 32],
        entries: Vec::new(),
    };
    let (endpoint, worker) = scripted_host(vec![Exchange {
        method_path: format!(
            "GET /v1/commits?after=3&previous_digest={}&limit=1",
            "03".repeat(32)
        ),
        binding: None,
        body: Vec::new(),
        response: Some(json_response(&page)),
    }]);
    let client = LanClient::new(&decode_hex(CERTIFICATE), "paired-test-token".to_owned()).unwrap();
    let pulled = client
        .pull_commits(&endpoint, 3, [3; 32], 1, Duration::ZERO)
        .await
        .unwrap();
    assert!(pulled.entries.is_empty());
    worker.join().unwrap();
}

fn upload_exchanges(request: &PrepareResource, first: &[u8], last: &[u8]) -> Vec<Exchange> {
    let initial = TransferProgress {
        upload_id: "1a".repeat(32),
        resource: request.resource.clone(),
        chunk_size: MAX_RESOURCE_CHUNK_BYTES,
        confirmed_chunks: Vec::new(),
        complete: false,
    };
    let resumed = TransferProgress {
        confirmed_chunks: vec![0],
        ..initial.clone()
    };
    let received = TransferProgress {
        confirmed_chunks: vec![0, 1],
        ..initial.clone()
    };
    let complete = TransferProgress {
        complete: true,
        ..received.clone()
    };
    let binding = Some(serde_json::to_string(&request.binding).unwrap());
    let prepare = serde_json::to_vec(request).unwrap();
    let chunk_path = format!("PUT /v1/resources/{}/chunks/", initial.upload_id);
    vec![
        Exchange {
            method_path: "POST /v1/resources/prepare".to_owned(),
            binding: binding.clone(),
            body: prepare.clone(),
            response: Some(json_response(&initial)),
        },
        // The host persisted chunk zero, but its ACK is lost before reaching the client.
        Exchange {
            method_path: format!("{chunk_path}0"),
            binding: binding.clone(),
            body: first.to_vec(),
            response: None,
        },
        Exchange {
            method_path: "POST /v1/resources/prepare".to_owned(),
            binding: binding.clone(),
            body: prepare.clone(),
            response: Some(json_response(&resumed)),
        },
        Exchange {
            method_path: format!("{chunk_path}1"),
            binding: binding.clone(),
            body: last.to_vec(),
            response: Some(json_response(&received)),
        },
        Exchange {
            method_path: format!("POST /v1/resources/{}/complete", initial.upload_id),
            binding: binding.clone(),
            body: Vec::new(),
            response: Some(json_response(&complete)),
        },
        // Repeating an already complete upload must not send chunks again.
        Exchange {
            method_path: "POST /v1/resources/prepare".to_owned(),
            binding,
            body: prepare,
            response: Some(json_response(&complete)),
        },
    ]
}

#[tokio::test]
async fn upload_resumes_after_lost_chunk_ack_and_confirms_completion_over_tls() {
    let first = vec![0x55; MAX_RESOURCE_CHUNK_BYTES];
    let last = b"last chunk";
    let mut bytes = first.clone();
    bytes.extend(last);
    let request = PrepareResource {
        binding: mutation().binding,
        resource: ResourceRefDto {
            kind: SharedResourceKind::Audio,
            content_hash: blake3::hash(&bytes).to_hex().to_string(),
            length: bytes.len() as u64,
            media_type: "audio/flac".to_owned(),
            relative_path: "media/resumable.flac".to_owned(),
        },
    };
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("source.flac");
    std::fs::write(&path, bytes).unwrap();
    let (endpoint, worker) = scripted_host(upload_exchanges(&request, &first, last));
    let client = LanClient::new(&decode_hex(CERTIFICATE), "paired-test-token".to_owned()).unwrap();
    let prepared = client.prepare_resource(&endpoint, &request).await.unwrap();
    assert!(matches!(
        client
            .upload_resource_chunk(&endpoint, request.binding, &prepared, 0, first)
            .await,
        Err(ClientError::Transport(_))
    ));
    let reconnected =
        LanClient::new(&decode_hex(CERTIFICATE), "paired-test-token".to_owned()).unwrap();
    let complete = reconnected
        .upload_resource_file(&endpoint, &request, path.clone())
        .await
        .unwrap();
    assert!(complete.complete);
    assert_eq!(complete.confirmed_chunks, vec![0, 1]);
    assert_eq!(complete.resource, request.resource);
    assert_eq!(
        reconnected
            .upload_resource_file(&endpoint, &request, path)
            .await
            .unwrap(),
        complete
    );
    worker.join().unwrap();
}
