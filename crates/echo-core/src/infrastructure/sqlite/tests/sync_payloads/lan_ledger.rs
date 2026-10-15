#[path = "remote_tombstones.rs"]
mod remote_tombstones;

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
fn host_peer_authorization_and_revocation_gate_mutation_ingress() {
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
    let peer = DeviceId::new();
    database
        .authorize_sync_peer(
            binding,
            peer,
            "Phone".to_owned(),
            "keychain://echo-peer-1".to_owned(),
        )
        .unwrap();
    let song = SongId::new();
    let candidate = MutationCandidate {
        resource_refs: Vec::new(),
        mutation_id: MutationId::new(),
        origin_device_id: peer,
        origin_seq: 1,
        previous_mutation_id: None,
        object: ObjectKey::Song(song),
        base_object_version: None,
        payload: sync_song_payload(song, "from phone"),
    };
    assert!(database
        .stage_confirmation(binding, &candidate)
        .unwrap()
        .checkpoint
        .is_some());
    database.revoke_sync_peer(peer).unwrap();
    let next = MutationCandidate {
        mutation_id: MutationId::new(),
        origin_seq: 2,
        previous_mutation_id: Some(candidate.mutation_id),
        ..candidate
    };
    assert!(database.stage_confirmation(binding, &next).is_err());
}

#[test]
fn local_revision_highwater_survives_outbox_cleanup() {
    let directory = tempfile::tempdir().expect("temp dir");
    let mut direct = open_writer(&directory.path().join("highwater.db")).expect("connection");
    apply_migrations(&mut direct).expect("migrations");
    let first = super::sync::enqueue_sync(&direct, "song", "song-1", "{}", "upsert").unwrap();
    let second =
        super::sync::enqueue_sync(&direct, "song", "song-1", "{\"v\":2}", "upsert").unwrap();
    assert_eq!((first, second), (1, 2));
    direct
        .execute(
            "DELETE FROM sync_outbox WHERE object_type = 'song' AND object_uuid = 'song-1'",
            [],
        )
        .expect("prune outbox");
    assert_eq!(
        super::sync::current_outbox_revision(&direct, "song", "song-1").unwrap(),
        2
    );
    let third =
        super::sync::enqueue_sync(&direct, "song", "song-1", "{\"v\":3}", "upsert").unwrap();
    assert_eq!(
        third, 3,
        "cleaning historical outbox rows must not reset revision"
    );
}

pub(super) fn claim_item(song: Option<SongId>, target: &str, key: &str) -> OperationItem {
    OperationItem {
        kind: OperationResourceKind::Audio,
        state: OperationState::Planned,
        song,
        source: None,
        staging_path: None,
        target_path: RelativeMediaPath::new(target).expect("path"),
        expected_hash: "a".repeat(64),
        item_key: key.to_owned(),
        claim_key: key.to_owned(),
    }
}

#[test]
fn stale_scan_upsert_preserves_favorite_play_count_and_availability() {
    let (_directory, database, root) = database();
    let original = song(root, "a.flac", "A", "甲");
    SongRepository::upsert(&database, &original).expect("insert");
    SongRepository::set_favorite(&database, original.id(), true).expect("favorite");
    SongRepository::increment_play_count(&database, original.id()).expect("play");

    // A scan writes back a stale snapshot: refreshed metadata, but the
    // in-memory copy was taken before the favorite/play/availability
    // mutations, so it carries favorite=false, count=0 and `missing`.
    let mut stale = original.clone();
    stale.apply_metadata(Some("A2".to_owned()), None, None, None);
    stale.mark_missing();
    SongRepository::upsert(&database, &stale).expect("stale write-back");

    let stored = SongRepository::by_id(&database, original.id())
        .expect("query")
        .expect("present");
    assert!(stored.favorite(), "favorite must survive a stale upsert");
    assert_eq!(
        stored.play_count().as_u64(),
        1,
        "play count must survive a stale upsert"
    );
    assert_eq!(
        stored.availability(),
        SongAvailability::Available,
        "availability only changes through its dedicated mutation"
    );
    // The metadata itself did update.
    assert_eq!(stored.title(), Some("A2"));
}

#[test]
fn pending_delete_hides_from_catalog_and_finalize_removes_members_atomically() {
    let (_directory, database, root) = database();
    let song = song(root, "pd.flac", "PD", "甲");
    SongRepository::upsert(&database, &song).expect("song");
    let playlist = PlaylistId::new();
    database.create(playlist, root, "歌单").expect("playlist");
    database
        .add_member(playlist, song.id(), u64::MAX)
        .expect("member");

    // Pending delete hides the song from catalog views…
    SongRepository::set_availability(&database, song.id(), SongAvailability::PendingDelete)
        .expect("pending delete");
    assert!(database
        .query_active_songs("", SongSort::default(), None, 10)
        .expect("page")
        .items
        .is_empty());
    // …while identity and associations are kept.
    let stored = SongRepository::by_id(&database, song.id())
        .expect("query")
        .expect("present");
    assert_eq!(stored.availability(), SongAvailability::PendingDelete);
    assert_eq!(database.members(playlist).expect("members").len(), 1);

    // Delete finalization removes the membership rows inside one transaction.
    database
        .with_tx(Box::new(move |tx: &mut dyn TxAccess| {
            tx.remove_member(playlist, song.id())
        }))
        .expect("finalize");
    assert!(database.members(playlist).expect("members").is_empty());
    assert!(database.delete(playlist).is_ok());
}

