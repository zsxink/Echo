//! Cross-module contract tests for synchronization domain values.

use echo_core::domain::ids::{PlaylistId, PlaylistItemId, SongId};
use echo_core::domain::sync::*;

fn binding() -> SyncBinding {
    SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    }
}

#[test]
fn sequences_compare_only_inside_the_same_binding() {
    let a = binding();
    assert_eq!(
        compare_in_binding(a, CommitSeq::from_u64(3), a, CommitSeq::from_u64(4)),
        Some(SequenceRelation::Before)
    );
    let mut other = a;
    other.epoch = Epoch::from_u64(2);
    assert_eq!(
        compare_in_binding(a, CommitSeq::from_u64(99), other, CommitSeq::from_u64(1)),
        None
    );
    let mut other = a;
    other.library_id = LibraryId::new();
    assert_eq!(
        compare_in_binding(a, CommitSeq::from_u64(1), other, CommitSeq::from_u64(2)),
        None
    );
    let mut other = a;
    other.authority_id = AuthorityId::new();
    assert_eq!(
        compare_in_binding(a, CommitSeq::from_u64(1), other, CommitSeq::from_u64(2)),
        None
    );
}

#[test]
fn object_kind_is_part_of_the_key() {
    let id = SongId::new();
    assert_ne!(ObjectKey::Song(id), ObjectKey::Favorite(id));
}

#[test]
fn sequence_overflow_does_not_wrap() {
    assert_eq!(CommitSeq::from_u64(u64::MAX).next(), None);
}

#[test]
fn host_confirmation_uses_order_not_wall_clock_and_preserves_loser() {
    use serde_json::json;
    let song = ObjectKey::Song(SongId::new());
    let device_a = DeviceId::new();
    let device_b = DeviceId::new();
    let first = MutationCandidate {
        resource_refs: Vec::new(),
        mutation_id: MutationId::new(),
        origin_device_id: device_a,
        origin_seq: 1,
        previous_mutation_id: None,
        object: song,
        base_object_version: None,
        payload: json!({"title": "A"}),
    };
    let confirmed_a = confirm_candidate(None, None, &first, CommitSeq::from_u64(1)).unwrap();
    let second = MutationCandidate {
        resource_refs: Vec::new(),
        mutation_id: MutationId::new(),
        origin_device_id: device_b,
        origin_seq: 1,
        previous_mutation_id: None,
        object: song,
        base_object_version: None,
        payload: json!({"title": "B"}),
    };
    let confirmed_b = confirm_candidate(
        Some(&confirmed_a.object),
        None,
        &second,
        CommitSeq::from_u64(2),
    )
    .unwrap();
    assert_eq!(confirmed_b.object.payload, json!({"title": "B"}));
    let conflict = confirmed_b.conflict.unwrap();
    assert_eq!(conflict.replaced_payload, json!({"title": "A"}));
    assert_eq!(conflict.accepted_payload, json!({"title": "B"}));
    assert_eq!(
        confirmed_b.object.version.commit_seq(),
        CommitSeq::from_u64(2)
    );
}

#[test]
fn concurrent_playlist_appends_keep_first_member_and_emit_typed_alias() {
    use serde_json::json;
    let playlist_id = PlaylistId::new();
    let playlist = ObjectKey::Playlist(playlist_id);
    let song_a = SongId::new();
    let song_b = SongId::new();
    let canonical_member = PlaylistItemId::new();
    let duplicate_member = PlaylistItemId::new();
    let appended_member = PlaylistItemId::new();
    let first_device = DeviceId::new();
    let current = ConfirmedObject {
        object: playlist,
        version: ObjectVersion::from_commit_seq(CommitSeq::from_u64(1)),
        origin_device_id: first_device,
        payload: json!({"playlist_uuid":playlist_id,"display_name":"Mix","created_at":1,
            "members":[{"song_uuid":song_a,"member_uuid":canonical_member,"position":0,"added_at":1}]}),
    };
    let candidate = MutationCandidate {
        mutation_id: MutationId::new(),
        origin_device_id: DeviceId::new(),
        origin_seq: 1,
        previous_mutation_id: None,
        object: playlist,
        base_object_version: Some(current.version),
        payload: json!({"playlist_uuid":playlist_id,"display_name":"Mix","created_at":1,
        "members":[
            {"song_uuid":song_a,"member_uuid":duplicate_member,"position":0,"added_at":2},
            {"song_uuid":song_b,"member_uuid":appended_member,"position":1,"added_at":2}
        ]}),
        resource_refs: Vec::new(),
    };
    let result =
        confirm_candidate(Some(&current), None, &candidate, CommitSeq::from_u64(2)).unwrap();
    let members = result.object.payload["members"].as_array().unwrap();
    assert_eq!(members.len(), 2);
    assert_eq!(members[0]["member_uuid"], json!(canonical_member));
    assert_eq!(members[1]["member_uuid"], json!(appended_member));
    assert_eq!(
        result.aliases,
        vec![PlaylistMemberAlias {
            playlist_id,
            alias_member_id: duplicate_member,
            canonical_member_id: canonical_member,
        }]
    );
}

