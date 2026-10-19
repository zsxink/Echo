//! Native first/incremental merge cycles: pull-before-upload, local
//! preservation, page draining, upload preparation idempotence and held-watch
//! outcomes.

#[allow(clippy::wildcard_imports)]
use super::tests::*;

#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn native_merge_pulls_host_before_uploading_and_preserves_local_song() {
    use echo_core::domain::entities::Song;
    use echo_core::domain::ids::{RelativeMediaPath, Revision, SongId};
    let fixture = LanMergeFixture::new().await;
    let local_song = SongId::new();
    let mut song = Song::new(
        local_song,
        fixture.root,
        RelativeMediaPath::new("media/local.flac").unwrap(),
        Revision::INITIAL,
    );
    let bytes = b"keep local audio";
    song.apply_scan_facts(
        blake3::hash(bytes).to_hex().to_string(),
        bytes.len() as u64,
        1,
        echo_core::domain::media::AudioFormat::Flac,
    );
    SongRepository::upsert(fixture.replica.as_ref(), &song).unwrap();
    let sentinel = fixture.directory.path().join("peer/media/local.flac");
    std::fs::write(&sentinel, b"keep local audio").unwrap();

    fixture
        .replica
        .ensure_replica_sync_baseline(fixture.peer.binding, fixture.root)
        .unwrap();
    for manifest in fixture
        .replica
        .local_sync_resource_manifests(fixture.root)
        .unwrap()
    {
        fixture
            .resources
            .import_reader(
                DeviceId::from_uuid(fixture.replica.current_device_id().as_uuid()),
                &PrepareResource {
                    binding: fixture.peer.binding,
                    resource: ResourceRefDto::from(&manifest),
                },
                &mut std::io::Cursor::new(bytes),
            )
            .unwrap();
    }
    fixture
        .provisioner
        .merge_peer_library(
            &fixture.client,
            &fixture.peer,
            &fixture.endpoint,
            &fixture.resources,
            fixture.root,
        )
        .await
        .unwrap();

    assert!(fixture.checkpoint.observed.load(Ordering::Relaxed));
    for (database, root) in [
        (&fixture.host, fixture.host_root),
        (&fixture.replica, fixture.root),
    ] {
        let ids = database
            .all_in_root(root)
            .unwrap()
            .iter()
            .map(echo_core::domain::entities::Song::id)
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(
            ids,
            std::collections::HashSet::from([fixture.host_song, local_song])
        );
    }
    assert!(fixture
        .replica
        .pending_local_mutations(512)
        .unwrap()
        .is_empty());
    assert_eq!(std::fs::read(&sentinel).unwrap(), b"keep local audio");
    let host_resources = FileResourceStore::new(
        fixture
            .directory
            .path()
            .canonicalize()
            .unwrap()
            .join("host-resources"),
    )
    .unwrap();
    assert_eq!(
        host_resources
            .download(
                fixture.peer.binding,
                blake3::hash(bytes).to_hex().as_ref(),
                0,
                bytes.len()
            )
            .unwrap()
            .bytes,
        bytes
    );
    fixture
        .provisioner
        .merge_peer_library(
            &fixture.client,
            &fixture.peer,
            &fixture.endpoint,
            &fixture.resources,
            fixture.root,
        )
        .await
        .unwrap();
    assert_eq!(
        fixture.host.all_in_root(fixture.host_root).unwrap().len(),
        2
    );
    assert_eq!(std::fs::read(sentinel).unwrap(), b"keep local audio");
}

#[tokio::test]
/// A steady idle cycle must reach the host once, not three times. The catch-up
/// pull that used to open every cycle was redundant with the pull inside
/// `synchronize_once`, and the closing pull only exists for cycles that pushed
/// something. Empty pages are free individually and expensive in aggregate:
/// this is the per-cycle tax a fallback (5 s) peer paid 12 times a minute.
async fn an_idle_cycle_reaches_the_host_exactly_once() {
    let fixture = LanMergeFixture::new().await;
    fixture
        .provisioner
        .merge_peer_library(
            &fixture.client,
            &fixture.peer,
            &fixture.endpoint,
            &fixture.resources,
            fixture.root,
        )
        .await
        .unwrap();
    assert!(
        SongRepository::by_id(fixture.replica.as_ref(), fixture.host_song)
            .unwrap()
            .is_some(),
        "dropping the opening pull must not drop the host's commits"
    );
    let after_warmup = fixture.host_pulls.load(Ordering::Relaxed);
    fixture
        .provisioner
        .merge_peer_library(
            &fixture.client,
            &fixture.peer,
            &fixture.endpoint,
            &fixture.resources,
            fixture.root,
        )
        .await
        .unwrap();
    assert_eq!(
        fixture.host_pulls.load(Ordering::Relaxed) - after_warmup,
        1,
        "a caught-up cycle reads one commit page, not one per phase"
    );
}

