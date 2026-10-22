//! Detached-binding cache cleanup: the durable retry marker must outlive the
//! binding identity, and a successful removal must clear it.

#[allow(clippy::wildcard_imports)]
use super::tests::*;

#[tokio::test]
async fn leave_records_and_completes_the_binding_cache_cleanup_marker() {
    let fixture = LanMergeFixture::new().await;
    let resources = fixture
        .provisioner
        .local_resource_store(fixture.peer.binding)
        .expect("peer resource partition");
    // A published resource gives the partition real bytes to reclaim.
    let owner = DeviceId::from_uuid(fixture.replica.current_device_id().as_uuid());
    let bytes = b"peer cached media";
    let resource = SharedResourceManifest::new(
        SharedResourceKind::Audio,
        ResourceDigest::from_bytes(*blake3::hash(bytes).as_bytes()),
        bytes.len() as u64,
        "audio/flac".to_owned(),
        "media/peer.flac",
    )
    .unwrap();
    resources
        .import_reader(
            owner,
            &PrepareResource {
                binding: fixture.peer.binding,
                resource: ResourceRefDto::from(&resource),
            },
            &mut std::io::Cursor::new(bytes),
        )
        .unwrap();

    // Encoding the peer identity is what `leave_sync` validates before detach;
    // here the marker/cleanup behaviour is the unit under test.
    fixture
        .host
        .record_sync_cache_cleanup(fixture.peer.binding)
        .unwrap();
    assert_eq!(
        fixture.host.pending_sync_cache_cleanups().unwrap(),
        vec![fixture.peer.binding]
    );

    resources.clear_binding(fixture.peer.binding).unwrap();
    fixture
        .host
        .complete_sync_cache_cleanup(fixture.peer.binding)
        .unwrap();
    assert!(fixture
        .host
        .pending_sync_cache_cleanups()
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn a_failed_cleanup_keeps_the_marker_and_stays_retryable_across_reopen() {
    let fixture = LanMergeFixture::new().await;
    let path = fixture.host.path().to_path_buf();
    let binding = fixture.peer.binding;
    fixture.host.record_sync_cache_cleanup(binding).unwrap();
    // Simulate a removal that could not run: the marker survives, with an
    // attempt count, and a reopened database still sees it.
    assert_eq!(fixture.host.retry_sync_cache_cleanup(binding).unwrap(), 1);
    // A second connection stands in for the next process: the fixture's
    // TempDir must stay alive or the database file itself would be deleted.
    let reopened = SqliteDatabase::open(&path).unwrap();
    assert_eq!(
        reopened.pending_sync_cache_cleanups().unwrap(),
        vec![binding]
    );
    reopened.complete_sync_cache_cleanup(binding).unwrap();
    assert!(reopened.pending_sync_cache_cleanups().unwrap().is_empty());
}