#[test]
fn same_device_predecessor_chain_is_enforced_without_self_conflict() {
    use serde_json::json;
    let device = DeviceId::new();
    let song = ObjectKey::Song(SongId::new());
    let first = MutationCandidate {
        resource_refs: Vec::new(),
        mutation_id: MutationId::new(),
        origin_device_id: device,
        origin_seq: 1,
        previous_mutation_id: None,
        object: song,
        base_object_version: None,
        payload: json!("first"),
    };
    let confirmed = confirm_candidate(None, None, &first, CommitSeq::from_u64(1)).unwrap();
    let second = MutationCandidate {
        resource_refs: Vec::new(),
        mutation_id: MutationId::new(),
        origin_device_id: device,
        origin_seq: 2,
        previous_mutation_id: Some(first.mutation_id),
        object: song,
        base_object_version: None,
        payload: json!("second"),
    };
    let next = confirm_candidate(
        Some(&confirmed.object),
        Some(confirmed.origin_cursor),
        &second,
        CommitSeq::from_u64(2),
    )
    .unwrap();
    assert!(next.conflict.is_none());
    let out_of_order = MutationCandidate {
        origin_seq: 3,
        previous_mutation_id: None,
        ..second
    };
    assert_eq!(
        confirm_candidate(
            Some(&next.object),
            Some(next.origin_cursor),
            &out_of_order,
            CommitSeq::from_u64(3)
        ),
        Err(ConfirmationError::OutOfOrderPredecessor)
    );
}

#[test]
fn delete_intent_can_be_undone_within_window_and_emits_no_tombstone() {
    let object = ObjectKey::Song(SongId::new());
    let mut intent = LocalDeleteIntent::begin(object, 1_000).unwrap();
    assert_eq!(
        intent.resolve(DeleteIntentAction::Undo, 2_000),
        Ok(DeleteIntentOutcome::Cancelled)
    );
    assert_eq!(
        intent.resolve(DeleteIntentAction::DeadlineReached, 11_000),
        Err(DeleteIntentError::AlreadyResolved)
    );
    assert_eq!(intent.state, DeleteIntentState::Cancelled);
    let mut expired = LocalDeleteIntent::begin(object, 1_000).unwrap();
    assert_eq!(
        expired.resolve(DeleteIntentAction::Undo, 11_000),
        Err(DeleteIntentError::UndoWindowExpired)
    );
}

#[test]
fn delete_intent_finalizes_once_and_retries_same_stable_event() {
    let object = ObjectKey::Playlist(PlaylistId::new());
    let mut intent = LocalDeleteIntent::begin(object, 5_000).unwrap();
    assert_eq!(
        intent.resolve(DeleteIntentAction::DeadlineReached, 14_999),
        Ok(DeleteIntentOutcome::Pending)
    );
    let first = intent
        .resolve(DeleteIntentAction::DeadlineReached, 15_000)
        .unwrap();
    assert_eq!(
        first,
        DeleteIntentOutcome::EmitFinalTombstone {
            mutation_id: intent.final_mutation_id,
            object
        }
    );
    assert_eq!(
        intent.resolve(DeleteIntentAction::DeadlineReached, 16_000),
        Ok(first)
    );
}