#[tokio::test]
async fn native_pull_drains_more_than_one_commit_page_without_losing_local_work() {
    use echo_core::domain::entities::Song;
    use echo_core::domain::ids::{RelativeMediaPath, Revision, SongId};
    let fixture = LanMergeFixture::new().await;
    let local_song = SongId::new();
    SongRepository::upsert(
        fixture.replica.as_ref(),
        &Song::new(
            local_song,
            fixture.root,
            RelativeMediaPath::new("media/local.flac").unwrap(),
            Revision::INITIAL,
        ),
    )
    .unwrap();
    let queued = fixture.replica.pending_local_mutations(512).unwrap();
    let mut previous = fixture.host_mutation;
    let mut last = None;
    for sequence in 2..=513 {
        let (checkpoint, mutation) = publish_test_song(
            &fixture.host,
            fixture.peer.binding,
            SongId::new(),
            sequence,
            Some(previous),
            vec![],
        );
        previous = mutation;
        last = Some(checkpoint);
    }
    let queue = ScopedLocalQueue {
        database: fixture.replica.as_ref(),
        root: fixture.root,
    };
    fixture
        .provisioner
        .pull_host_changes(&fixture.client, &fixture.peer, &fixture.endpoint, &queue)
        .await
        .unwrap();
    assert_eq!(
        queue
            .replica_checkpoint(fixture.peer.host_device_id)
            .unwrap(),
        last.unwrap()
    );
    assert_eq!(
        fixture.replica.all_in_root(fixture.root).unwrap().len(),
        514
    );
    assert_eq!(
        fixture.replica.pending_local_mutations(512).unwrap(),
        queued
    );
}

#[tokio::test]
#[ignore = "real LAN host rate limit makes 513 uploads take several minutes"]
async fn native_merge_drains_more_than_512_local_additions() {
    use echo_core::application::ports::PlaylistRepository;
    use echo_core::domain::ids::PlaylistId;
    let fixture = LanMergeFixture::new().await;
    // Pair an empty replica first, then add more than one upload page locally.
    fixture
        .replica
        .ensure_replica_sync_baseline(fixture.peer.binding, fixture.root)
        .unwrap();
    for index in 0..513 {
        PlaylistRepository::create(
            fixture.replica.as_ref(),
            PlaylistId::new(),
            fixture.root,
            &format!("Local playlist {index}"),
        )
        .unwrap();
    }
    fixture
        .replica
        .ensure_replica_sync_baseline(fixture.peer.binding, fixture.root)
        .unwrap();
    assert_eq!(
        fixture.replica.pending_local_mutations(512).unwrap().len(),
        512
    );
    fixture
        .provisioner
        .merge_peer_library(
            &fixture.client,
            &fixture.peer,
            &fixture.endpoint,
            &fixture.resources,
            fixture.root,
        )
        .await
        .unwrap();
    assert!(fixture
        .replica
        .pending_local_mutations(512)
        .unwrap()
        .is_empty());
    assert_eq!(
        fixture.host.all_in_root(fixture.host_root).unwrap().len(),
        1
    );
    assert!(fixture.checkpoint.observed.load(Ordering::Relaxed));
    assert_eq!(
        PlaylistRepository::list(fixture.host.as_ref(), fixture.host_root)
            .unwrap()
            .len(),
        513
    );
    assert_eq!(
        PlaylistRepository::list(fixture.replica.as_ref(), fixture.root)
            .unwrap()
            .len(),
        513
    );
}

#[tokio::test]
async fn native_enrollment_persists_host_manifests_before_downloading_audio() {
    use echo_core::domain::ids::SongId;
    let fixture = LanMergeFixture::new().await;
    let song = SongId::new();
    let manifest = SharedResourceManifest::new(
        SharedResourceKind::Audio,
        ResourceDigest::from_bytes(*blake3::hash(b"not seeded on host").as_bytes()),
        18,
        "audio/flac".into(),
        &format!("media/{song}.flac"),
    )
    .unwrap();
    publish_test_song(
        &fixture.host,
        fixture.peer.binding,
        song,
        2,
        Some(fixture.host_mutation),
        vec![manifest.clone()],
    );

    // The host resource store deliberately lacks these bytes. Catalog enrollment
    // must finish first so a later media transfer can resume from durable state.
    fixture
        .provisioner
        .enroll_peer_snapshot(&fixture.client, &fixture.peer, &fixture.endpoint)
        .await
        .unwrap();

    assert!(fixture
        .replica
        .replica_enrollment_complete(fixture.peer.host_device_id)
        .unwrap());
    let sets = fixture
        .replica
        .pending_remote_resource_sets(fixture.root)
        .unwrap();
    assert_eq!(sets.len(), 1);
    assert_eq!(sets[0].0, ObjectKey::Song(song));
    assert_eq!(sets[0].1[0].manifest, manifest);
    assert!(fixture
        .resources
        .verify_complete(fixture.peer.binding, &ResourceRefDto::from(&manifest))
        .is_err());
}