#[test]
fn playlist_members_reject_cross_root_and_position_conflicts() {
    let (directory, database, root) = database();
    let root_two = LibraryRootId::new();
    LibraryRepository::upsert(
        &database,
        &LibraryRoot::new(root_two, directory.path().join("two"), false, true),
    )
    .expect("second (inactive) root");
    let song_a = song(root, "a.flac", "A", "甲");
    SongRepository::upsert(&database, &song_a).expect("a");
    let song_b = song(root_two, "b.flac", "B", "乙");
    SongRepository::upsert(&database, &song_b).expect("b");
    let playlist = PlaylistId::new();
    database.create(playlist, root, "歌单").expect("playlist");

    // A playlist never references songs of another library root.
    assert!(
        database
            .add_member(playlist, song_b.id(), u64::MAX)
            .is_err(),
        "cross-root membership must be rejected"
    );

    // Appending takes the next free position; re-adding is idempotent…
    database
        .add_member(playlist, song_a.id(), u64::MAX)
        .expect("append a at 0");
    let original_member = database.members(playlist).expect("member")[0].id();
    // The same song in a different playlist is a distinct logical membership,
    // so it gets a distinct UUID rather than inheriting the song identity.
    let other_playlist = PlaylistId::new();
    database
        .create(other_playlist, root, "另一张歌单")
        .expect("other playlist");
    database
        .add_member(other_playlist, song_a.id(), u64::MAX)
        .expect("member in other playlist");
    assert_ne!(
        original_member,
        database.members(other_playlist).expect("other member")[0].id()
    );
    // …but an explicit position clash with another member is a real conflict,
    // never a silent no-op (no INSERT OR IGNORE on position).
    let other = song(root, "c.flac", "C", "丙");
    SongRepository::upsert(&database, &other).expect("c");
    assert!(
        database.add_member(playlist, other.id(), 0).is_err(),
        "position clash must surface as an error"
    );
    database
        .add_member(playlist, song_a.id(), u64::MAX)
        .expect("re-adding an existing member is idempotent");
    let repeated = database.members(playlist).expect("member after re-add")[0].clone();
    assert_eq!(repeated.id(), original_member, "member UUID remains stable");
    assert_eq!(repeated.position(), 0, "re-add preserves its append order");
    database
        .add_member(playlist, other.id(), u64::MAX)
        .expect("append c at 1");

    let members = database.members(playlist).expect("members");
    assert_eq!(members.len(), 2);
    assert_eq!(members[0].song(), song_a.id());
    assert_eq!(members[0].position(), 0);
    assert_eq!(members[1].song(), other.id());
    assert_eq!(members[1].position(), 1);

    database
        .remove_member(playlist, song_a.id())
        .expect("remove member");
    let tombstoned: i64 = database
        .with_reader(move |connection| {
            connection
                .query_row(
                    "SELECT COUNT(*) FROM tombstones WHERE object_type = 'playlist-item' AND object_uuid = ?1",
                    [original_member.to_string()],
                    |row| row.get(0),
                )
                .map_err(super::support::storage)
        })
        .expect("playlist-item tombstone query");
    assert_eq!(
        tombstoned, 1,
        "removal retains a recovery-consumable tombstone"
    );
}

#[test]
fn target_claims_block_other_operations_until_released() {
    let (_directory, database, root) = database();
    let song = song(root, "a.flac", "A", "甲");
    SongRepository::upsert(&database, &song).expect("song");

    let first = OperationId::new();
    database
        .create_operation(first, root, "import", Some(song.id()))
        .expect("envelope");
    database
        .upsert_item(first, claim_item(Some(song.id()), "新/歌.flac", "audio"))
        .expect("claim");

    // A second operation cannot claim the same target path while the first
    // claim is active…
    let second = OperationId::new();
    database
        .create_operation(second, root, "import", None)
        .expect("envelope");
    assert!(database
        .upsert_item(second, claim_item(None, "新/歌.flac", "audio"))
        .is_err());

    // …and a terminal operation releases its claims, freeing the path.
    database.release_claims(first).expect("release");
    database
        .upsert_item(second, claim_item(None, "新/歌.flac", "audio"))
        .expect("re-claim after release");
}