#[test]
fn reopened_delete_intent_keeps_its_original_deadline_and_final_event_identity() {
    let object = ObjectKey::Song(SongId::new());
    let intent = LocalDeleteIntent::begin(object, 40_000).unwrap();
    let final_id = intent.final_mutation_id;
    let encoded = serde_json::to_vec(&intent).unwrap();
    let mut reopened: LocalDeleteIntent = serde_json::from_slice(&encoded).unwrap();

    assert_eq!(
        reopened.resolve(DeleteIntentAction::DeadlineReached, 49_999),
        Ok(DeleteIntentOutcome::Pending)
    );
    let finalized = reopened
        .resolve(DeleteIntentAction::DeadlineReached, 50_000)
        .unwrap();
    assert_eq!(
        finalized,
        DeleteIntentOutcome::EmitFinalTombstone {
            mutation_id: final_id,
            object,
        }
    );
}

#[test]
fn remote_tombstone_supersedes_an_open_local_delete_intent() {
    let mut intent = LocalDeleteIntent::begin(ObjectKey::Song(SongId::new()), 50).unwrap();
    assert_eq!(
        intent.resolve(DeleteIntentAction::RemoteFinalTombstone, 100),
        Ok(DeleteIntentOutcome::RemovedByRemoteTombstone)
    );
    assert_eq!(
        intent.resolve(DeleteIntentAction::DeadlineReached, 20_000),
        Ok(DeleteIntentOutcome::RemovedByRemoteTombstone)
    );
    assert_eq!(intent.state, DeleteIntentState::SupersededByRemoteTombstone);
}

#[test]
fn typed_final_tombstone_rejects_old_identity_but_allows_new_import_identity() {
    use serde_json::json;
    let deleted = ObjectKey::Song(SongId::new());
    let tombstone = FinalTombstone {
        object: deleted,
        version: ObjectVersion::from_commit_seq(CommitSeq::from_u64(9)),
        mutation_id: MutationId::new(),
    };
    let candidate = MutationCandidate {
        resource_refs: Vec::new(),
        mutation_id: MutationId::new(),
        origin_device_id: DeviceId::new(),
        origin_seq: 1,
        previous_mutation_id: None,
        object: deleted,
        base_object_version: None,
        payload: json!({"title": "stale"}),
    };
    assert_eq!(
        check_candidate_against_tombstone(&candidate, Some(&tombstone)),
        Err(TombstoneDecisionError::ObjectAlreadyDeleted)
    );
    let reimport = MutationCandidate {
        object: ObjectKey::Song(SongId::new()),
        ..candidate.clone()
    };
    assert_eq!(
        check_candidate_against_tombstone(&reimport, Some(&tombstone)),
        Ok(())
    );
    assert_eq!(check_candidate_against_tombstone(&candidate, None), Ok(()));
}

#[test]
fn sync_session_pause_offline_cancel_and_disable_are_explicit() {
    use SyncSessionEvent as E;
    use SyncSessionState as S;
    assert_eq!(
        transition_sync_session(S::Disabled, E::Enable),
        Ok(S::Ready)
    );
    assert_eq!(
        transition_sync_session(S::Ready, E::PairingRequired),
        Ok(S::AwaitingPairing)
    );
    assert_eq!(
        transition_sync_session(S::AwaitingPairing, E::BeginPairing),
        Ok(S::Pairing)
    );
    assert_eq!(
        transition_sync_session(S::Pairing, E::PairingAccepted),
        Ok(S::Enrolling)
    );
    assert_eq!(
        transition_sync_session(S::Enrolling, E::EnrollmentComplete),
        Ok(S::Synchronizing)
    );
    assert_eq!(
        transition_sync_session(S::Synchronizing, E::Pause),
        Ok(S::Paused)
    );
    assert_eq!(
        transition_sync_session(S::Paused, E::HostUnavailable),
        Ok(S::Offline)
    );
    assert_eq!(
        transition_sync_session(S::Offline, E::Reconnected),
        Ok(S::Synchronizing)
    );
    assert_eq!(
        transition_sync_session(S::Synchronizing, E::Disable),
        Ok(S::Disabled)
    );
    assert_eq!(
        transition_sync_session(S::Disabled, E::Resume),
        Err(InvalidSyncTransition {
            from: S::Disabled,
            event: E::Resume
        })
    );
}

