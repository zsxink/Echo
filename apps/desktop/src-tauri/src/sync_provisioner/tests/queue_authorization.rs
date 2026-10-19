//! Typed deletion and re-anchored queue payloads: authorization by queue
//! entry and the change-time backing through the scoped wrapper.

#[allow(clippy::wildcard_imports)]
use super::tests::*;

#[derive(serde::Deserialize)]
struct ImmediateCommitQuery {
    after: u64,
    previous_digest: String,
}

/// A host from before the bounded long-poll: it answers an empty page at once
/// and ignores `wait_ms` entirely.
async fn immediate_commit_page(
    axum::extract::Query(query): axum::extract::Query<ImmediateCommitQuery>,
) -> axum::Json<CommitPageDto> {
    let mut digest = [0_u8; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&query.previous_digest[index * 2..index * 2 + 2], 16).unwrap();
    }
    axum::Json(CommitPageDto {
        binding: SyncBinding {
            library_id: LibraryId::new(),
            authority_id: AuthorityId::new(),
            epoch: Epoch::INITIAL,
        },
        anchor_sequence: CommitSeq::from_u64(query.after),
        anchor_digest: digest,
        entries: Vec::new(),
    })
}

/// Serve an immediate-answer host over TLS and return its endpoint, pinned
/// certificate, and server handle.
async fn spawn_immediate_commit_host(
) -> (Url, Vec<u8>, tokio::task::JoinHandle<std::io::Result<()>>) {
    let certificate = rcgen::generate_simple_self_signed(vec!["localhost".into()]).unwrap();
    let certificate_der = certificate.cert.der().to_vec();
    let tls = tls13_server_config(
        vec![CertificateDer::from(certificate_der.clone())],
        PrivatePkcs8KeyDer::from(certificate.key_pair.serialize_der()).into(),
    )
    .unwrap();
    let router =
        axum::Router::new().route("/v1/commits", axum::routing::get(immediate_commit_page));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = Url::parse(&format!("https://{}/", listener.local_addr().unwrap())).unwrap();
    let server = tokio::spawn(echo_sync::server::serve_tls(listener, tls, router));
    (endpoint, certificate_der, server)
}

#[tokio::test]
async fn a_host_that_answers_before_the_budget_is_reported_as_unsupported() {
    let fixture = LanMergeFixture::new().await;
    let (endpoint, certificate_der, server) = spawn_immediate_commit_host().await;
    let peer = ProvisionedPeer {
        binding: fixture.peer.binding,
        address: endpoint.to_string(),
        client: LanClient::new(
            &certificate_der,
            "temporary-loopback-test-token-123456789".into(),
        )
        .unwrap(),
        host_device_id: DeviceId::new(),
    };

    let cancelled = Arc::new(AtomicBool::new(false));
    let outcome = tokio::time::timeout(
        Duration::from_secs(5),
        fixture.provisioner.watch_peer_commits(
            &peer,
            fixture.root,
            cancelled,
            Duration::from_secs(30),
        ),
    )
    .await;
    server.abort();
    let outcome = outcome.expect("an immediate host must end the watch at once");
    assert_eq!(
        outcome.unwrap(),
        WatchOutcome::Unsupported,
        "an early empty page means the host cannot hold; the caller must poll instead"
    );
}