#[test]
fn song_lyrics_keep_multiple_candidates_for_fallback() {
    let (_directory, database, root) = database();
    let song = song(root, "lyrics.flac", "L", "甲");
    SongRepository::upsert(&database, &song).expect("song");

    // Write through a side connection: the lyrics repository itself arrives
    // with the scan pipeline; this test pins the storage contract only.
    let raw = rusqlite::Connection::open(database.path()).expect("side connection");
    for (source, kind) in [
        ("embedded", "timed"),
        ("sidecar", "plain"),
        ("override", "plain"),
    ] {
        raw.execute(
            "INSERT INTO song_lyrics (song_uuid, source, text_kind, raw_text, updated_at) VALUES (?1, ?2, ?3, 'raw', 1)",
            params![song.id().to_string(), source, kind],
        )
        .expect("candidate row");
    }
    // One candidate per source: a second embedded row is rejected…
    assert!(raw
        .execute(
            "INSERT INTO song_lyrics (song_uuid, source, text_kind, updated_at) VALUES (?1, 'embedded', 'timed', 2)",
            params![song.id().to_string()],
        )
        .is_err());
    // …and all three candidates coexist for priority selection with fallback.
    let count: i64 = raw
        .query_row(
            "SELECT COUNT(*) FROM song_lyrics WHERE song_uuid = ?1",
            params![song.id().to_string()],
            |row| row.get(0),
        )
        .expect("count");
    assert_eq!(count, 3);
}

#[test]
fn multi_playlist_membership_commits_in_one_transaction() {
    let (_directory, database, root) = database();
    let song_a = song(root, "a.flac", "A", "甲");
    SongRepository::upsert(&database, &song_a).expect("song");
    let song_id = song_a.id();
    let playlist_one = PlaylistId::new();
    let playlist_two = PlaylistId::new();
    database.create(playlist_one, root, "其一").expect("one");
    database.create(playlist_two, root, "其二").expect("two");

    // Adding the same song to several playlists is one atomic snapshot: the
    // transaction either commits every membership or none of them.
    database
        .with_tx(Box::new(move |tx: &mut dyn TxAccess| {
            tx.insert_member(&PlaylistMember::new(
                playlist_one,
                song_id,
                0,
                SongAvailability::Available,
            ))?;
            tx.insert_member(&PlaylistMember::new(
                playlist_two,
                song_id,
                0,
                SongAvailability::Available,
            ))
        }))
        .expect("multi-playlist add");
    assert_eq!(database.members(playlist_one).expect("one").len(), 1);
    assert_eq!(database.members(playlist_two).expect("two").len(), 1);

    // A failure on the second target rolls the first back as well: the song
    // joins playlist one (append) but hits an occupied position in playlist
    // two, so the whole transaction — including playlist one's membership —
    // must disappear.
    let song_c = song(root, "c.flac", "C", "丙");
    SongRepository::upsert(&database, &song_c).expect("c");
    let song_c_id = song_c.id();
    let result: Result<(), Error> = database.with_tx(Box::new(move |tx: &mut dyn TxAccess| {
        tx.insert_member(&PlaylistMember::new(
            playlist_one,
            song_c_id,
            u64::MAX,
            SongAvailability::Available,
        ))?;
        tx.insert_member(&PlaylistMember::new(
            playlist_two,
            song_c_id,
            0,
            SongAvailability::Available,
        ))
    }));
    assert!(result.is_err(), "position clash must fail the transaction");
    assert_eq!(
        database.members(playlist_one).expect("one").len(),
        1,
        "first membership of the failed transaction is rolled back"
    );
    assert_eq!(database.members(playlist_two).expect("two").len(), 1);
}

/// Task 5.3: the conditional unique target claim protects the import pipeline
/// against a live second claim on the same planned target — the claim blocks
/// until the blocking operation reaches a terminal state and releases it.
/// Register the import fixtures on the scan stack: the source content's tags
/// (content-keyed lookup) and the published path's probe/tags (path-keyed).
pub(super) fn seed_import_source(
    stack: &ScanStack,
    published: &str,
    content: &[u8],
    artist: &str,
    title: &str,
) {
    let tags = |artist: &str, title: &str| crate::domain::media::ParsedMetadata {
        artist: Some(artist.to_owned()),
        title: Some(title.to_owned()),
        ..crate::domain::media::ParsedMetadata::default()
    };
    stack.metadata.set_bytes(content, tags(artist, title));
    stack.probe.set(
        published,
        crate::application::ports::ProbeOutcome::Audio {
            format: crate::domain::media::AudioFormat::Flac,
            duration: Some(Duration::from_secs(1)),
        },
    );
    stack.metadata.set(published, tags(artist, title));
}
fn sync_song_payload(id: SongId, title: &str) -> serde_json::Value {
    serde_json::json!({
        "song_uuid": id.to_string(),
        "relative_path": format!("{title}.flac"),
        "title": title,
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
    })
}

