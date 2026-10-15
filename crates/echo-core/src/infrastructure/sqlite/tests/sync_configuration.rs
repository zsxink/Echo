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
