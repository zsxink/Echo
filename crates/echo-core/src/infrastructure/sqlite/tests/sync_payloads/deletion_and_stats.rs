//! Regression tests for the three paths that used to leave the host ledger
//! blind: play-statistics payload shape, membership removal, and playlist
//! deletion. Each one asserted end to end against the host confirmation port,
//! because the failure mode of the old code was "locally deleted, remotely
//! resurrected" rather than a local error.

use super::*;
use crate::application::ports::{
    DeviceIdProvider, HostCommitLedgerPort, LocalSyncQueuePort, PlaylistRepository, SongRepository,
};
use crate::domain::ids::LibraryRootId;
use crate::domain::library::PlayStatsRecord;
use crate::domain::sync::{
    AuthorityId, CommitSeq, Epoch, LibraryId, MutationCandidate, ObjectKey, SyncBinding,
};
use serde_json::json;

fn host_binding(database: &SqliteDatabase, root: LibraryRootId) -> SyncBinding {
    let binding = SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    database
        .bind_sync_library(binding, true, root)
        .expect("bind host");
    binding
}

/// Confirm every pending local mutation in origin order and publish it.
///
/// A rejection is a test failure: these are locally produced candidates, so the
/// host must be able to decode each one.
fn drain_local_queue(database: &SqliteDatabase, binding: SyncBinding) -> Vec<MutationCandidate> {
    let mut accepted = Vec::new();
    for _ in 0..64 {
        let pending = database.pending_local_mutations(50).expect("pending queue");
        if pending.is_empty() {
            return accepted;
        }
        for candidate in pending {
            let staged = database
                .stage_confirmation(binding, &candidate)
                .expect("stage confirmation");
            match staged.checkpoint {
                Some(checkpoint) => {
                    database.mark_published(checkpoint).expect("publish");
                    accepted.push(candidate);
                }
                None => panic!(
                    "a locally produced mutation was rejected: {}",
                    staged.outcome_json
                ),
            }
        }
    }
    panic!("the local queue never drained");
}

/// The committed (`published`) view of one playlist's membership.
fn confirmed_playlist_members(
    database: &SqliteDatabase,
    binding: SyncBinding,
    playlist: PlaylistId,
) -> Vec<serde_json::Value> {
    let commits = database
        .published_commits_after(binding, CommitSeq::GENESIS, [0; 32], 500)
        .expect("published log");
    let mut members = Vec::new();
    for commit in commits {
        let canonical: serde_json::Value =
            serde_json::from_str(&commit.canonical_json).expect("canonical json");
        let Some(payload) = canonical.pointer("/mutation/payload") else {
            continue;
        };
        if payload["playlist_uuid"].as_str() != Some(playlist.to_string().as_str()) {
            continue;
        }
        members = payload["members"].as_array().cloned().unwrap_or_default();
    }
    members
}

