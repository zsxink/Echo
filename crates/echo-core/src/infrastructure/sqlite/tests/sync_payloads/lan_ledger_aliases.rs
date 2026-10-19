//! Ledger confirmation staging and the member-alias delete/re-add flows (compact record + log projections).

#[allow(clippy::wildcard_imports)]
use super::*;

#[test]
fn host_ledger_stages_confirmation_idempotently_and_tracks_publication() {
    use crate::application::ports::HostCommitLedgerPort;
    use crate::domain::sync::{
        AuthorityId, DeviceId as SyncDeviceId, Epoch, LibraryId, MutationCandidate, MutationId,
        ObjectKey,
    };
    use serde_json::json;
    let (_directory, database, root) = database();
    let binding = crate::domain::sync::SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    database
        .bind_sync_library(binding, true, root)
        .expect("bind host");
    let song = SongId::new();
    let candidate = MutationCandidate {
        resource_refs: Vec::new(),
        mutation_id: MutationId::new(),
        origin_device_id: SyncDeviceId::from_uuid(database.current_device_id().as_uuid()),
        origin_seq: 1,
        previous_mutation_id: None,
        object: ObjectKey::Song(song),
        base_object_version: None,
        payload: sync_song_payload(song, "host confirmation"),
    };
    let accepted = database
        .stage_confirmation(binding, &candidate)
        .expect("stage confirmation");
    assert!(accepted.checkpoint.is_some());
    assert_eq!(
        database
            .stage_confirmation(binding, &candidate)
            .expect("idempotent retry"),
        accepted
    );
    assert_eq!(database.pending_publications(10).expect("pending").len(), 1);
    database
        .mark_published(accepted.checkpoint.expect("checkpoint"))
        .expect("publish");
    assert!(database.pending_publications(10).expect("empty").is_empty());
    let commits = database
        .published_commits_after(
            binding,
            crate::domain::sync::CommitSeq::GENESIS,
            [0; 32],
            10,
        )
        .expect("read published commit");
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0].sequence, accepted.checkpoint.unwrap().sequence);
    assert!(database
        .published_commits_after(
            binding,
            crate::domain::sync::CommitSeq::GENESIS,
            [1; 32],
            10,
        )
        .is_err());

    let changed = MutationCandidate {
        payload: json!({"title":"different body"}),
        ..candidate
    };
    assert!(database.stage_confirmation(binding, &changed).is_err());
}