/// A typed deletion removes its object row before it is pushed, so the root
/// guard can no longer resolve it from live state. Rejecting it there would
/// abort the entire push loop on the first delete — the host returns
/// `LibraryUnavailable` and the replica turns it into a scoped-queue conflict —
/// leaving every later mutation permanently stuck behind it.
#[test]
fn typed_deletion_is_authorized_by_its_queue_entry_not_by_a_live_row() {
    use echo_core::application::ports::PlaylistRepository;
    use echo_core::domain::ids::PlaylistId;
    use serde_json::json;

    let directory = tempfile::tempdir().unwrap();
    let database = SqliteDatabase::open(directory.path().join("echo.db")).unwrap();
    let root = LibraryRootId::new();
    LibraryRepository::upsert(
        &database,
        &LibraryRoot::new(root, directory.path().join("library"), true, true),
    )
    .unwrap();
    let playlist = PlaylistId::new();
    database.create(playlist, root, "Mix").unwrap();
    PlaylistRepository::delete(&database, playlist).unwrap();

    let pending = database.pending_local_mutations(50).unwrap();
    let deletion = pending
        .iter()
        .find(|candidate| {
            candidate.object == ObjectKey::Playlist(playlist)
                && candidate.payload == json!({"tombstone": true})
        })
        .expect("the delete must reach the queue");
    assert!(
        ensure_candidate_root(&database, root, deletion).is_ok(),
        "a delete's row is gone by design; its own queue entry is the durable \
         root record"
    );

    // The guard still rejects a live-object event whose object does not exist,
    // so this is a scoped exemption for deletions, not a blanket disable.
    let stranger = MutationCandidate {
        mutation_id: MutationId::new(),
        origin_device_id: DeviceId::new(),
        origin_seq: 1,
        previous_mutation_id: None,
        object: ObjectKey::Song(echo_core::domain::ids::SongId::new()),
        base_object_version: None,
        payload: json!({"title": "ghost"}),
        resource_refs: Vec::new(),
    };
    assert!(ensure_candidate_root(&database, root, &stranger).is_err());
}

/// The peer cycle is handed this scoped wrapper, never the database itself.
///
/// A port method the wrapper forgets to forward falls back to the trait
/// default, and neither default is harmless here: re-anchoring fails with
/// "adapter cannot re-anchor a pending origin chain" — which the caller folds
/// into a generic library error, so the user is told to check a music
/// directory that was never the problem — and the edit time silently
/// disappears, degrading change-time arbitration to "the host always wins" on
/// exactly the replica path. Both have to reach the database.
#[test]
fn scoped_queue_forwards_re_anchoring_and_the_local_change_time() {
    use echo_core::domain::entities::Song;
    use echo_core::domain::ids::{RelativeMediaPath, Revision, SongId};
    use echo_core::domain::sync::OriginAnchor;

    let directory = tempfile::tempdir().unwrap();
    let database = SqliteDatabase::open(directory.path().join("scoped.sqlite")).unwrap();
    let path = directory.path().join("library");
    std::fs::create_dir_all(path.join("media")).unwrap();
    prepared_empty_library(&path);
    let root = derive_root_id(&path.canonicalize().unwrap());
    LibraryRepository::upsert(&database, &LibraryRoot::new(root, path, true, true)).unwrap();
    let binding = SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    database.bind_sync_library(binding, false, root).unwrap();

    let song = SongId::new();
    SongRepository::upsert(
        &database,
        &Song::new(
            song,
            root,
            RelativeMediaPath::new("media/scoped.flac").unwrap(),
            Revision::INITIAL,
        ),
    )
    .unwrap();
    SongRepository::set_favorite(&database, song, true).unwrap();
    SongRepository::set_favorite(&database, song, false).unwrap();

    let queue = ScopedLocalQueue {
        database: &database,
        root,
    };
    let queued = queue.pending_local_mutations(512).unwrap();
    assert!(!queued.is_empty(), "the wrapper must see the pending chain");

    let changed_at = queue
        .local_mutation_changed_at_ms(queued[0].mutation_id)
        .unwrap();
    assert_eq!(
        changed_at,
        database
            .local_mutation_changed_at_ms(queued[0].mutation_id)
            .unwrap(),
        "the edit time must reach the host through the wrapper"
    );
    assert!(
        changed_at.is_some(),
        "a queued mutation always froze its change time"
    );

    let anchor = OriginAnchor {
        origin_seq: 57,
        last_mutation_id: Some(MutationId::new()),
    };
    assert_eq!(queue.rebase_local_chain(anchor).unwrap(), queued.len());
    let rebased = queue.pending_local_mutations(512).unwrap();
    assert_eq!(rebased[0].origin_seq, 58);
    assert_eq!(
        rebased[0].previous_mutation_id, anchor.last_mutation_id,
        "the re-anchored chain must continue the host position"
    );
}
