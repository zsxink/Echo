use super::lan_ledger::sync_song_payload;
use super::*;

/// The newest library value wins; a tie keeps the host's current value.
///
/// Every accepted event still receives a canonical commit, including a loser,
/// so a replica can replace the optimistic value it had already shown.
#[test]
fn newer_change_time_wins_and_a_tie_keeps_the_host_value() {
    use crate::application::ports::HostCommitLedgerPort;
    use crate::domain::sync::{
        AuthorityId, DeviceId as SyncDeviceId, Epoch, LibraryId, MutationCandidate, MutationId,
        ObjectKey, SyncBinding,
    };

    let (_directory, database, root) = database();
    let binding = SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    database.bind_sync_library(binding, true, root).unwrap();
    let origin = SyncDeviceId::from_uuid(database.current_device_id().as_uuid());
    let song = SongId::new();
    let first_id = MutationId::new();
    let second_id = MutationId::new();
    let third_id = MutationId::new();

    let first = MutationCandidate {
        resource_refs: Vec::new(),
        mutation_id: first_id,
        origin_device_id: origin,
        origin_seq: 1,
        previous_mutation_id: None,
        object: ObjectKey::Song(song),
        base_object_version: None,
        payload: sync_song_payload(song, "first"),
    };
    let first_checkpoint = database
        .stage_confirmation_at(binding, &first, Some(1_000))
        .unwrap()
        .checkpoint
        .expect("first event");
    database.mark_published(first_checkpoint).unwrap();

    // A later edit replaces the confirmed value.
    let second = MutationCandidate {
        resource_refs: Vec::new(),
        mutation_id: second_id,
        origin_device_id: origin,
        origin_seq: 2,
        previous_mutation_id: Some(first_id),
        object: ObjectKey::Song(song),
        base_object_version: None,
        payload: sync_song_payload(song, "second"),
    };
    let accepted = database
        .stage_confirmation_at(binding, &second, Some(2_000))
        .unwrap()
        .checkpoint
        .expect("later edit");
    database.mark_published(accepted).unwrap();
    let payload: String = database
        .with_reader(|connection| {
            connection
                .query_row(
                    "SELECT payload_json FROM sync_object_versions WHERE object_type='song' AND object_uuid=?1",
                    [song.to_string()],
                    |row| row.get(0),
                )
                .map_err(super::support::storage)
        })
        .unwrap();
    let payload: serde_json::Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(payload["title"], "second");

    // The identical change time is the host's tie-break: its value stands, and
    // the losing event is still committed so every replica converges on it.
    let third = MutationCandidate {
        resource_refs: Vec::new(),
        mutation_id: third_id,
        origin_device_id: origin,
        origin_seq: 3,
        previous_mutation_id: Some(second_id),
        object: ObjectKey::Song(song),
        base_object_version: None,
        payload: sync_song_payload(song, "third"),
    };
    let tied = database
        .stage_confirmation_at(binding, &third, Some(2_000))
        .unwrap()
        .checkpoint
        .expect("losing event is still committed");
    database.mark_published(tied).unwrap();
    let payload: String = database
        .with_reader(|connection| {
            connection
                .query_row(
                    "SELECT payload_json FROM sync_object_versions WHERE object_type='song' AND object_uuid=?1",
                    [song.to_string()],
                    |row| row.get(0),
                )
                .map_err(super::support::storage)
        })
        .unwrap();
    let payload: serde_json::Value = serde_json::from_str(&payload).unwrap();
    assert_eq!(payload["title"], "second", "a tie keeps the host's value");
}

