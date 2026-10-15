use super::*;

#[test]
fn remote_tombstones_hide_objects_and_preserve_held_delete_resources() {
    use crate::application::ports::{
        OperationItem, OperationJournalRepository, OperationResourceKind, RemoteApplyPort,
    };
    use crate::domain::ids::OperationId;
    use crate::domain::state::OperationState;
    use crate::domain::sync::{
        AuthorityId, CommitCheckpoint, CommitSeq, DeviceId, Epoch, LibraryId, SyncBinding,
    };
    let (_host_dir, host, host_root) = database();
    let (_replica_dir, replica, replica_root) = database();
    let binding = SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    host.bind_sync_library(binding, true, host_root).unwrap();
    replica
        .bind_sync_library(binding, false, replica_root)
        .unwrap();
    let song = song(replica_root, "deleted.flac", "Deleted", "Artist");
    SongRepository::upsert(&replica, &song).unwrap();
    SongRepository::set_favorite(&replica, song.id(), true).unwrap();
    let playlist = PlaylistId::new();
    replica
        .create(playlist, replica_root, "Deleted playlist")
        .unwrap();
    replica.add_member(playlist, song.id(), 0).unwrap();
    let operation = OperationId::new();
    replica
        .ensure_operation(operation, replica_root, "delete", Some(song.id()))
        .unwrap();
    let item = OperationItem {
        kind: OperationResourceKind::Audio,
        state: OperationState::HiddenInDatabase,
        song: Some(song.id()),
        source: None,
        staging_path: Some(RelativeMediaPath::new("echo/trash/held/audio.flac").unwrap()),
        target_path: song.path().clone(),
        expected_hash: "a".repeat(64),
        item_key: "audio".into(),
        claim_key: song.path().identity_key().into(),
    };
    replica.upsert_item(operation, item.clone()).unwrap();
    replica
        .with_tx(Box::new(move |tx: &mut dyn TxAccess| {
            tx.set_undo_deadline(operation, i64::MAX)
        }))
        .unwrap();
    let before_outbox: i64 = replica
        .with_reader(|c| {
            c.query_row("SELECT COUNT(*) FROM sync_outbox", [], |r| r.get(0))
                .map_err(super::support::storage)
        })
        .unwrap();
    let commits = published_tombstone_commits(&host, binding, song.id(), playlist);
    let peer = DeviceId::new();
    let anchor = CommitCheckpoint {
        binding,
        sequence: CommitSeq::GENESIS,
        digest: [0; 32],
    };
    let applied = replica
        .apply_remote_batch(binding, peer, anchor, &commits)
        .unwrap();
    assert_eq!(applied.applied, 2);
    assert!(replica
        .query_active_songs("", SongSort::default(), None, 10)
        .unwrap()
        .items
        .is_empty());
    assert!(PlaylistRepository::by_id(&replica, playlist)
        .unwrap()
        .is_none());
    assert_eq!(
        SongRepository::by_id(&replica, song.id())
            .unwrap()
            .unwrap()
            .availability(),
        SongAvailability::PendingDelete
    );
    assert_held_tombstone_resources(&replica, &song, operation, item, before_outbox);
    assert_eq!(
        replica
            .apply_remote_batch(binding, peer, anchor, &commits)
            .unwrap()
            .applied,
        0
    );
}

fn assert_held_tombstone_resources(
    replica: &SqliteDatabase,
    song: &crate::domain::entities::Song,
    operation: crate::domain::ids::OperationId,
    item: crate::application::ports::OperationItem,
    before_outbox: i64,
) {
    use crate::application::ports::{OperationItem, OperationJournalRepository};
    use crate::domain::state::OperationState;
    let held = replica.items(operation).unwrap();
    assert_eq!(held[0].state, OperationState::FailedRecoverable);
    assert_eq!(held[0].staging_path, item.staging_path);
    assert_eq!(held[0].expected_hash, item.expected_hash);
    assert!(replica
        .upsert_item(
            operation,
            OperationItem {
                state: OperationState::RestorePending,
                ..item
            }
        )
        .is_err());
    assert!(
        SongRepository::set_availability(replica, song.id(), SongAvailability::Available).is_err()
    );
    assert!(SongRepository::upsert(replica, song).is_err());
    let (intent, indexed, after_outbox): (String, i64, i64) = replica
        .with_reader(|c| {
            let intent = c
                .query_row(
                    "SELECT state FROM sync_local_delete_intents WHERE object_uuid=?1",
                    params![song.id().to_string()],
                    |r| r.get(0),
                )
                .map_err(super::support::storage)?;
            let indexed = c
                .query_row(
                    "SELECT COUNT(*) FROM song_search WHERE song_uuid=?1",
                    params![song.id().to_string()],
                    |r| r.get(0),
                )
                .map_err(super::support::storage)?;
            let outbox = c
                .query_row("SELECT COUNT(*) FROM sync_outbox", [], |r| r.get(0))
                .map_err(super::support::storage)?;
            Ok((intent, indexed, outbox))
        })
        .unwrap();
    assert_eq!(intent, "remote_deleted");
    assert_eq!(indexed, 0);
    assert_eq!(after_outbox, before_outbox);
}

fn published_tombstone_commits(
    host: &SqliteDatabase,
    binding: crate::domain::sync::SyncBinding,
    song: SongId,
    playlist: PlaylistId,
) -> Vec<crate::application::ports::RemoteCommit> {
    use crate::application::ports::{HostCommitLedgerPort, RemoteCommit};
    use crate::domain::sync::{CommitSeq, DeviceId, MutationCandidate, MutationId, ObjectKey};
    let origin = DeviceId::new();
    host.authorize_sync_peer(binding, origin, "peer".into(), "test-credential".into())
        .unwrap();
    let song_deletion = MutationCandidate {
        resource_refs: Vec::new(),
        mutation_id: MutationId::new(),
        origin_device_id: origin,
        origin_seq: 1,
        previous_mutation_id: None,
        object: ObjectKey::Song(song),
        base_object_version: None,
        payload: serde_json::json!({"tombstone":true}),
    };
    let playlist_deletion = MutationCandidate {
        mutation_id: MutationId::new(),
        origin_seq: 2,
        previous_mutation_id: Some(song_deletion.mutation_id),
        object: ObjectKey::Playlist(playlist),
        ..song_deletion.clone()
    };
    let song_checkpoint = host
        .stage_confirmation(binding, &song_deletion)
        .unwrap()
        .checkpoint
        .unwrap();
    host.mark_published(song_checkpoint).unwrap();
    let checkpoint = host
        .stage_confirmation(binding, &playlist_deletion)
        .unwrap()
        .checkpoint
        .unwrap();
    host.mark_published(checkpoint).unwrap();
    let published = host
        .published_commits_after(binding, CommitSeq::GENESIS, [0; 32], 10)
        .unwrap();
    published
        .iter()
        .zip([&song_deletion, &playlist_deletion])
        .map(|(c, candidate)| RemoteCommit {
            sequence: c.sequence,
            previous_digest: c.previous_digest,
            digest: c.digest,
            origin_device_id: origin,
            mutation_id: candidate.mutation_id,
            object: candidate.object,
            canonical_json: c.canonical_json.clone(),
            payload_json: candidate.payload.to_string(),
            is_tombstone: true,
        })
        .collect()
}