/// An incremental commit page must land its catalog objects even when the host
/// cannot serve the referenced bytes yet. Media is attached afterwards from the
/// durable manifests, exactly like snapshot enrollment, so a stalled transfer
/// cannot park song and playlist updates behind it.
#[tokio::test]
async fn native_incremental_applies_objects_before_audio_arrives() {
    use echo_core::domain::ids::SongId;
    let fixture = LanMergeFixture::new().await;
    let song = SongId::new();
    let manifest = SharedResourceManifest::new(
        SharedResourceKind::Audio,
        ResourceDigest::from_bytes(*blake3::hash(b"incremental bytes not on host").as_bytes()),
        18,
        "audio/flac".into(),
        &format!("media/{song}.flac"),
    )
    .unwrap();
    publish_test_song(
        &fixture.host,
        fixture.peer.binding,
        song,
        2,
        Some(fixture.host_mutation),
        vec![manifest.clone()],
    );

    let queue = ScopedLocalQueue {
        database: fixture.replica.as_ref(),
        root: fixture.root,
    };
    let pulled = tokio::time::timeout(
        Duration::from_secs(5),
        fixture.provisioner.pull_host_changes(
            &fixture.client,
            &fixture.peer,
            &fixture.endpoint,
            &queue,
        ),
    )
    .await
    .expect("object application must not wait for the media transfer");
    pulled.unwrap();

    // The object exists, is explicitly unavailable rather than absent, and its
    // verified bytes are still outstanding for publication to attach later.
    let projected = SongRepository::by_id(fixture.replica.as_ref(), song)
        .unwrap()
        .expect("incremental page must materialize the song without its bytes");
    assert!(
        !projected.availability().is_playable(),
        "a confirmed remote song without its bytes must stay unavailable"
    );
    let sets = fixture
        .replica
        .pending_remote_resource_sets(fixture.root)
        .unwrap();
    assert_eq!(sets.len(), 1);
    assert_eq!(sets[0].0, ObjectKey::Song(song));
    assert_eq!(sets[0].1[0].manifest, manifest);
    assert!(fixture
        .resources
        .verify_complete(fixture.peer.binding, &ResourceRefDto::from(&manifest))
        .is_err());
}

/// A resource is prepared once per session. The unacked mutation keeps
/// declaring it every cycle, so without the session cache a 512-deep backlog
/// re-sends `prepare` for the whole queue each round and re-reads each blob
/// twice to re-derive BLAKE3.
#[tokio::test]
async fn native_upload_prepares_a_resource_once_until_the_host_acks_it() {
    use echo_core::domain::entities::Song;
    use echo_core::domain::ids::{RelativeMediaPath, Revision, SongId};
    let fixture = LanMergeFixture::new().await;
    let song = Song::new(
        SongId::new(),
        fixture.root,
        RelativeMediaPath::new("media/upload.flac").unwrap(),
        Revision::INITIAL,
    );
    let bytes = b"uploadable audio bytes";
    let mut song = song;
    song.apply_scan_facts(
        blake3::hash(bytes).to_hex().to_string(),
        bytes.len() as u64,
        1,
        echo_core::domain::media::AudioFormat::Flac,
    );
    SongRepository::upsert(fixture.replica.as_ref(), &song).unwrap();
    std::fs::write(
        fixture.directory.path().join("peer/media/upload.flac"),
        bytes,
    )
    .unwrap();
    fixture
        .replica
        .ensure_replica_sync_baseline(fixture.peer.binding, fixture.root)
        .unwrap();
    let device = DeviceId::from_uuid(fixture.replica.current_device_id().as_uuid());
    for manifest in fixture
        .replica
        .local_sync_resource_manifests(fixture.root)
        .unwrap()
    {
        fixture
            .resources
            .import_reader(
                device,
                &PrepareResource {
                    binding: fixture.peer.binding,
                    resource: ResourceRefDto::from(&manifest),
                },
                &mut std::io::Cursor::new(bytes),
            )
            .unwrap();
    }
    let pending = fixture.replica.pending_local_mutations(512).unwrap();
    assert!(!pending.is_empty());
    assert!(pending
        .iter()
        .flat_map(|mutation| &mutation.resource_refs)
        .any(|resource| resource.kind == SharedResourceKind::Audio));
    // The baseline enqueues one mutation per local projection, so several of
    // them re-declare the same audio resource. Each distinct resource is
    // prepared exactly once.
    let distinct: std::collections::HashSet<_> = pending
        .iter()
        .flat_map(|mutation| &mutation.resource_refs)
        .map(|resource| resource.digest.to_hex())
        .collect();
    assert!(!distinct.is_empty());

    fixture
        .provisioner
        .upload_pending_resources(
            &fixture.client,
            &fixture.peer,
            &fixture.endpoint,
            fixture.root,
        )
        .await
        .unwrap();
    assert_eq!(
        fixture.host_prepares.load(Ordering::Relaxed),
        distinct.len()
    );

    // The host has not acknowledged the mutation, so the next cycle selects the
    // same resource again; it must not cost another request or another hash.
    fixture
        .provisioner
        .upload_pending_resources(
            &fixture.client,
            &fixture.peer,
            &fixture.endpoint,
            fixture.root,
        )
        .await
        .unwrap();
    assert_eq!(fixture.host_prepares.load(Ordering::Relaxed), 1);
}