#[test]
fn recording_a_play_enqueues_a_complete_play_stats_record_the_host_accepts() {
    let (_directory, database, root) = database();
    let binding = host_binding(&database, root);
    let song = song(root, "stats.flac", "Stats", "Artist");
    SongRepository::upsert(&database, &song).expect("song");
    let session = PlaybackSessionId::new();
    assert!(database
        .record_playback(session, song.id())
        .expect("record play"));

    let pending = database.pending_local_mutations(50).expect("pending queue");
    let stats = pending
        .iter()
        .find(|candidate| matches!(candidate.object, ObjectKey::PlayStats(_)))
        .expect("recording a play must enqueue a play-stats mutation");
    // The host merges this payload as a `PlayStatsRecord`. A `{song_uuid,
    // by_device}` sketch fails to decode there and is a *terminal* `WrongObject`
    // rejection, so the shape is asserted at the producer.
    let record: PlayStatsRecord =
        serde_json::from_value(stats.payload.clone()).expect("a complete PlayStatsRecord");
    let device = database.current_device_id().as_uuid();
    assert_eq!(record.song_uuid, song.id().as_uuid());
    assert_eq!(record.updated_by_device_id.as_uuid(), device);
    assert_eq!(record.by_device.get(&device).copied(), Some(1));
    assert_eq!(record.total(), 1);

    // A session is private idempotence evidence, including when a pending
    // payload is exported to another device and accepted by the host.
    let shared = serde_json::to_string(&stats.payload).unwrap();
    assert!(!shared.contains(&session.to_string()));
    assert!(!shared.contains("recorded_sessions"));
    assert!(!shared.contains("playback_session"));
    let recovered: crate::domain::library::PortableRecord = serde_json::from_value(
        serde_json::to_value(crate::domain::library::PortableRecord::PlayStats(
            record.clone(),
        ))
        .unwrap(),
    )
    .unwrap();
    assert_eq!(
        recovered,
        crate::domain::library::PortableRecord::PlayStats(record)
    );
    database
        .with_reader(|connection| {
            let private =
                crate::infrastructure::sqlite::durability_backfill::export_playback_events(
                    connection, root,
                )?;
            assert_eq!(private.len(), 1);
            assert_eq!(private[0].id, session.as_uuid());
            assert_eq!(private[0].kind.directory(), "playback-events");
            assert_eq!(private[0].payload["song_uuid"], song.id().to_string());
            Ok(())
        })
        .unwrap();

    // End-to-end: draining the queue rejects nothing.
    drain_local_queue(&database, binding);
}

#[test]
fn removing_a_playlist_member_enqueues_a_typed_tombstone_that_blocks_resurrection() {
    let (_directory, database, root) = database();
    let binding = host_binding(&database, root);
    let first = song(root, "first.flac", "First", "Artist");
    let second = song(root, "second.flac", "Second", "Artist");
    SongRepository::upsert(&database, &first).expect("first song");
    SongRepository::upsert(&database, &second).expect("second song");
    let playlist = PlaylistId::new();
    database
        .create(playlist, root, "Mix")
        .expect("create playlist");
    database
        .add_member(playlist, first.id(), u64::MAX)
        .expect("first member");
    database
        .add_member(playlist, second.id(), u64::MAX)
        .expect("second member");
    drain_local_queue(&database, binding);
    assert_eq!(
        confirmed_playlist_members(&database, binding, playlist).len(),
        2
    );

    database
        .remove_member(playlist, second.id())
        .expect("remove member");

    let pending = database.pending_local_mutations(50).expect("pending queue");
    let member_tombstones: Vec<&MutationCandidate> = pending
        .iter()
        .filter(|candidate| matches!(candidate.object, ObjectKey::PlaylistItem(_)))
        .collect();
    assert_eq!(
        member_tombstones.len(),
        1,
        "removing a member must enqueue exactly one typed member tombstone: a \
         playlist-only snapshot cannot express a removal, because the host \
         merges playlist writes as an append-only union"
    );
    assert_eq!(member_tombstones[0].payload, json!({"tombstone": true}));

    drain_local_queue(&database, binding);
    let members = confirmed_playlist_members(&database, binding, playlist);
    assert_eq!(
        members.len(),
        1,
        "the removed member must not be merged back on confirmation"
    );
    assert_eq!(members[0]["song_uuid"], json!(first.id().to_string()));
}