/// A predecessor rejection names the host's confirmed position, which is what
/// lets the sender re-anchor instead of retrying a sequence the host can never
/// accept.
#[test]
fn predecessor_rejection_reports_the_host_cursor_for_re_anchoring() {
    use crate::application::ports::HostCommitLedgerPort;
    use crate::domain::sync::{
        AuthorityId, DeviceId as SyncDeviceId, Epoch, LibraryId, MutationCandidate, MutationId,
        ObjectKey, SyncBinding,
    };

    let (_directory, database, root) = database();
    let binding = SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    database.bind_sync_library(binding, true, root).unwrap();
    let origin = SyncDeviceId::from_uuid(database.current_device_id().as_uuid());
    let first_id = MutationId::new();
    let first_song = SongId::new();
    let first = MutationCandidate {
        resource_refs: Vec::new(),
        mutation_id: first_id,
        origin_device_id: origin,
        origin_seq: 1,
        previous_mutation_id: None,
        object: ObjectKey::Song(first_song),
        base_object_version: None,
        payload: sync_song_payload(first_song, "first"),
    };
    let checkpoint = database
        .stage_confirmation(binding, &first)
        .unwrap()
        .checkpoint
        .expect("first event");
    database.mark_published(checkpoint).unwrap();

    let queued_song = SongId::new();
    let queued = MutationCandidate {
        resource_refs: Vec::new(),
        mutation_id: MutationId::new(),
        origin_device_id: origin,
        origin_seq: 60,
        previous_mutation_id: Some(MutationId::new()),
        object: ObjectKey::Song(queued_song),
        base_object_version: None,
        payload: sync_song_payload(queued_song, "detached chain"),
    };
    let rejected = database.stage_confirmation(binding, &queued).unwrap();
    assert!(rejected.checkpoint.is_none());
    let body: serde_json::Value = serde_json::from_str(&rejected.outcome_json).unwrap();
    assert_eq!(body["reason"], "OutOfOrderPredecessor");
    assert_eq!(body["origin_seq"], 1);
    assert_eq!(body["last_mutation_id"], first_id.as_uuid().to_string());
}

/// Re-anchoring changes the envelope of an event the host never recorded, so
/// the same identity must be accepted with the repaired sequence. Rejecting it
/// as "same identity, different content" would leave the queue stuck exactly
/// where the repair was supposed to unblock it.
#[test]
fn re_anchored_event_is_accepted_after_a_retryable_predecessor_rejection() {
    use crate::application::ports::HostCommitLedgerPort;
    use crate::domain::sync::{
        AuthorityId, DeviceId as SyncDeviceId, Epoch, LibraryId, MutationCandidate, MutationId,
        ObjectKey, SyncBinding,
    };

    let (_directory, database, root) = database();
    let binding = SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    database.bind_sync_library(binding, true, root).unwrap();
    let origin = SyncDeviceId::from_uuid(database.current_device_id().as_uuid());
    let first_id = MutationId::new();
    let first_song = SongId::new();
    let first = MutationCandidate {
        resource_refs: Vec::new(),
        mutation_id: first_id,
        origin_device_id: origin,
        origin_seq: 1,
        previous_mutation_id: None,
        object: ObjectKey::Song(first_song),
        base_object_version: None,
        payload: sync_song_payload(first_song, "first"),
    };
    let checkpoint = database
        .stage_confirmation(binding, &first)
        .unwrap()
        .checkpoint
        .expect("first event");
    database.mark_published(checkpoint).unwrap();

    let song = SongId::new();
    let detached = MutationCandidate {
        resource_refs: Vec::new(),
        mutation_id: MutationId::new(),
        origin_device_id: origin,
        origin_seq: 60,
        previous_mutation_id: Some(MutationId::new()),
        object: ObjectKey::Song(song),
        base_object_version: None,
        payload: sync_song_payload(song, "re-anchored"),
    };
    let rejected = database.stage_confirmation(binding, &detached).unwrap();
    assert!(rejected.outcome_json.contains("OutOfOrderPredecessor"));

    // The replica re-numbers the queue onto the cursor the rejection reported.
    let reanchored = MutationCandidate {
        origin_seq: 2,
        previous_mutation_id: Some(first_id),
        ..detached
    };
    let accepted = database.stage_confirmation(binding, &reanchored).unwrap();
    assert!(
        accepted.checkpoint.is_some(),
        "the repaired envelope must be confirmable: {}",
        accepted.outcome_json
    );
}
