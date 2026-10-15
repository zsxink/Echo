use super::*;
use echo_core::application::ports::StagedConfirmation;
use echo_core::domain::ids::SongId;

use echo_core::domain::sync::{
    AuthorityId, CommitSeq, Epoch, LibraryId, MutationId, ObjectKey, SyncBinding,
};
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::Arc;
use std::thread::JoinHandle;

fn decode_hex(value: &str) -> Vec<u8> {
    value
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
        .collect()
}

fn mutation() -> MutationDto {
    MutationDto {
        resource_refs: Vec::new(),
        protocol_version: crate::protocol::PROTOCOL_VERSION,
        binding: SyncBinding {
            library_id: LibraryId::new(),
            authority_id: AuthorityId::new(),
            epoch: Epoch::INITIAL,
        },
        origin_device_id: DeviceId::new(),
        mutation_id: MutationId::new(),
        origin_seq: 1,
        previous_mutation_id: None,
        object: ObjectKey::Song(SongId::new()),
        base_object_version: None,
        payload: serde_json::json!({"title": "song"}),
    }
}

fn accepted_ack(mutation: &MutationDto) -> MutationAck {
    MutationAck {
        confirmation: StagedConfirmation {
            mutation_id: mutation.mutation_id,
            outcome_json: "{\"accepted\":true}".to_owned(),
            checkpoint: Some(CommitCheckpoint {
                binding: mutation.binding,
                sequence: CommitSeq::from_u64(1),
                digest: [1; 32],
            }),
        },
    }
}

#[derive(Debug)]
struct CertificateResolver(Arc<CertifiedKey>);
impl ResolvesServerCert for CertificateResolver {
    fn resolve(&self, _client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(Arc::clone(&self.0))
    }
}

/// A real TLS/HTTP endpoint; returns whether an authenticated HTTP request arrived.
fn host(
    certificate_hex: &str,
    key_hex: &str,
    tls12: bool,
    body: Vec<u8>,
) -> (Url, JoinHandle<bool>) {
    let certificate = CertificateDer::from(decode_hex(certificate_hex));
    let key = PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(decode_hex(key_hex)));
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let signing_key = provider.key_provider.load_private_key(key).unwrap();
    // This resolver intentionally allows mismatched keys to exercise the client's
    // actual CertificateVerify validation, not just its certificate comparison.
    let certified_key = Arc::new(CertifiedKey::new(vec![certificate], signing_key));
    let versions = if tls12 {
        &[&rustls::version::TLS12][..]
    } else {
        &[&rustls::version::TLS13][..]
    };
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(versions)
        .unwrap()
        .with_no_client_auth()
        .with_cert_resolver(Arc::new(CertificateResolver(certified_key)));
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let endpoint = Url::parse(&format!("https://{}/", listener.local_addr().unwrap())).unwrap();
    let worker = std::thread::spawn(move || {
        let (socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let connection = rustls::ServerConnection::new(Arc::new(config)).unwrap();
        let mut stream = rustls::StreamOwned::new(connection, socket);
        let mut request = Vec::new();
        let mut chunk = [0; 4096];
        loop {
            let Ok(length) = stream.read(&mut chunk) else {
                return false;
            };
            if length == 0 {
                return false;
            }
            request.extend_from_slice(&chunk[..length]);
            if let Some(end) = request.windows(4).position(|bytes| bytes == b"\r\n\r\n") {
                let headers = std::str::from_utf8(&request[..end]).unwrap();
                let content_length = headers
                    .lines()
                    .find_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        key.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                if request.len() >= end + 4 + content_length {
                    break;
                }
            }
        }
        assert!(std::str::from_utf8(&request)
            .unwrap()
            .contains("Bearer paired-test-token"));
        assert_eq!(
            stream.conn.protocol_version(),
            Some(rustls::ProtocolVersion::TLSv1_3)
        );
        let headers = format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
        let _ = stream.write_all(headers.as_bytes());
        if body.len() <= MAX_SYNC_PAGE_BYTES {
            let _ = stream.write_all(&body);
        }
        let _ = stream.flush();
        true
    });
    (endpoint, worker)
}

