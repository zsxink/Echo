//! Cross-module contract tests for synchronization domain values.

use echo_core::domain::ids::{PlaylistId, SongId};
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
