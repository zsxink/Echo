use super::*;
use crate::domain::sync::{AuthorityId, DeviceId, Epoch, LibraryId, SyncBinding};

fn binding() -> SyncBinding {
    SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    }
}

#[test]
fn revoked_peer_can_be_reauthorized_without_reviving_the_old_credential() {
    let (_directory, database, root) = database();
    let binding = binding();
    database.bind_sync_library(binding, true, root).unwrap();
    let device = DeviceId::new();
    database
        .authorize_sync_peer(binding, device, "Laptop".into(), "keychain://old".into())
        .unwrap();
    assert!(
        database
            .sync_peer_has_credential(binding, device, "keychain://old".into())
            .unwrap()
    );
    assert!(
        database
            .reauthorize_sync_peer(binding, device, "Laptop 2".into(), "keychain://new".into())
            .is_err()
    );

    database.revoke_sync_peer(device).unwrap();
    assert!(
        !database
            .sync_peer_has_credential(binding, device, "keychain://old".into())
            .unwrap()
    );
    database
        .reauthorize_sync_peer(binding, device, "Laptop 2".into(), "keychain://new".into())
        .unwrap();
    assert!(
        !database
            .sync_peer_has_credential(binding, device, "keychain://old".into())
            .unwrap()
    );
    assert!(
        database
            .sync_peer_has_credential(binding, device, "keychain://new".into())
            .unwrap()
    );
}

#[test]
fn replica_enrollment_is_idempotent_only_for_an_empty_exact_target() {
    let (_directory, db, root) = database();
    let binding = binding();
    db.bind_empty_sync_replica(binding, root).unwrap();
    db.bind_empty_sync_replica(binding, root).unwrap();
    assert!(!db.current_sync_configuration().unwrap().unwrap().host);

    let other_binding = SyncBinding {
        epoch: Epoch::from_u64(2),
        ..binding
    };
    assert!(db.bind_empty_sync_replica(other_binding, root).is_err());

    let (_directory, occupied, occupied_root) = database();
    let existing = song(occupied_root, "existing.flac", "existing", "artist");
    SongRepository::upsert(&occupied, &existing).unwrap();
    assert!(
        occupied
            .bind_empty_sync_replica(binding, occupied_root)
            .is_err()
    );
}

#[test]
fn host_binding_is_idempotent_but_never_rebound_or_attached_to_a_missing_root() {
    let (_directory, db, root) = database();
    let binding = binding();
    db.bind_sync_library(binding, true, root).unwrap();
    db.bind_sync_library(binding, true, root).unwrap();

    let changed_identity = SyncBinding {
        library_id: LibraryId::new(),
        ..binding
    };
    assert!(db.bind_sync_library(changed_identity, true, root).is_err());
    assert!(db.bind_sync_library(binding, false, root).is_err());
    assert!(
        db.bind_sync_library(binding, true, crate::domain::ids::LibraryRootId::new())
            .is_err()
    );
}

#[test]
fn peer_authorization_validates_identity_and_rejects_stale_credential_cleanup() {
    let (_directory, db, root) = database();
    let binding = binding();
    let peer = DeviceId::new();
    assert!(
        db.authorize_sync_peer(binding, peer, "Laptop".into(), "keychain://active".into())
            .is_err()
    );

    db.bind_sync_library(binding, true, root).unwrap();
    assert!(
        db.authorize_sync_peer(binding, peer, "  ".into(), "keychain://active".into())
            .is_err()
    );
    assert!(
        db.authorize_sync_peer(binding, peer, "Laptop".into(), "bad\nhandle".into())
            .is_err()
    );

    db.authorize_sync_peer(binding, peer, "Laptop".into(), "keychain://active".into())
        .unwrap();
    let other_binding = SyncBinding {
        epoch: Epoch::from_u64(2),
        ..binding
    };
    assert!(
        !db.sync_peer_has_credential(other_binding, peer, "keychain://active".into())
            .unwrap()
    );
    assert!(
        !db.sync_peer_has_credential(binding, peer, "keychain://stale".into())
            .unwrap()
    );
    assert!(
        !db.revoke_sync_peer_credential(binding, peer, "keychain://stale".into())
            .unwrap()
    );
    assert!(
        db.sync_peer_has_credential(binding, peer, "keychain://active".into())
            .unwrap()
    );
    assert!(
        db.revoke_sync_peer_credential(binding, peer, "keychain://active".into())
            .unwrap()
    );
    assert!(
        !db.sync_peer_has_credential(binding, peer, "keychain://active".into())
            .unwrap()
    );
}
