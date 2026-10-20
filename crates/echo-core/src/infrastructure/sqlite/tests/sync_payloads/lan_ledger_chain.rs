//! Outbox origin cursors, predecessor rejections, legacy terminal rejections and replica chain application.

#[allow(clippy::wildcard_imports)]
use super::*;

#[test]
fn host_local_outbox_allocation_does_not_advance_confirmed_origin_cursor() {
    use crate::application::ports::{HostCommitLedgerPort, SongRepository};
    use crate::domain::sync::{
        AuthorityId, DeviceId, Epoch, LibraryId, MutationCandidate, MutationId, ObjectKey,
        SyncBinding,
    };
    use serde_json::Value;

    let (_directory, database, root) = database();
    let binding = SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    database
        .bind_sync_library(binding, true, root)
        .expect("bind host");
    let local_song = song(root, "queued.flac", "Queued", "Artist");
    SongRepository::upsert(&database, &local_song).expect("local mutation");

    let queued: (String, i64, Option<String>, String, String) = database
        .with_reader(|connection| {
            connection
                .query_row(
                    "SELECT origin_device_id, origin_seq, previous_mutation_id, mutation_id, payload_json FROM sync_mutations WHERE object_type='song' AND object_uuid=?1 ORDER BY origin_seq LIMIT 1",
                    params![local_song.id().to_string()],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
                )
                .map_err(super::support::storage)
        })
        .expect("queued candidate");
    let candidate = MutationCandidate {
        resource_refs: Vec::new(),
        mutation_id: MutationId::from_uuid(uuid::Uuid::parse_str(&queued.3).expect("mutation id")),
        origin_device_id: DeviceId::from_uuid(uuid::Uuid::parse_str(&queued.0).expect("device id")),
        origin_seq: u64::try_from(queued.1).expect("origin sequence"),
        previous_mutation_id: queued.2.map(|value| {
            MutationId::from_uuid(uuid::Uuid::parse_str(&value).expect("previous id"))
        }),
        object: ObjectKey::Song(local_song.id()),
        base_object_version: None,
        payload: serde_json::from_str::<Value>(&queued.4).expect("payload"),
    };
    let confirmed = database
        .stage_confirmation(binding, &candidate)
        .expect("confirm the host's own queued event");
    assert!(confirmed.checkpoint.is_some());
    let cursors: (i64, i64) = database
        .with_reader(|connection| {
            connection
                .query_row(
                    "SELECT (SELECT origin_seq FROM sync_local_origin_cursors WHERE origin_device_id=?1), (SELECT origin_seq FROM sync_origin_cursors WHERE origin_device_id=?1)",
                    params![queued.0],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(super::support::storage)
        })
        .expect("separate allocation and confirmation cursors");
    assert_eq!(cursors, (1, 1));

    let mut changed = local_song.clone();
    changed.apply_metadata(Some("after confirmation".to_owned()), None, None, None);
    SongRepository::upsert(&database, &changed).expect("second local edit");
    let next_base: i64 = database
        .with_reader(|connection| {
            connection
                .query_row(
                    "SELECT base_object_version FROM sync_mutations WHERE object_type='song' AND object_uuid=?1 ORDER BY origin_seq DESC LIMIT 1",
                    params![local_song.id().to_string()],
                    |row| row.get(0),
                )
                .map_err(super::support::storage)
        })
        .expect("captured accepted base version");
    assert_eq!(
        next_base, 1,
        "new local mutations remember the last confirmed object version"
    );
    let cursors: (i64, i64) = database
        .with_reader(|connection| {
            connection
                .query_row(
                    "SELECT (SELECT origin_seq FROM sync_local_origin_cursors WHERE origin_device_id=?1), (SELECT origin_seq FROM sync_origin_cursors WHERE origin_device_id=?1)",
                    params![queued.0],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(super::support::storage)
        })
        .expect("local and host cursors remain separate");
    assert_eq!(cursors, (2, 1));
}

#[test]
fn terminal_deleted_object_rejection_consumes_origin_chain_position() {
    use crate::application::ports::{DeviceIdProvider, HostCommitLedgerPort};
    use crate::domain::sync::{
        AuthorityId, CommitSeq, DeviceId, Epoch, LibraryId, MutationCandidate, MutationId,
        ObjectKey, SyncBinding,
    };
    use serde_json::json;
    let (_directory, database, root) = database();
    let binding = SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    database
        .bind_sync_library(binding, true, root)
        .expect("bind host");
    let origin = DeviceId::from_uuid(database.current_device_id().as_uuid());
    let song = SongId::new();
    let deletion = MutationCandidate {
        resource_refs: Vec::new(),
        mutation_id: MutationId::new(),
        origin_device_id: origin,
        origin_seq: 1,
        previous_mutation_id: None,
        object: ObjectKey::Song(song),
        base_object_version: None,
        payload: json!({"tombstone":true}),
    };
    let deletion_checkpoint = database
        .stage_confirmation(binding, &deletion)
        .unwrap()
        .checkpoint
        .expect("deletion checkpoint");
    database.mark_published(deletion_checkpoint).unwrap();
    let stale_edit = MutationCandidate {
        mutation_id: MutationId::new(),
        origin_seq: 2,
        previous_mutation_id: Some(deletion.mutation_id),
        payload: json!({"title":"old edit"}),
        ..deletion
    };
    let rejected = database
        .stage_confirmation(binding, &stale_edit)
        .expect("terminal rejection");
    assert!(rejected.checkpoint.is_none());
    assert!(rejected.outcome_json.contains("object_already_deleted"));
    let next_song = SongId::new();
    let next = MutationCandidate {
        mutation_id: MutationId::new(),
        origin_seq: 3,
        previous_mutation_id: Some(stale_edit.mutation_id),
        object: ObjectKey::Song(next_song),
        payload: sync_song_payload(next_song, "unblocked"),
        ..stale_edit
    };
    let accepted = database
        .stage_confirmation(binding, &next)
        .expect("next event is not blocked");
    assert_eq!(
        accepted.checkpoint.expect("accepted checkpoint").sequence,
        CommitSeq::from_u64(2)
    );
}

#[test]
fn replica_applies_published_chain_atomically_and_replays_as_noop() {
    use crate::application::ports::{HostCommitLedgerPort, RemoteApplyPort, RemoteCommit};
    use crate::domain::sync::{
        AuthorityId, CommitCheckpoint, CommitSeq, DeviceId, Epoch, LibraryId, MutationCandidate,
        MutationId, ObjectKey, SyncBinding,
    };
    let (_host_dir, host, host_root) = database();
    let (_replica_dir, replica, replica_root) = database();
    let binding = SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    host.bind_sync_library(binding, true, host_root)
        .expect("bind host");
    replica
        .bind_sync_library(binding, false, replica_root)
        .expect("bind replica");
    let origin = DeviceId::from_uuid(host.current_device_id().as_uuid());
    let song = SongId::new();
    let candidate = MutationCandidate {
        resource_refs: Vec::new(),
        mutation_id: MutationId::new(),
        origin_device_id: origin,
        origin_seq: 1,
        previous_mutation_id: None,
        object: ObjectKey::Song(song),
        base_object_version: None,
        payload: sync_song_payload(song, "replicated"),
    };
    let staged = host
        .stage_confirmation(binding, &candidate)
        .expect("confirm");
    let checkpoint = staged.checkpoint.expect("checkpoint");
    host.mark_published(checkpoint).expect("publish");
    let published = host
        .published_commits_after(binding, CommitSeq::GENESIS, [0; 32], 4)
        .expect("pull");
    let canonical: serde_json::Value =
        serde_json::from_str(&published[0].canonical_json).expect("canonical body");
    let commit = RemoteCommit {
        sequence: published[0].sequence,
        previous_digest: published[0].previous_digest,
        digest: published[0].digest,
        origin_device_id: origin,
        mutation_id: candidate.mutation_id,
        object: candidate.object,
        payload_json: canonical
            .pointer("/mutation/payload")
            .expect("payload")
            .to_string(),
        canonical_json: published[0].canonical_json.clone(),
        is_tombstone: false,
    };
    let peer = DeviceId::new();
    let anchor = CommitCheckpoint {
        binding,
        sequence: CommitSeq::GENESIS,
        digest: [0; 32],
    };
    let mut forged_tombstone = commit.clone();
    forged_tombstone.is_tombstone = true;
    assert!(
        replica
            .apply_remote_batch(binding, peer, anchor, &[forged_tombstone])
            .is_err(),
        "tombstone interpretation is authenticated by the canonical body"
    );
    let applied = replica
        .apply_remote_batch(binding, peer, anchor, std::slice::from_ref(&commit))
        .expect("apply");
    assert_eq!(applied.sequence, checkpoint.sequence);
    assert_eq!(applied.digest, checkpoint.digest);
    assert_eq!(applied.applied, 1);
    let replay = replica
        .apply_remote_batch(binding, peer, anchor, &[commit])
        .expect("idempotent replay");
    assert_eq!(replay.sequence, checkpoint.sequence);
    assert_eq!(replay.digest, checkpoint.digest);
    assert_eq!(replay.applied, 0);
    let local_outbox: i64 = replica
        .with_reader(|connection| {
            connection
                .query_row("SELECT COUNT(*) FROM sync_outbox", [], |row| row.get(0))
                .map_err(super::support::storage)
        })
        .expect("outbox count");
    assert_eq!(
        local_outbox, 0,
        "remote commits never echo into the local outbox"
    );
}

#[test]
fn rejected_missing_predecessor_can_be_retried_after_predecessor_is_confirmed() {
    use crate::application::ports::DeviceIdProvider;
    use crate::application::ports::HostCommitLedgerPort;
    use crate::domain::sync::{
        AuthorityId, CommitSeq, DeviceId as SyncDeviceId, Epoch, LibraryId, MutationCandidate,
        MutationId, ObjectKey, SyncBinding,
    };

    let (_directory, database, root) = database();
    let binding = SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    database.bind_sync_library(binding, true, root).unwrap();
    let origin = SyncDeviceId::from_uuid(database.current_device_id().as_uuid());
    let predecessor_id = MutationId::new();
    let queued_song = SongId::new();
    let queued = MutationCandidate {
        resource_refs: Vec::new(),
        mutation_id: MutationId::new(),
        origin_device_id: origin,
        origin_seq: 2,
        previous_mutation_id: Some(predecessor_id),
        object: ObjectKey::Song(queued_song),
        base_object_version: None,
        payload: sync_song_payload(queued_song, "second"),
    };
    let rejected = database.stage_confirmation(binding, &queued).unwrap();
    assert!(rejected.checkpoint.is_none());
    assert!(rejected.outcome_json.contains("OutOfOrderPredecessor"));
    assert_eq!(
        database.stage_confirmation(binding, &queued).unwrap(),
        rejected
    );

    let first_song = SongId::new();
    let first = MutationCandidate {
        resource_refs: Vec::new(),
        mutation_id: predecessor_id,
        origin_device_id: origin,
        origin_seq: 1,
        previous_mutation_id: None,
        object: ObjectKey::Song(first_song),
        base_object_version: None,
        payload: sync_song_payload(first_song, "first"),
    };
    let first_checkpoint = database
        .stage_confirmation(binding, &first)
        .unwrap()
        .checkpoint
        .expect("predecessor checkpoint");
    database.mark_published(first_checkpoint).unwrap();
    let accepted = database.stage_confirmation(binding, &queued).unwrap();
    assert!(accepted.checkpoint.is_some());
    let commits = database
        .published_commits_after(binding, CommitSeq::GENESIS, [0; 32], 8)
        .unwrap();
    assert_eq!(commits.len(), 1, "only the published predecessor is readable");
}

#[test]
fn legacy_terminal_rejection_does_not_block_its_immediate_successor() {
    use crate::application::ports::HostCommitLedgerPort;
    use crate::domain::sync::{
        AuthorityId, DeviceId, Epoch, LibraryId, MutationCandidate, MutationId, ObjectKey,
        SyncBinding,
    };

    let (_directory, database, root) = database();
    let binding = SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    database.bind_sync_library(binding, true, root).unwrap();
    let origin = DeviceId::from_uuid(database.current_device_id().as_uuid());
    let rejected_id = MutationId::new();
    let song = SongId::new();
    let following = MutationCandidate {
        resource_refs: Vec::new(),
        mutation_id: MutationId::new(),
        origin_device_id: origin,
        origin_seq: 2,
        previous_mutation_id: Some(rejected_id),
        object: ObjectKey::Song(song),
        base_object_version: None,
        payload: sync_song_payload(song, "following mutation"),
    };
    let out_of_order = database.stage_confirmation(binding, &following).unwrap();
    assert!(out_of_order.checkpoint.is_none());
    assert!(out_of_order.outcome_json.contains("OutOfOrderPredecessor"));

    let playlist = crate::domain::ids::PlaylistId::new();
    let rejected = MutationCandidate {
        resource_refs: Vec::new(),
        mutation_id: rejected_id,
        origin_device_id: origin,
        origin_seq: 1,
        previous_mutation_id: None,
        object: ObjectKey::Playlist(playlist),
        base_object_version: None,
        payload: serde_json::json!({"title": "invalid playlist payload"}),
    };

    let outcome = database.stage_confirmation(binding, &rejected).unwrap();
    assert!(outcome.checkpoint.is_none());
    assert!(outcome.outcome_json.contains("WrongObject"));
    // The old host may already have durably cached the retryable rejection for
    // this exact successor before the rejected predecessor was recorded.
    // Simulate its missing cursor update, then ensure retry re-evaluates the
    // cached out-of-order result and recovers from the immediate predecessor.
    let origin_for_delete = origin;
    database
        .writer
        .run(move |connection| {
            connection
                .execute(
                    "DELETE FROM sync_origin_cursors WHERE origin_device_id=?1",
                    [origin_for_delete.as_uuid().to_string()],
                )
                .map_err(crate::infrastructure::sqlite::support::storage)?;
            Ok(())
        })
        .unwrap();
    let accepted = database.stage_confirmation(binding, &following).unwrap();
    assert!(accepted.checkpoint.is_some());
}