#[test]
fn concurrent_duplicate_playlist_append_persists_alias_in_log_and_snapshot_state() {
    use crate::application::ports::{HostCommitLedgerPort, SongRepository};
    use crate::domain::sync::{
        AuthorityId, DeviceId, Epoch, LibraryId, MutationCandidate, MutationId, ObjectKey,
        SyncBinding,
    };
    let (_directory, host, root) = database();
    let (_replica_directory, replica, replica_root) = database();
    let binding = SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    host.bind_sync_library(binding, true, root).expect("bind host");
    replica.bind_sync_library(binding, false, replica_root).expect("bind replica");
    let library_song = song(root, "alias.flac", "Alias", "Artist");
    SongRepository::upsert(&host, &library_song).expect("insert song");
    let playlist = crate::domain::ids::PlaylistId::new();
    let canonical = crate::domain::ids::PlaylistItemId::new();
    let alias = crate::domain::ids::PlaylistItemId::new();
    let device = DeviceId::from_uuid(host.current_device_id().as_uuid());
    let first = MutationCandidate {
        mutation_id: MutationId::new(), origin_device_id: device, origin_seq: 1,
        previous_mutation_id: None, object: ObjectKey::Playlist(playlist),
        base_object_version: None,
        payload: serde_json::json!({"playlist_uuid":playlist,"display_name":"Alias test","created_at":1,
            "members":[{"song_uuid":library_song.id(),"member_uuid":canonical,"position":0,"added_at":1}]}),
        resource_refs: Vec::new(),
    };
    let first_result = host.stage_confirmation(binding, &first).expect("first append");
    host
        .mark_published(first_result.checkpoint.expect("first checkpoint"))
        .expect("publish first append");
    let second = MutationCandidate {
        mutation_id: MutationId::new(), origin_seq: 2,
        previous_mutation_id: Some(first.mutation_id),
        base_object_version: first_result.checkpoint.map(|checkpoint| crate::domain::sync::ObjectVersion::from_commit_seq(checkpoint.sequence)),
        payload: serde_json::json!({"playlist_uuid":playlist,"display_name":"Alias test","created_at":1,
            "members":[{"song_uuid":library_song.id(),"member_uuid":alias,"position":0,"added_at":2}]}),
        ..first
    };
    let second_result = host.stage_confirmation(binding, &second).expect("deduplicate append");
    host.mark_published(second_result.checkpoint.expect("second checkpoint")).expect("publish second append");
    let (alias_count, member_count, commit_body): (i64, i64, String) = host.with_reader(|connection| {
        connection.query_row(
            "SELECT (SELECT COUNT(*) FROM sync_member_aliases WHERE playlist_uuid=?1), (SELECT COUNT(*) FROM playlist_songs WHERE playlist_uuid=?1), (SELECT canonical_json FROM sync_commits WHERE commit_seq=?2)",
            params![playlist.to_string(), i64::try_from(second_result.checkpoint.expect("checkpoint").sequence.as_u64()).expect("sequence")],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        ).map_err(super::super::support::storage)
    }).expect("persisted alias");
    assert_eq!((alias_count, member_count), (1, 1));
    let body: serde_json::Value = serde_json::from_str(&commit_body).expect("commit json");
    assert_eq!(body["aliases"][0]["kind"], "playlist_member");
    assert_eq!(body["aliases"][0]["alias"]["alias"], serde_json::json!(alias));
    assert_eq!(body["aliases"][0]["alias"]["canonical"], serde_json::json!(canonical));
    let deletion = MutationCandidate {
        mutation_id: MutationId::new(),
        origin_seq: 3,
        previous_mutation_id: Some(second.mutation_id),
        object: ObjectKey::PlaylistItem(canonical),
        base_object_version: None,
        payload: serde_json::json!({"tombstone":true}),
        ..second.clone()
    };
    let deletion_result = host.stage_confirmation(binding, &deletion).expect("delete canonical member");
    host.mark_published(deletion_result.checkpoint.expect("delete checkpoint")).expect("publish member delete");
    let readded = crate::domain::ids::PlaylistItemId::new();
    let readd = MutationCandidate {
        mutation_id: MutationId::new(),
        origin_seq: 4,
        previous_mutation_id: Some(deletion.mutation_id),
        object: ObjectKey::Playlist(playlist),
        base_object_version: second_result.checkpoint.map(|checkpoint| crate::domain::sync::ObjectVersion::from_commit_seq(checkpoint.sequence)),
        payload: serde_json::json!({"playlist_uuid":playlist,"display_name":"Alias test","created_at":1,
            "members":[{"song_uuid":library_song.id(),"member_uuid":readded,"position":0,"added_at":4}]}),
        ..second
    };
    let readd_result = host.stage_confirmation(binding, &readd).expect("re-add member after final delete");
    host.mark_published(readd_result.checkpoint.expect("re-add checkpoint")).expect("publish re-add");
    let stale_snapshot = MutationCandidate {
        mutation_id: MutationId::new(),
        origin_seq: 5,
        previous_mutation_id: Some(readd.mutation_id),
        base_object_version: readd_result.checkpoint.map(|checkpoint| crate::domain::sync::ObjectVersion::from_commit_seq(checkpoint.sequence)),
        payload: serde_json::json!({"playlist_uuid":playlist,"display_name":"Alias test","created_at":1,
            "members":[{"song_uuid":library_song.id(),"member_uuid":alias,"position":0,"added_at":2}]}),
        ..readd
    };
    let stale_result = host.stage_confirmation(binding, &stale_snapshot)
        .expect("stale alias cannot redirect to the re-added member");
    host.mark_published(stale_result.checkpoint.expect("stale snapshot checkpoint"))
        .expect("publish stale snapshot resolution");
    assert_late_alias_delete_is_terminal(&host, binding, stale_snapshot, alias);
    assert_readded_member_preserves_historical_alias(&host, playlist, library_song.id(), readded, alias, canonical);
    assert_replica_applies_member_alias(&host, &replica, binding, playlist, alias, canonical);
}