#[test]
fn deleting_a_playlist_enqueues_one_delete_mutation_that_replays_idempotently() {
    let (_directory, database, root) = database();
    let binding = host_binding(&database, root);
    let playlist = PlaylistId::new();
    database
        .create(playlist, root, "Mix")
        .expect("create playlist");
    drain_local_queue(&database, binding);

    PlaylistRepository::delete(&database, playlist).expect("delete playlist");
    let pending = database.pending_local_mutations(50).expect("pending queue");
    let deletes: Vec<&MutationCandidate> = pending
        .iter()
        .filter(|candidate| candidate.object == ObjectKey::Playlist(playlist))
        .collect();
    assert_eq!(
        deletes.len(),
        1,
        "a playlist delete must reach the mutation stream, not only the local \
         tombstone table"
    );
    assert_eq!(deletes[0].payload, json!({"tombstone": true}));
    let queued = pending.len();

    // Deleting again stays idempotent: the row is gone, so only the durable
    // local tombstone is rewritten and no second mutation is produced.
    PlaylistRepository::delete(&database, playlist).expect("idempotent replay");
    assert_eq!(
        database
            .pending_local_mutations(50)
            .expect("pending queue")
            .len(),
        queued,
        "a replayed delete must not enqueue a second mutation"
    );
}

/// The undo window means a member delete is routinely queued before the add it
/// undoes has been acknowledged. Both events then travel in one batch, and the
/// host must end with no membership at all — not with the add winning because
/// the playlist snapshot still carries the member.
#[test]
fn a_member_deleted_before_its_add_is_acknowledged_never_survives() {
    let (_directory, database, root) = database();
    let binding = host_binding(&database, root);
    let track = song(root, "queued.flac", "Queued", "Artist");
    SongRepository::upsert(&database, &track).expect("song");
    let playlist = PlaylistId::new();
    database
        .create(playlist, root, "Mix")
        .expect("create playlist");

    // Deliberately no drain between the two calls.
    database
        .add_member(playlist, track.id(), u64::MAX)
        .expect("add member");
    database
        .remove_member(playlist, track.id())
        .expect("remove member");

    drain_local_queue(&database, binding);
    assert!(
        confirmed_playlist_members(&database, binding, playlist).is_empty(),
        "the host merges playlist writes as an append-only union, so the add \
         survives unless the member tombstone lands after it"
    );
    assert!(database.members(playlist).expect("members").is_empty());
}

/// Re-adding the same song mints a new member identity, so retrying the old
/// delete (an ACK that was lost, or a resend after reconnect) must be a no-op on
/// the new membership rather than erasing it a second time.
#[test]
fn retrying_an_old_member_delete_does_not_remove_the_readded_member() {
    let (_directory, database, root) = database();
    let binding = host_binding(&database, root);
    let track = song(root, "again.flac", "Again", "Artist");
    SongRepository::upsert(&database, &track).expect("song");
    let playlist = PlaylistId::new();
    database
        .create(playlist, root, "Mix")
        .expect("create playlist");
    database
        .add_member(playlist, track.id(), u64::MAX)
        .expect("add member");
    drain_local_queue(&database, binding);

    database
        .remove_member(playlist, track.id())
        .expect("remove member");
    let old_delete = database
        .pending_local_mutations(50)
        .expect("pending queue")
        .into_iter()
        .find(|candidate| matches!(candidate.object, ObjectKey::PlaylistItem(_)))
        .expect("the removal must be queued as a typed member tombstone");
    drain_local_queue(&database, binding);
    assert!(database.members(playlist).expect("members").is_empty());

    database
        .add_member(playlist, track.id(), u64::MAX)
        .expect("re-add member");
    drain_local_queue(&database, binding);
    let re_added = database.members(playlist).expect("members");
    assert_eq!(re_added.len(), 1);
    let new_member = re_added[0].id();

    // The retry keeps its original identity, so the host replays its ACK.
    let staged = database
        .stage_confirmation(binding, &old_delete)
        .expect("retry the old delete");
    assert!(
        staged.checkpoint.is_some(),
        "an accepted retry must replay its original checkpoint, not re-decide"
    );
    let after = database.members(playlist).expect("members");
    assert_eq!(
        after.len(),
        1,
        "the old tombstone must not touch the new member"
    );
    assert_eq!(after[0].id(), new_member);
    assert_eq!(
        confirmed_playlist_members(&database, binding, playlist).len(),
        1
    );
}
