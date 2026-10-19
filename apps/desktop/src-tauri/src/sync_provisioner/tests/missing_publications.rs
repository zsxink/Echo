use super::*;
use echo_core::application::ports::{CoverRepository, LyricsRepository, UnitOfWork};
use echo_core::domain::entities::SongAvailability;
use echo_core::domain::ids::SongId;

fn audio_with_embedded_lyrics() -> Vec<u8> {
    let source = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../fixtures/audio/tone-short.flac"
    ));
    let mut result = source[..4].to_vec();
    let mut offset = 4;
    loop {
        let flags = source[offset];
        let length = (usize::from(source[offset + 1]) << 16)
            | (usize::from(source[offset + 2]) << 8)
            | usize::from(source[offset + 3]);
        let mut block = source[offset + 4..offset + 4 + length].to_vec();
        if flags & 0x7f == 4 {
            let vendor_length =
                usize::try_from(u32::from_le_bytes(block[..4].try_into().unwrap())).unwrap();
            let count_offset = 4 + vendor_length;
            let count =
                u32::from_le_bytes(block[count_offset..count_offset + 4].try_into().unwrap());
            block[count_offset..count_offset + 4].copy_from_slice(&(count + 1).to_le_bytes());
            let lyrics = b"LYRICS=[00:00.00]Recovered embedded lyrics";
            block.extend_from_slice(&u32::try_from(lyrics.len()).unwrap().to_le_bytes());
            block.extend_from_slice(lyrics);
        }
        result.push(flags);
        result.extend_from_slice(&u32::try_from(block.len()).unwrap().to_be_bytes()[1..]);
        result.extend_from_slice(&block);
        offset += 4 + length;
        if flags & 0x80 != 0 {
            break;
        }
    }
    result.extend_from_slice(&source[offset..]);
    result
}

#[tokio::test]
async fn maintenance_restores_completed_missing_audio_without_new_transfers_or_mutations() {
    missing_publication_scenario("present").await;
}

#[tokio::test]
async fn maintenance_does_not_restore_removed_or_changed_published_audio() {
    missing_publication_scenario("removed").await;
    missing_publication_scenario("changed").await;
}

#[allow(clippy::too_many_lines)]
async fn missing_publication_scenario(state: &str) {
    let fixture = LanMergeFixture::new().await;
    let id = SongId::new();
    let bytes = audio_with_embedded_lyrics();
    let logical_path = format!("media/{id}.flac");
    let manifest = SharedResourceManifest::new(
        SharedResourceKind::Audio,
        ResourceDigest::from_bytes(*blake3::hash(&bytes).as_bytes()),
        bytes.len() as u64,
        "audio/flac".into(),
        &logical_path,
    )
    .unwrap();
    publish_test_song(
        &fixture.host,
        fixture.peer.binding,
        id,
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
            &mut std::io::Cursor::new(&bytes),
        )
        .unwrap();
    fixture
        .provisioner
        .merge_peer_library(
            &fixture.peer,
            &fixture.endpoint,
            &fixture.resources,
            fixture.root,
        )
        .await
        .unwrap();
    let mut row = SongRepository::by_id(fixture.replica.as_ref(), id)
        .unwrap()
        .unwrap();
    let path = fixture
        .directory
        .path()
        .join("peer")
        .join(row.path().normalized());
    let meta = fixture
        .provisioner
        .scan_deps
        .fs
        .file_meta(fixture.root, row.path())
        .unwrap();
    row.apply_scan_facts(
        manifest.digest.to_hex(),
        manifest.length,
        meta.modified_ns,
        echo_core::domain::media::AudioFormat::Flac,
    );
    // Reproduce an old completed publication with a non-NULL scan stamp but
    // no extracted assets; the old full scan then incorrectly marked it missing.
    fixture
        .replica
        .with_tx(Box::new(move |tx| tx.refresh_song_file_index(&row)))
        .unwrap();
    fixture
        .replica
        .set_availability(id, SongAvailability::Missing)
        .unwrap();
    assert!(SongRepository::by_id(fixture.replica.as_ref(), id)
        .unwrap()
        .unwrap()
        .file_mtime_ns()
        .is_some());
    assert!(fixture.replica.cover_of(id).unwrap().is_none());
    assert!(fixture.replica.candidates(id).unwrap().is_empty());
    let queue = fixture.replica.pending_local_mutations(512).unwrap();
    let checkpoint = fixture
        .replica
        .replica_checkpoint(fixture.peer.host_device_id)
        .unwrap();
    let prepares = fixture.host_prepares.load(Ordering::Relaxed);
    assert!(fixture
        .replica
        .pending_remote_resource_sets(fixture.root)
        .unwrap()
        .is_empty());
    assert!(fixture
        .replica
        .pending_remote_song_metadata(fixture.root)
        .unwrap()
        .contains(&id));

    match state {
        "removed" => std::fs::remove_file(&path).unwrap(),
        "changed" => {
            let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
            let mut changed = bytes.clone();
            *changed.last_mut().unwrap() ^= 1;
            std::fs::write(&path, changed).unwrap();
            std::fs::File::open(&path)
                .unwrap()
                .set_modified(modified)
                .unwrap();
        }
        "present" => {}
        _ => unreachable!(),
    }
    fixture.provisioner.refresh_published_metadata(fixture.root);
    let restored = SongRepository::by_id(fixture.replica.as_ref(), id)
        .unwrap()
        .unwrap();
    assert_eq!(restored.id(), id);
    assert_eq!(fixture.replica.pending_local_mutations(512).unwrap(), queue);
    assert_eq!(
        fixture
            .replica
            .replica_checkpoint(fixture.peer.host_device_id)
            .unwrap(),
        checkpoint
    );
    assert_eq!(fixture.host_prepares.load(Ordering::Relaxed), prepares);
    if state == "present" {
        assert_eq!(restored.availability(), SongAvailability::Available);
        assert!(fixture.replica.cover_of(id).unwrap().is_some());
        assert!(fixture
            .replica
            .candidates(id)
            .unwrap()
            .iter()
            .any(|candidate| candidate.raw_text().contains("Recovered embedded lyrics")));
        assert!(!fixture
            .replica
            .pending_remote_song_metadata(fixture.root)
            .unwrap()
            .contains(&id));
    } else {
        assert_eq!(restored.availability(), SongAvailability::Missing);
        assert!(fixture.replica.cover_of(id).unwrap().is_none());
        assert!(fixture.replica.candidates(id).unwrap().is_empty());
    }
}