fn assert_late_alias_delete_is_terminal(
    host: &crate::infrastructure::sqlite::SqliteDatabase,
    binding: crate::domain::sync::SyncBinding,
    predecessor: crate::domain::sync::MutationCandidate,
    alias: crate::domain::ids::PlaylistItemId,
) {
    use crate::application::ports::HostCommitLedgerPort;
    use crate::domain::sync::{MutationCandidate, MutationId, ObjectKey};
    let late_delete = MutationCandidate {
        mutation_id: MutationId::new(),
        origin_seq: predecessor.origin_seq + 1,
        previous_mutation_id: Some(predecessor.mutation_id),
        object: ObjectKey::PlaylistItem(alias),
        payload: serde_json::json!({"tombstone":true}),
        ..predecessor
    };
    let result = host.stage_confirmation(binding, &late_delete)
        .expect("late deletion through historical alias is terminal and idempotent");
    assert!(result.checkpoint.is_none());
    assert!(result.outcome_json.contains("object_already_deleted"));
}

fn assert_readded_member_preserves_historical_alias(
    host: &crate::infrastructure::sqlite::SqliteDatabase,
    playlist: crate::domain::ids::PlaylistId,
    song: SongId,
    readded: crate::domain::ids::PlaylistItemId,
    alias: crate::domain::ids::PlaylistItemId,
    canonical: crate::domain::ids::PlaylistItemId,
) {
    let current_member: String = host.with_reader(|connection| connection.query_row(
        "SELECT member_uuid FROM playlist_songs WHERE playlist_uuid=?1 AND song_uuid=?2",
        params![playlist.to_string(), song.to_string()],
        |row| row.get(0),
    ).map_err(super::super::support::storage)).expect("re-added playlist row");
    assert_eq!(current_member, readded.to_string());
    let historical_alias_target: String = host.with_reader(|connection| connection.query_row(
        "SELECT canonical_member_uuid FROM sync_member_aliases WHERE playlist_uuid=?1 AND alias_member_uuid=?2",
        params![playlist.to_string(), alias.to_string()],
        |row| row.get(0),
    ).map_err(super::super::support::storage)).expect("immutable historical mapping");
    assert_eq!(historical_alias_target, canonical.to_string());
}

fn assert_replica_applies_member_alias(
    host: &crate::infrastructure::sqlite::SqliteDatabase,
    replica: &crate::infrastructure::sqlite::SqliteDatabase,
    binding: crate::domain::sync::SyncBinding,
    playlist: crate::domain::ids::PlaylistId,
    alias: crate::domain::ids::PlaylistItemId,
    canonical: crate::domain::ids::PlaylistItemId,
) {
    use crate::application::ports::{HostCommitLedgerPort, RemoteApplyPort, RemoteCommit};
    use crate::domain::sync::{CommitCheckpoint, CommitSeq, DeviceId};
    let commits = host.published_commits_after(binding, CommitSeq::GENESIS, [0; 32], 10).expect("published chain");
    let remote_commits = commits.iter().map(|entry| {
        let body: serde_json::Value = serde_json::from_str(&entry.canonical_json).expect("commit body");
        let mutation = &body["mutation"];
        RemoteCommit {
            sequence: entry.sequence,
            previous_digest: entry.previous_digest,
            digest: entry.digest,
            origin_device_id: DeviceId::from_uuid(uuid::Uuid::parse_str(mutation["origin_device_id"].as_str().expect("origin")).expect("origin uuid")),
            mutation_id: crate::domain::sync::MutationId::from_uuid(uuid::Uuid::parse_str(mutation["mutation_id"].as_str().expect("mutation")).expect("mutation uuid")),
            object: serde_json::from_value(mutation["object"].clone()).expect("object key"),
            payload_json: serde_json::to_string(&mutation["payload"]).expect("canonical payload"),
            canonical_json: entry.canonical_json.clone(),
            is_tombstone: mutation["payload"] == serde_json::json!({"tombstone":true}),
        }
    }).collect::<Vec<_>>();
    replica.apply_remote_batch(binding, DeviceId::new(), CommitCheckpoint {
        binding,
        sequence: CommitSeq::GENESIS,
        digest: [0; 32],
    }, &remote_commits).expect("replica applies typed alias");
    let replica_alias_count: i64 = replica.with_reader(|connection| connection.query_row(
        "SELECT COUNT(*) FROM sync_member_aliases WHERE playlist_uuid=?1 AND alias_member_uuid=?2 AND canonical_member_uuid=?3",
        params![playlist.to_string(), alias.to_string(), canonical.to_string()],
        |row| row.get(0),
    ).map_err(super::super::support::storage)).expect("replica alias");
    assert_eq!(replica_alias_count, 1);
}