#[test]
fn playlist_member_aliases_are_typed_and_never_cross_playlist() {
    use echo_core::domain::ids::PlaylistItemId;
    let playlist_a = PlaylistId::new();
    let playlist_b = PlaylistId::new();
    let alias = PlaylistItemId::new();
    let canonical = PlaylistItemId::new();
    let mut registry = MemberAliasRegistry::default();
    registry
        .insert(PlaylistMemberAlias {
            playlist_id: playlist_a,
            alias_member_id: alias,
            canonical_member_id: canonical,
        })
        .unwrap();
    assert_eq!(registry.resolve(playlist_a, alias), canonical);
    assert_eq!(registry.resolve(playlist_b, alias), alias);
    assert_eq!(
        registry.insert(PlaylistMemberAlias {
            playlist_id: playlist_a,
            alias_member_id: alias,
            canonical_member_id: PlaylistItemId::new()
        }),
        Err(AliasError::AlreadyMapped)
    );
    assert_eq!(
        registry.insert(PlaylistMemberAlias {
            playlist_id: playlist_a,
            alias_member_id: canonical,
            canonical_member_id: alias,
        }),
        Err(AliasError::CyclicAlias)
    );
}

#[test]
fn concurrent_duplicate_playlist_append_aliases_the_later_confirmed_member() {
    use echo_core::domain::ids::PlaylistItemId;
    let playlist = PlaylistId::new();
    let song = SongId::new();
    let first = ConfirmedMember {
        playlist_id: playlist,
        member_id: PlaylistItemId::new(),
        song_id: song,
        confirmed_order: 10,
    };
    let later = ConfirmedMember {
        playlist_id: playlist,
        member_id: PlaylistItemId::new(),
        song_id: song,
        confirmed_order: 11,
    };
    assert_eq!(
        decide_member_append(&[first], later),
        MemberAppendDecision::Duplicate {
            canonical_member_id: first.member_id,
            alias_member_id: later.member_id
        }
    );
}

#[test]
fn shared_resource_manifest_accepts_only_safe_library_relative_metadata() {
    let manifest = SharedResourceManifest::new(
        SharedResourceKind::Audio,
        ResourceDigest::from_bytes([0xabu8; 32]),
        42,
        "audio/flac".to_owned(),
        "media/Artist/Track.flac",
    )
    .unwrap();
    assert_eq!(manifest.digest.to_hex(), "ab".repeat(32));
    assert!(SharedResourceManifest::new(
        SharedResourceKind::UserCover,
        ResourceDigest::from_bytes([0; 32]),
        1,
        "image/png".to_owned(),
        "/Users/private/cover.png",
    )
    .is_err());
    assert!(SharedResourceManifest::new(
        SharedResourceKind::LyricsSidecar,
        ResourceDigest::from_bytes([0; 32]),
        1,
        "text/plain".to_owned(),
        "media/../secret.lrc",
    )
    .is_err());
}

fn play_stats_buckets(entries: &[(uuid::Uuid, u64)]) -> serde_json::Value {
    serde_json::to_value(
        entries
            .iter()
            .copied()
            .collect::<std::collections::BTreeMap<_, _>>(),
    )
    .expect("bucket map")
}

/// A confirmed play-stats row written by an earlier release only carried
/// `{song_uuid, by_device}` — it could not attest `revision` /
/// `updated_by_device_id` / `hlc`. Such a row must still authorize exactly the
/// buckets it recorded: otherwise every later merge treats every other device's
/// confirmed count as an unauthorized raise and rejects the mutation forever.
#[test]
fn legacy_play_stats_payload_still_authorizes_the_buckets_it_recorded() {
    use serde_json::json;
    let song = SongId::new();
    let origin = DeviceId::new();
    let other = DeviceId::new();
    let current = ConfirmedObject {
        object: ObjectKey::PlayStats(song),
        version: ObjectVersion::from_commit_seq(CommitSeq::from_u64(1)),
        origin_device_id: other,
        payload: json!({
            "song_uuid": song.as_uuid(),
            "by_device": play_stats_buckets(&[(other.as_uuid(), 5)]),
        }),
    };
    let candidate = MutationCandidate {
        mutation_id: MutationId::new(),
        origin_device_id: origin,
        origin_seq: 1,
        previous_mutation_id: None,
        object: ObjectKey::PlayStats(song),
        base_object_version: Some(current.version),
        payload: json!({
            "song_uuid": song.as_uuid(),
            "by_device": play_stats_buckets(&[(origin.as_uuid(), 2), (other.as_uuid(), 5)]),
        }),
        resource_refs: Vec::new(),
    };
    let confirmed =
        confirm_candidate(Some(&current), None, &candidate, CommitSeq::from_u64(2)).unwrap();
    let merged = &confirmed.object.payload["by_device"];
    assert_eq!(merged[origin.as_uuid().to_string()], json!(2));
    assert_eq!(merged[other.as_uuid().to_string()], json!(5));

    // Raising another device's bucket stays unauthorized.
    let mut raising = candidate;
    raising.mutation_id = MutationId::new();
    raising.payload = json!({
        "song_uuid": song.as_uuid(),
        "by_device": play_stats_buckets(&[(origin.as_uuid(), 9), (other.as_uuid(), 6)]),
    });
    assert_eq!(
        confirm_candidate(Some(&current), None, &raising, CommitSeq::from_u64(3)),
        Err(ConfirmationError::UnauthorizedPlayStatsBucket)
    );
}