/// Pull until the replica sits at the host's head, so the next commit page is
/// empty and the host holds it open for the watch under test.
async fn pull_replica_to_host_head(fixture: &LanMergeFixture) {
    let queue = ScopedLocalQueue {
        database: fixture.replica.as_ref(),
        root: fixture.root,
    };
    fixture
        .provisioner
        .pull_host_changes(&fixture.client, &fixture.peer, &fixture.endpoint, &queue)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_host_publication_ends_a_held_watch() {
    use echo_core::domain::ids::SongId;
    let fixture = LanMergeFixture::new().await;
    pull_replica_to_host_head(&fixture).await;

    let cancelled = Arc::new(AtomicBool::new(false));
    // Records when the host's publication committed, so the assertion below
    // measures propagation latency rather than the test's own settle delay.
    let published_at = Arc::new(Mutex::new(None::<Instant>));
    let joined = tokio::time::timeout(Duration::from_secs(5), async {
        let published_at = Arc::clone(&published_at);
        tokio::join!(
            async {
                // Let the watch settle into the held page before publishing.
                tokio::time::sleep(Duration::from_millis(300)).await;
                let committed = Instant::now();
                publish_test_song(
                    &fixture.host,
                    fixture.peer.binding,
                    SongId::new(),
                    2,
                    Some(fixture.host_mutation),
                    vec![],
                );
                *published_at
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(committed);
            },
            fixture.provisioner.watch_peer_commits(
                &fixture.client,
                &fixture.peer,
                fixture.root,
                cancelled,
                Duration::from_secs(30),
            ),
        )
    })
    .await;
    let ((), outcome) = joined.expect("a held watch must end when the host publishes");
    assert_eq!(outcome.unwrap(), WatchOutcome::Advanced);
    // The host rechecks the held page every 500 ms, so the peer learns about a
    // publication in about one second — the plain-poll floor was five.
    let latency = published_at
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .expect("the publication committed")
        .elapsed();
    assert!(
        latency < Duration::from_secs(2),
        "the held watch took {latency:?} to notice the publication"
    );
}

#[tokio::test]
async fn a_local_write_ends_a_held_watch() {
    use echo_core::application::ports::PlaylistRepository;
    use echo_core::domain::ids::PlaylistId;
    let fixture = LanMergeFixture::new().await;
    pull_replica_to_host_head(&fixture).await;

    let cancelled = Arc::new(AtomicBool::new(false));
    let joined = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(
            async {
                tokio::time::sleep(Duration::from_millis(300)).await;
                // A local write wakes every sync listener, including the
                // channel this provisioner registered for its watch.
                PlaylistRepository::create(
                    fixture.replica.as_ref(),
                    PlaylistId::new(),
                    fixture.root,
                    "Local edit",
                )
                .unwrap();
            },
            fixture.provisioner.watch_peer_commits(
                &fixture.client,
                &fixture.peer,
                fixture.root,
                cancelled,
                Duration::from_secs(30),
            ),
        )
    })
    .await;
    let ((), outcome) = joined.expect("a held watch must end on a local write");
    assert_eq!(outcome.unwrap(), WatchOutcome::LocalWrite);
}