#[tokio::test]
async fn paired_key_allows_tls13_and_certificate_renewal_without_matching_san() {
    let mutation = mutation();
    let ack = accepted_ack(&mutation);
    let client = LanClient::new(&decode_hex(CERTIFICATE), "paired-test-token".to_owned()).unwrap();
    // The paired key authenticates an IP address discovered over LAN, even when
    // a renewed certificate has a different CN and no SAN for that address.
    for certificate in [CERTIFICATE, RENEWED_CERTIFICATE] {
        let (endpoint, worker) = host(
            certificate,
            PRIVATE_KEY,
            false,
            serde_json::to_vec(&ack).unwrap(),
        );
        assert_eq!(
            client.submit_mutation(&endpoint, &mutation).await.unwrap(),
            ack
        );
        assert!(worker.join().unwrap());
    }
}

#[tokio::test]
async fn refuses_wrong_host_key_tls12_and_forged_handshake_signature() {
    let mutation = mutation();
    let client = LanClient::new(&decode_hex(CERTIFICATE), "paired-test-token".to_owned()).unwrap();
    for (certificate, key, tls12) in [
        (WRONG_CERTIFICATE, WRONG_PRIVATE_KEY, false),
        (CERTIFICATE, PRIVATE_KEY, true),
        (CERTIFICATE, WRONG_PRIVATE_KEY, false),
    ] {
        let (endpoint, worker) = host(certificate, key, tls12, Vec::new());
        assert!(matches!(
            client.submit_mutation(&endpoint, &mutation).await,
            Err(ClientError::Transport(_))
        ));
        assert!(
            !worker.join().unwrap(),
            "untrusted peer must never receive the bearer token"
        );
    }
}

#[tokio::test]
async fn rejects_ack_for_a_different_mutation_on_the_wire() {
    let mutation = mutation();
    let mut ack = accepted_ack(&mutation);
    ack.confirmation.mutation_id = MutationId::new();
    let client = LanClient::new(&decode_hex(CERTIFICATE), "paired-test-token".to_owned()).unwrap();
    let (endpoint, worker) = host(
        CERTIFICATE,
        PRIVATE_KEY,
        false,
        serde_json::to_vec(&ack).unwrap(),
    );
    assert!(matches!(
        client.submit_mutation(&endpoint, &mutation).await,
        Err(ClientError::InvalidResponseBody)
    ));
    assert!(worker.join().unwrap());
}

#[tokio::test]
async fn rejects_oversized_mutation_ack_before_decoding() {
    let mutation = mutation();
    let client = LanClient::new(&decode_hex(CERTIFICATE), "paired-test-token".to_owned()).unwrap();
    let (endpoint, worker) = host(
        CERTIFICATE,
        PRIVATE_KEY,
        false,
        vec![b' '; MAX_SYNC_PAGE_BYTES + 1],
    );
    assert!(matches!(
        client.submit_mutation(&endpoint, &mutation).await,
        Err(ClientError::InvalidResponseBody)
    ));
    assert!(worker.join().unwrap());
}

#[test]
fn validates_ack_identity_binding_checkpoint_and_outcome() {
    let mutation = mutation();
    let ack = accepted_ack(&mutation);
    assert!(validate_mutation_ack(&ack, &mutation).is_ok());
    let mut invalid = ack.clone();
    invalid.confirmation.mutation_id = MutationId::new();
    assert!(validate_mutation_ack(&invalid, &mutation).is_err());
    invalid = ack.clone();
    invalid
        .confirmation
        .checkpoint
        .as_mut()
        .unwrap()
        .binding
        .authority_id = AuthorityId::new();
    assert!(validate_mutation_ack(&invalid, &mutation).is_err());
    invalid = ack.clone();
    invalid.confirmation.checkpoint.as_mut().unwrap().sequence = CommitSeq::GENESIS;
    assert!(validate_mutation_ack(&invalid, &mutation).is_err());
    for outcome in [
        "{}",
        "null",
        "{\"accepted\":\"true\"}",
        "{\"accepted\":false,\"reason\":\"denied\"}",
        "{\"accepted\":true,\"reason\":\"denied\"}",
    ] {
        invalid = ack.clone();
        invalid.confirmation.outcome_json = outcome.to_owned();
        assert!(validate_mutation_ack(&invalid, &mutation).is_err());
    }
    invalid = ack;
    invalid.confirmation.checkpoint = None;
    assert!(validate_mutation_ack(&invalid, &mutation).is_err());
    invalid.confirmation.outcome_json =
        "{\"accepted\":false,\"reason\":\"object_already_deleted\"}".to_owned();
    assert!(validate_mutation_ack(&invalid, &mutation).is_ok());
    invalid.confirmation.outcome_json = "{\"accepted\":false}".to_owned();
    assert!(validate_mutation_ack(&invalid, &mutation).is_err());
    invalid.confirmation.outcome_json = "{\"accepted\":false,\"reason\":\"\"}".to_owned();
    assert!(validate_mutation_ack(&invalid, &mutation).is_err());
}

