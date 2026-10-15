use super::*;
use crate::domain::sync::{AuthorityId, DeviceId, Epoch, LibraryId, SyncBinding};

fn sync_binding() -> SyncBinding {
    SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::from_u64(7),
    }
}

#[test]
fn sync_configuration_is_absent_until_bound_then_reports_stable_identity_and_genesis() {
    let (_directory, database, root) = database();
    assert!(database.current_sync_configuration().unwrap().is_none());

    let binding = sync_binding();
    database.bind_sync_library(binding, true, root).unwrap();
    let configuration = database
        .current_sync_configuration()
        .unwrap()
        .expect("host configuration");
    assert_eq!(configuration.binding, binding);
    assert!(configuration.host);
    assert_eq!(configuration.local_root, root);
    assert_eq!(
        configuration.local_device,
        DeviceId::from_uuid(database.current_device_id().as_uuid())
    );

    let checkpoint = database
        .latest_published_sync_checkpoint(binding)
        .expect("genesis checkpoint");
    assert_eq!(checkpoint.binding, binding);
    assert_eq!(checkpoint.sequence, crate::domain::sync::CommitSeq::GENESIS);
    assert_eq!(checkpoint.digest, [0; 32]);
}

#[test]
fn replica_configuration_has_no_host_credentials_and_authorized_handles_revoke_cleanly() {
    let (_replica_directory, replica, root) = database();
    let binding = sync_binding();
    replica.bind_sync_library(binding, false, root).unwrap();
    assert!(!replica.current_sync_configuration().unwrap().unwrap().host);
    assert!(replica.authorized_sync_peer_handles().unwrap().is_empty());

    let (_host_directory, host, host_root) = database();
    let host_binding = sync_binding();
    host.bind_sync_library(host_binding, true, host_root)
        .unwrap();
    let peer = DeviceId::new();
    host.authorize_sync_peer(
        host_binding,
        peer,
        "Windows desktop".to_owned(),
        "keyring://echo/library/peer".to_owned(),
    )
    .unwrap();
    assert_eq!(
        host.authorized_sync_peer_handles().unwrap(),
        [(peer, "keyring://echo/library/peer".to_owned())]
    );
    host.revoke_sync_peer(peer).unwrap();
    assert!(host.authorized_sync_peer_handles().unwrap().is_empty());
}

#[test]
fn explicit_sync_rebuild_clears_lan_lineage_but_keeps_library_and_device_state() {
    let (_directory, database, root) = database();
    let binding = sync_binding();
    database.bind_sync_library(binding, true, root).unwrap();
    let peer = DeviceId::new();
    database
        .authorize_sync_peer(binding, peer, "old peer".into(), "vault-old".into())
        .unwrap();

    database.reset_lan_sync_for_rebuild().unwrap();

    assert!(database.current_sync_configuration().unwrap().is_none());
    assert!(database.authorized_sync_peer_handles().unwrap().is_empty());
    let root_uuid = root.to_string();
    let root_still_exists: bool = database
        .writer
        .run(move |connection| {
            connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM library_roots WHERE uuid=?1)",
                    [root_uuid],
                    |row| row.get(0),
                )
                .map_err(crate::infrastructure::sqlite::support::storage)
        })
        .unwrap();
    assert!(root_still_exists);
}

#[test]
fn invalid_configuration_role_is_rejected_instead_of_defaulting_to_a_role() {
    let (_directory, database, root) = database();
    database
        .bind_sync_library(sync_binding(), true, root)
        .unwrap();
    database
        .writer
        .run(|connection| {
            connection
                .execute_batch(
                    "PRAGMA ignore_check_constraints=ON; UPDATE sync_binding SET role='observer' WHERE singleton=1; PRAGMA ignore_check_constraints=OFF;",
                )
                .map_err(crate::infrastructure::sqlite::support::storage)
        })
        .unwrap();
    assert!(matches!(
        database.current_sync_configuration(),
        Err(Error::InvariantViolation { .. })
    ));
}

#[test]
fn malformed_configuration_identity_root_and_published_digest_fail_closed() {
    use crate::application::ports::HostCommitLedgerPort;

    let (_directory, db, root) = database();
    let binding = sync_binding();
    db.bind_sync_library(binding, true, root).unwrap();
    db
        .writer
        .run(|connection| {
            connection
                .execute_batch(
                    "PRAGMA ignore_check_constraints=ON; UPDATE sync_binding SET library_id='invalid' WHERE singleton=1; PRAGMA ignore_check_constraints=OFF;",
                )
                .map_err(crate::infrastructure::sqlite::support::storage)
        })
        .unwrap();
    assert!(matches!(
        db.current_sync_configuration(),
        Err(Error::InvariantViolation { .. })
    ));

    let (_directory, db, root) = database();
    let binding = sync_binding();
    db.bind_sync_library(binding, true, root).unwrap();
    db.writer
        .run(|connection| {
            connection
                .execute(
                    "UPDATE sync_binding SET local_root_uuid=NULL WHERE singleton=1",
                    [],
                )
                .map_err(crate::infrastructure::sqlite::support::storage)
        })
        .unwrap();
    assert!(matches!(
        db.current_sync_configuration(),
        Err(Error::Conflict { .. })
    ));

    let (_directory, db, root) = database();
    let binding = sync_binding();
    db.bind_sync_library(binding, true, root).unwrap();
    let song = crate::domain::ids::SongId::new();
    let candidate = crate::domain::sync::MutationCandidate {
        resource_refs: Vec::new(),
        mutation_id: crate::domain::sync::MutationId::new(),
        origin_device_id: DeviceId::from_uuid(db.current_device_id().as_uuid()),
        origin_seq: 1,
        previous_mutation_id: None,
        object: crate::domain::sync::ObjectKey::Song(song),
        base_object_version: None,
        payload: serde_json::json!({
            "song_uuid": song.to_string(),
            "relative_path": "published.flac",
            "title": "published",
            "artist": null,
            "album": null,
            "blake3_hash": null,
            "file_size": null,
            "duration_ms": null,
            "format": null,
            "bitrate_bps": null,
            "sample_rate_hz": null,
            "channels": null,
            "bits_per_sample": null,
            "added_at": 1,
        }),
    };
    let checkpoint = db
        .stage_confirmation(binding, &candidate)
        .unwrap()
        .checkpoint
        .unwrap();
    db.mark_published(checkpoint).unwrap();
    db
        .writer
        .run(|connection| {
            connection
                .execute_batch(
                    "PRAGMA ignore_check_constraints=ON; UPDATE sync_commits SET commit_digest=x'01' WHERE publication_state='published'; PRAGMA ignore_check_constraints=OFF;",
                )
                .map_err(crate::infrastructure::sqlite::support::storage)
        })
        .unwrap();
    assert!(matches!(
        db.latest_published_sync_checkpoint(binding),
        Err(Error::InvariantViolation { .. })
    ));
}