#[test]
fn stale_base_without_favorite_or_play_stats_value_change_is_not_a_conflict() {
    use serde_json::json;

    let song = SongId::new();
    let first_device = DeviceId::new();
    let second_device = DeviceId::new();
    let stale_base = Some(ObjectVersion::from_commit_seq(CommitSeq::from_u64(1)));
    let current_version = ObjectVersion::from_commit_seq(CommitSeq::from_u64(2));

    let favorite = ConfirmedObject {
        object: ObjectKey::Favorite(song),
        version: current_version,
        origin_device_id: first_device,
        payload: json!({"is_favorite": true, "revision": 2}),
    };
    let same_favorite = MutationCandidate {
        mutation_id: MutationId::new(),
        origin_device_id: second_device,
        origin_seq: 1,
        previous_mutation_id: None,
        object: ObjectKey::Favorite(song),
        base_object_version: stale_base,
        payload: json!({"is_favorite": true, "revision": 1}),
        resource_refs: Vec::new(),
    };
    let result = confirm_candidate(
        Some(&favorite),
        None,
        &same_favorite,
        CommitSeq::from_u64(3),
    )
    .unwrap();
    assert!(result.conflict.is_none());

    let changed_favorite = MutationCandidate {
        payload: json!({"is_favorite": false, "revision": 1}),
        ..same_favorite
    };
    let result = confirm_candidate(
        Some(&favorite),
        None,
        &changed_favorite,
        CommitSeq::from_u64(3),
    )
    .unwrap();
    assert!(
        result.conflict.is_some(),
        "a real favorite change remains visible"
    );

    let play_stats = ConfirmedObject {
        object: ObjectKey::PlayStats(song),
        version: current_version,
        origin_device_id: first_device,
        payload: json!({"by_device": {
            first_device.as_uuid().to_string(): 2,
            second_device.as_uuid().to_string(): 1,
        }}),
    };
    let same_play_stats = MutationCandidate {
        mutation_id: MutationId::new(),
        origin_device_id: second_device,
        origin_seq: 1,
        previous_mutation_id: None,
        object: ObjectKey::PlayStats(song),
        base_object_version: stale_base,
        payload: json!({"song_uuid": song.as_uuid(), "by_device": {
            first_device.as_uuid().to_string(): 2,
            second_device.as_uuid().to_string(): 1,
        }}),
        resource_refs: Vec::new(),
    };
    let result = confirm_candidate(
        Some(&play_stats),
        None,
        &same_play_stats,
        CommitSeq::from_u64(3),
    )
    .unwrap();
    assert!(result.conflict.is_none());

    let changed_play_stats = MutationCandidate {
        payload: json!({"song_uuid": song.as_uuid(), "by_device": {
            first_device.as_uuid().to_string(): 2,
            second_device.as_uuid().to_string(): 3,
        }}),
        ..same_play_stats
    };
    let result = confirm_candidate(
        Some(&play_stats),
        None,
        &changed_play_stats,
        CommitSeq::from_u64(3),
    )
    .unwrap();
    assert!(
        result.conflict.is_some(),
        "a real counter change remains visible"
    );
}

