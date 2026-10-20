use super::*;
use echo_core::application::ports::UnitOfWork;

/// Embedded tags alter the immutable audio digest even when the logical path
/// stays unchanged. The replica must send those bytes before trying to fetch
/// them from the host that has never received them.
#[tokio::test]
#[allow(clippy::too_many_lines)]
async fn replica_embedded_tag_edit_round_trips_without_downloading_its_own_new_audio() {
    embedded_tag_edit_scenario(false).await;
}

#[tokio::test]
async fn host_refusal_keeps_locally_reused_resource_and_pending_mutation() {
    embedded_tag_edit_scenario(true).await;
}

#[allow(clippy::too_many_lines)]
async fn embedded_tag_edit_scenario(refuse_host: bool) {
    use echo_core::domain::ids::SongId;
    let fixture = LanMergeFixture::new().await;
    let song_id = SongId::new();
    let original = b"original FLAC audio and embedded tags";
    let edited = b"FLAC audio with changed cover lyrics and album tags";
    let logical_path = "media/薛之谦/薛之谦 - 骆驼.flac";
    let manifest = SharedResourceManifest::new(
        SharedResourceKind::Audio,
        ResourceDigest::from_bytes(*blake3::hash(original).as_bytes()),
        original.len() as u64,
        "audio/flac".into(),
        logical_path,
    )
    .unwrap();
    publish_test_song(
        &fixture.host,
        fixture.peer.binding,
        song_id,
        2,
        Some(fixture.host_mutation),
        vec![manifest.clone()],
    );
    let host_resources = FileResourceStore::new(
        fixture
            .directory
            .path()
            .canonicalize()
            .unwrap()
            .join("host-resources"),
    )
    .unwrap();
    host_resources
        .import_reader(
            fixture.peer.host_device_id,
            &PrepareResource {
                binding: fixture.peer.binding,
                resource: ResourceRefDto::from(&manifest),
            },
            &mut std::io::Cursor::new(original),
        )
        .unwrap();
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
    let path = fixture.directory.path().join("peer").join(logical_path);
    assert_eq!(std::fs::read(&path).unwrap(), original);
    std::fs::write(&path, edited).unwrap();
    let mut song = fixture
        .replica
        .all_in_root(fixture.root)
        .unwrap()
        .into_iter()
        .find(|song| song.id() == song_id)
        .unwrap();
    song.apply_scan_facts(
        blake3::hash(edited).to_hex().to_string(),
        edited.len() as u64,
        2,
        echo_core::domain::media::AudioFormat::Flac,
    );
    let edited_manifest = SharedResourceManifest::new(
        SharedResourceKind::Audio,
        ResourceDigest::from_bytes(*blake3::hash(edited).as_bytes()),
        edited.len() as u64,
        "audio/flac".into(),
        logical_path,
    )
    .unwrap();
    // The watcher and scanner freeze resources and the song snapshot together.
    fixture
        .replica
        .with_tx(Box::new(move |tx| {
            tx.set_song_file_resources(&song, &[edited_manifest])?;
            tx.upsert_song(&song)
        }))
        .unwrap();
    let pending = fixture.replica.pending_local_mutations(512).unwrap();
    assert!(pending
        .iter()
        .any(|mutation| mutation.resource_refs.iter().any(|resource| {
            resource.digest.to_hex() == blake3::hash(edited).to_hex().as_str()
        })));
    assert!(host_resources
        .download(
            fixture.peer.binding,
            blake3::hash(edited).to_hex().as_ref(),
            0,
            edited.len()
        )
        .is_err());

    // A missing local source must still fail honestly, without discarding the
    // immutable local queue or pretending the host received the resource.
    std::fs::remove_file(&path).unwrap();
    assert_eq!(
        fixture
            .provisioner
            .merge_peer_library(
                &fixture.client,
                &fixture.peer,
                &fixture.endpoint,
                &fixture.resources,
                fixture.root,
            )
            .await,
        Err(SyncRuntimeError::SyncResourceUnavailable),
    );
    assert_eq!(
        fixture.replica.pending_local_mutations(512).unwrap(),
        pending
    );
    std::fs::write(&path, edited).unwrap();

    std::fs::write(&path, vec![b'X'; edited.len()]).unwrap();
    assert_eq!(
        fixture
            .provisioner
            .publish_remote_resources(
                &fixture.client,
                &fixture.peer,
                &fixture.endpoint,
                fixture.root,
                &fixture.resources,
            )
            .await,
        Err(SyncRuntimeError::SyncResourceUnavailable),
    );
    assert_eq!(
        fixture.replica.pending_local_mutations(512).unwrap(),
        pending
    );
    std::fs::write(&path, edited).unwrap();

    // Local reuse publishes verified local bytes only. It is independent from
    // a host ACK, and must leave pending mutations and the upload cache alone.
    fixture
        .provisioner
        .publish_remote_resources(
            &fixture.client,
            &fixture.peer,
            &fixture.endpoint,
            fixture.root,
            &fixture.resources,
        )
        .await
        .unwrap();
    assert_eq!(
        fixture.replica.pending_local_mutations(512).unwrap(),
        pending
    );
    assert!(!fixture
        .provisioner
        .is_uploaded(fixture.peer.binding, blake3::hash(edited).to_hex().as_ref(),));
    assert!(host_resources
        .download(
            fixture.peer.binding,
            blake3::hash(edited).to_hex().as_ref(),
            0,
            edited.len(),
        )
        .is_err());

    if refuse_host {
        // Host refusal after local reuse cannot erase the pending edit, rewrite
        // the host's accepted declaration, or count the resource as uploaded.
        let peer_device = DeviceId::from_uuid(fixture.replica.current_device_id().as_uuid());
        fixture.host.revoke_sync_peer(peer_device).unwrap();
        assert_eq!(
            fixture
                .provisioner
                .merge_peer_library(
                    &fixture.client,
                    &fixture.peer,
                    &fixture.endpoint,
                    &fixture.resources,
                    fixture.root,
                )
                .await,
            Err(SyncRuntimeError::SyncAuthorizationFailed),
        );
        assert_eq!(
            fixture.replica.pending_local_mutations(512).unwrap(),
            pending
        );
        assert_eq!(std::fs::read(&path).unwrap(), edited);
        assert!(!fixture
            .provisioner
            .is_uploaded(fixture.peer.binding, blake3::hash(edited).to_hex().as_ref(),));
        assert!(fixture
            .host
            .local_library_resource_manifests(fixture.host_root)
            .unwrap()
            .contains(&manifest));
        return;
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
        .expect("replica uploads its locally edited audio instead of fetching it from the host");
    assert!(fixture
        .replica
        .pending_local_mutations(512)
        .unwrap()
        .is_empty());
    assert_eq!(std::fs::read(path).unwrap(), edited);
    let songs = fixture.replica.all_in_root(fixture.root).unwrap();
    let song = songs.iter().find(|song| song.id() == song_id).unwrap();
    assert_eq!(song.path().normalized(), logical_path);
    assert_eq!(songs.iter().filter(|song| song.id() == song_id).count(), 1);
    let mut refreshed = song.clone();
    refreshed.apply_scan_facts(
        blake3::hash(edited).to_hex().to_string(),
        edited.len() as u64,
        3,
        echo_core::domain::media::AudioFormat::Flac,
    );
    fixture
        .replica
        .with_tx(Box::new(move |tx| tx.refresh_song_file_index(&refreshed)))
        .unwrap();
    assert!(fixture
        .replica
        .pending_local_mutations(512)
        .unwrap()
        .is_empty());

    assert_eq!(
        host_resources
            .download(
                fixture.peer.binding,
                blake3::hash(edited).to_hex().as_ref(),
                0,
                edited.len()
            )
            .unwrap()
            .bytes,
        edited
    );
}