#[test]
fn rejects_malformed_provisioned_certificate() {
    assert!(matches!(
        LanClient::new(b"invalid DER", "unused".to_owned()),
        Err(ClientError::InvalidTlsConfiguration(_))
    ));
}

// Static test-only ECDSA certificates and keys; never used outside loopback tests.
const CERTIFICATE: &str = "3082016230820108a003020102020900986fc10ebda96d5a300a06082a8648ce3d040302301b3119301706035504030c106563686f2d7061697265642d686f7374301e170d3236313031343031303434395a170d3336313031313031303434395a301b3119301706035504030c106563686f2d7061697265642d686f73743059301306072a8648ce3d020106082a8648ce3d03010703420004601722e3634d1a51cb2f023d459cc70991f508c1eff1289446d483df5470d9d73a3f890a585c71114247d73bb9aeb83e793f6fabccbf981c0839acb51ba1bda0a3353033300c0603551d130101ff04023000300e0603551d0f0101ff04040302078030130603551d25040c300a06082b06010505070301300a06082a8648ce3d040302034800304502204272dc76b07a15f2a35d13b82e205a887864637b3b18afe540978e851da99cdc022100c51f938f68965cc66b6bdc3c5d2e783e0b93c1e2ccbddc32e3b873d99bb474dd";
const PRIVATE_KEY: &str = "308187020100301306072a8648ce3d020106082a8648ce3d030107046d306b020101042062853497712675dd6632a7085ee432558b34c5160577b1fd58e9308118489157a14403420004601722e3634d1a51cb2f023d459cc70991f508c1eff1289446d483df5470d9d73a3f890a585c71114247d73bb9aeb83e793f6fabccbf981c0839acb51ba1bda0";
const WRONG_CERTIFICATE: &str = "3082016130820108a00302010202090095a24f4f8aedadfd300a06082a8648ce3d040302301b3119301706035504030c106563686f2d7061697265642d686f7374301e170d3236313031343031303434395a170d3336313031313031303434395a301b3119301706035504030c106563686f2d7061697265642d686f73743059301306072a8648ce3d020106082a8648ce3d0301070342000452213485b997329c023f001754f9827d234716643b54fa3aa034cd9445e1d91b5984dea63b342b5923ea40beeb7e538cd6bfdf09ad80cb75a18b23f14c1db548a3353033300c0603551d130101ff04023000300e0603551d0f0101ff04040302078030130603551d25040c300a06082b06010505070301300a06082a8648ce3d0403020347003044022036f88c35793b160d93e797f60ff85fa1699a77cef31464fc2eb3aea3cf04b4be022000f7953b031917ba135a8c133319f14572e8e4eef382be703278cc342e4112ca";
const WRONG_PRIVATE_KEY: &str = "308187020100301306072a8648ce3d020106082a8648ce3d030107046d306b0201010420fca9c1b401ba3e4446d61b47ce9c169fea962ac069d8b648bb69abfc102d580ca1440342000452213485b997329c023f001754f9827d234716643b54fa3aa034cd9445e1d91b5984dea63b342b5923ea40beeb7e538cd6bfdf09ad80cb75a18b23f14c1db548";
const RENEWED_CERTIFICATE: &str = "308201653082010aa003020102020900f012247227fd8315300a06082a8648ce3d040302301c311a301806035504030c116563686f2d686f73742d72656e65776564301e170d3236313031343031303434395a170d3336313031313031303434395a301c311a301806035504030c116563686f2d686f73742d72656e657765643059301306072a8648ce3d020106082a8648ce3d03010703420004601722e3634d1a51cb2f023d459cc70991f508c1eff1289446d483df5470d9d73a3f890a585c71114247d73bb9aeb83e793f6fabccbf981c0839acb51ba1bda0a3353033300c0603551d130101ff04023000300e0603551d0f0101ff04040302078030130603551d25040c300a06082b06010505070301300a06082a8648ce3d0403020349003046022100a7c81f65767aac8de3e42ba3f8f4346b8e0d23211ae1337ddf7228f4cb28ac3e022100fffc6a6c174e848277f2d29913d168c07e9711da86756fcd6500d0989c9874b2";

#[path = "client_http_tests.rs"]
mod http_contracts;