/// A conflict record is the input to the user's recovery UI. It must carry the
/// accepted (merged) object, because a recovery action echoes it back verbatim;
/// storing the submitted sketch would drop every member the losing device never
/// saw and turn "recover" into a second data loss.
#[test]
fn conflict_records_the_merged_payload_not_the_submitted_sketch() {
    use serde_json::json;
    let playlist_id = PlaylistId::new();
    let playlist = ObjectKey::Playlist(playlist_id);
    let songs = [SongId::new(), SongId::new(), SongId::new()];
    let winners = [
        PlaylistItemId::new(),
        PlaylistItemId::new(),
        PlaylistItemId::new(),
    ];
    let members: Vec<serde_json::Value> = songs
        .iter()
        .zip(winners)
        .enumerate()
        .map(|(position, (song, member))| {
            json!({"song_uuid": song, "member_uuid": member, "position": position, "added_at": 1})
        })
        .collect();
    let current = ConfirmedObject {
        object: playlist,
        version: ObjectVersion::from_commit_seq(CommitSeq::from_u64(3)),
        origin_device_id: DeviceId::new(),
        payload: json!({"playlist_uuid": playlist_id, "display_name": "Mix", "created_at": 1,
            "members": members}),
    };
    // A device that never saw members 2 and 3 renames the playlist. Its stale
    // base loses on LWW, but the stored conflict must still show all three.
    let candidate = MutationCandidate {
        mutation_id: MutationId::new(),
        origin_device_id: DeviceId::new(),
        origin_seq: 1,
        previous_mutation_id: None,
        object: playlist,
        base_object_version: Some(ObjectVersion::from_commit_seq(CommitSeq::from_u64(1))),
        payload: json!({"playlist_uuid": playlist_id, "display_name": "Mix renamed", "created_at": 1,
            "members": [members[0].clone()]}),
        resource_refs: Vec::new(),
    };
    let confirmed =
        confirm_candidate(Some(&current), None, &candidate, CommitSeq::from_u64(4)).unwrap();
    let conflict = confirmed.conflict.expect("stale base from another device");
    assert_eq!(
        conflict.replaced_payload["members"]
            .as_array()
            .unwrap()
            .len(),
        3
    );
    assert_eq!(
        conflict.accepted_payload["members"]
            .as_array()
            .unwrap()
            .len(),
        3,
        "the accepted value is the merged playlist, not the one-member sketch"
    );
    assert_eq!(conflict.accepted_payload, confirmed.object.payload);
}

/// Positions come from the merged order, never from either side's numbering. A
/// device that removed a member earlier leaves a gap in its own positions, so
/// copying them across would let two devices record different `position` values
/// for the same playlist and make every later ordering comparison ambiguous.
#[test]
fn playlist_append_renumbers_positions_from_the_merged_order() {
    use serde_json::json;
    let playlist_id = PlaylistId::new();
    let playlist = ObjectKey::Playlist(playlist_id);
    let existing_song = SongId::new();
    let new_song = SongId::new();
    let existing_member = PlaylistItemId::new();
    let new_member = PlaylistItemId::new();
    let current = ConfirmedObject {
        object: playlist,
        version: ObjectVersion::from_commit_seq(CommitSeq::from_u64(1)),
        origin_device_id: DeviceId::new(),
        payload: json!({"playlist_uuid":playlist_id,"display_name":"Mix","created_at":1,
            "members":[{"song_uuid":existing_song,"member_uuid":existing_member,"position":7,"added_at":1}]}),
    };
    let candidate = MutationCandidate {
        mutation_id: MutationId::new(),
        origin_device_id: DeviceId::new(),
        origin_seq: 1,
        previous_mutation_id: None,
        object: playlist,
        base_object_version: Some(current.version),
        payload: json!({"playlist_uuid":playlist_id,"display_name":"Mix","created_at":1,
        "members":[
            {"song_uuid":existing_song,"member_uuid":existing_member,"position":7,"added_at":1},
            {"song_uuid":new_song,"member_uuid":new_member,"position":8,"added_at":2}
        ]}),
        resource_refs: Vec::new(),
    };
    let merged =
        confirm_candidate(Some(&current), None, &candidate, CommitSeq::from_u64(2)).unwrap();
    let positions: Vec<u64> = merged.object.payload["members"]
        .as_array()
        .unwrap()
        .iter()
        .map(|member| member["position"].as_u64().unwrap())
        .collect();
    assert_eq!(positions, vec![0, 1], "positions must be renumbered 0..n");
    assert_eq!(
        merged.object.payload["members"][0]["member_uuid"],
        json!(existing_member)
    );
}
