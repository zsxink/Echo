//! Missing provenance and incomplete independent anchors stay explicitly unresolved.
use super::*;

#[test]
fn library_durability_trusted_sync_retry_new_author_does_not_send_old_bound_candidate() {
    let mut f = fixture();
    let author = DeviceId::new();
    f.db.writer
        .run(move |c| {
            c.execute(
                "UPDATE device_state SET value=?1 WHERE key='device_id'",
                [author.as_uuid().to_string()],
            )
            .map_err(storage)?;
            Ok(())
        })
        .unwrap();
    f.context.local_author = author;
    let report = retry(&f);
    assert_eq!(counts(&f), (0, 0, 1));
    assert_eq!(report.unresolved.len(), 2);
    assert!(report
        .unresolved
        .iter()
        .all(|r| r.reason == Reason::RequiresStateRecovery));
}

#[test]
fn library_durability_trusted_sync_retry_unverifiable_anchor_never_installs_baseline_or_pending() {
    let mut f = fixture();
    f.context.verified_checkpoint.digest = [9; 32];
    let report = retry(&f);
    assert_eq!(counts(&f), (0, 0, 0));
    assert!(report
        .unresolved
        .iter()
        .any(|r| r.reason == Reason::MissingSyncDependency));
    assert_eq!(
        PlaylistRepository::name(&f.db, f.playlist)
            .unwrap()
            .as_deref(),
        Some("B")
    );
}

#[test]
fn library_durability_trusted_sync_retry_keeps_rejected_receipt_without_ledger_proof() {
    let f = fixture();
    let rejected = MutationId::new();
    let author = f.context.local_author;
    let mut batch = LibraryBatch::new(
        uuid::Uuid::new_v4(),
        f.context.local_library_id,
        ChangeOrigin::Recovery,
        vec![],
    );
    batch.state.push(DurableStateChange::scoped(StateKey {namespace:StateNamespace::Bound {binding:f.context.verified_checkpoint.binding},target:StateTarget::Receipt {author,mutation_id:rejected}},json!({"schema":"echo.sync.receipt.v1","author":author,"mutation_id":rejected,"payload_digest":"b".repeat(64),"outcome":{"accepted":false},"sequence":null,"received_at":7})).unwrap());
    f.control.commit(f.context.root, &batch).unwrap();
    let report = retry(&f);
    assert_eq!(report.unresolved.len(), 1);
    assert_eq!(report.unresolved[0].reason, Reason::RequiresStateRecovery);
    f.db.with_reader(|c| {
        let count: u64 = c
            .query_row(
                "SELECT COUNT(*) FROM sync_inbox WHERE mutation_id=?1",
                [rejected.as_uuid().to_string()],
                |r| r.get(0),
            )
            .map_err(storage)?;
        assert_eq!(count, 0);
        Ok(())
    })
    .unwrap();
}

fn stage_and_publish_peer_playlist(
    source: &SqliteDatabase,
    binding: SyncBinding,
    playlist: PlaylistId,
) -> (
    DeviceId,
    MutationId,
    crate::domain::sync_binding::CommitCheckpoint,
) {
    let peer = DeviceId::new();
    source
        .authorize_sync_peer(
            binding,
            peer,
            "peer".into(),
            "test-only-secure-handle".into(),
        )
        .unwrap();
    let candidate = MutationCandidate {
        mutation_id: MutationId::new(),
        origin_device_id: peer,
        origin_seq: 1,
        previous_mutation_id: None,
        object: ObjectKey::Playlist(playlist),
        base_object_version: None,
        payload: json!({"playlist_uuid":playlist,"display_name":"C","members":[],"created_at":1000}),
        resource_refs: vec![],
    };
    let staged = source
        .stage_confirmation_at(binding, &candidate, Some(222))
        .unwrap();
    let checkpoint = staged.checkpoint.unwrap();
    source.mark_published(checkpoint).unwrap();
    (peer, candidate.mutation_id, checkpoint)
}

fn poison_target_inbox_row(f: &Fixture, peer: DeviceId, mutation: MutationId) {
    f.db.writer
        .run({
            let origin = peer.as_uuid().to_string();
            let mutation = mutation.as_uuid().to_string();
            move |c| {
                c.execute(
                    "INSERT INTO sync_inbox(origin_device_id,mutation_id,payload_digest,outcome_json,commit_seq,received_at) VALUES(?1,?2,?3,'{}',NULL,0)",
                    params![origin, mutation, "c".repeat(64)],
                )
                .map_err(storage)?;
                Ok(())
            }
        })
        .unwrap();
}

fn stray_receipt_fact(binding: SyncBinding, sequence: u64) -> DurableStateChange {
    let stranger = DeviceId::new();
    let stray = MutationId::new();
    DurableStateChange::scoped(
        StateKey {
            namespace: StateNamespace::Bound { binding },
            target: StateTarget::Receipt {
                author: stranger,
                mutation_id: stray,
            },
        },
        json!({
            "schema": "echo.sync.receipt.v1",
            "author": stranger.as_uuid(),
            "mutation_id": stray.as_uuid(),
            "payload_digest": "d".repeat(64),
            "outcome": {"accepted": true},
            "original_outcome": "{\"accepted\":true}",
            "received_at": 9,
            "sequence": sequence,
        }),
    )
    .unwrap()
}

#[test]
fn library_durability_trusted_sync_retry_restores_receipt_from_anchored_commit() {
    let mut f = fixture();
    let source = SqliteDatabase::open(f.directory.path().join("source.sqlite")).unwrap();
    let (peer, mutation, checkpoint) =
        stage_and_publish_peer_playlist(&source, f.context.verified_checkpoint.binding, f.playlist);
    let (digest, raw, sequence, received): (String, String, i64, i64) = source
        .with_reader(|c| {
            c.query_row(
                "SELECT payload_digest,outcome_json,commit_seq,received_at FROM sync_inbox WHERE origin_device_id=?1 AND mutation_id=?2",
                params![peer.as_uuid().to_string(), mutation.as_uuid().to_string()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .map_err(storage)
        })
        .unwrap();
    assert_eq!(
        sequence,
        i64::try_from(checkpoint.sequence.as_u64()).unwrap()
    );
    let mut batch = LibraryBatch::new(
        uuid::Uuid::new_v4(),
        f.context.local_library_id,
        ChangeOrigin::Recovery,
        vec![],
    );
    batch.state = source
        .with_reader(|c| {
            crate::infrastructure::sqlite::sync::durability::export_sync_state(c, f.context.root)
        })
        .unwrap();
    assert!(batch
        .state
        .iter()
        .any(|s| s.payload.get("schema") == Some(&json!("echo.sync.receipt.v2"))));
    batch.state.push(stray_receipt_fact(
        f.context.verified_checkpoint.binding,
        checkpoint.sequence.as_u64(),
    ));
    f.control.commit(f.context.root, &batch).unwrap();
    f.context.verified_checkpoint = checkpoint;
    // A conflicting retained inbox identity stays unresolved and is never overwritten.
    poison_target_inbox_row(&f, peer, mutation);
    let report = retry(&f);
    assert!(report
        .unresolved
        .iter()
        .any(|r| r.reason == Reason::IdentityConflict));
    assert!(report
        .unresolved
        .iter()
        .any(|r| r.reason == Reason::InvalidStateFact));
    f.db.with_reader(|c| {
        let kept: String = c
            .query_row(
                "SELECT payload_digest FROM sync_inbox WHERE origin_device_id=?1 AND mutation_id=?2",
                params![peer.as_uuid().to_string(), mutation.as_uuid().to_string()],
                |r| r.get(0),
            )
            .map_err(storage)?;
        assert_eq!(kept, "c".repeat(64));
        Ok(())
    })
    .unwrap();
    // Dropping the poisoned row lets the same retained fact restore the receipt.
    f.db.writer
        .run({
            let origin = peer.as_uuid().to_string();
            let mutation = mutation.as_uuid().to_string();
            move |c| {
                c.execute(
                    "DELETE FROM sync_inbox WHERE origin_device_id=?1 AND mutation_id=?2",
                    params![origin, mutation],
                )
                .map_err(storage)?;
                Ok(())
            }
        })
        .unwrap();
    let report = retry(&f);
    assert!(report
        .unresolved
        .iter()
        .all(|r| r.reason != Reason::IdentityConflict));
    f.db.with_reader(|c| {
        let restored: (String, String, i64, i64) = c
            .query_row(
                "SELECT payload_digest,outcome_json,commit_seq,received_at FROM sync_inbox WHERE origin_device_id=?1 AND mutation_id=?2",
                params![peer.as_uuid().to_string(), mutation.as_uuid().to_string()],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .map_err(storage)?;
        assert_eq!(restored, (digest, raw.clone(), sequence, received));
        let staged: crate::application::ports::StagedConfirmation =
            serde_json::from_str(&raw).unwrap();
        assert_eq!(staged.checkpoint, Some(checkpoint));
        Ok(())
    })
    .unwrap();
    let again = retry(&f);
    assert_eq!(again.unresolved.len(), report.unresolved.len());
}

#[test]
fn library_durability_trusted_sync_retry_keeps_resolved_conflict_idempotent() {
    let mut f = fixture();
    let source = SqliteDatabase::open(f.directory.path().join("source.sqlite")).unwrap();
    let (_peer, _mutation, checkpoint) =
        stage_and_publish_peer_playlist(&source, f.context.verified_checkpoint.binding, f.playlist);
    let mut batch = LibraryBatch::new(
        uuid::Uuid::new_v4(),
        f.context.local_library_id,
        ChangeOrigin::Recovery,
        vec![],
    );
    batch.state = source
        .with_reader(|c| {
            crate::infrastructure::sqlite::sync::durability::export_sync_state(c, f.context.root)
        })
        .unwrap();
    f.control.commit(f.context.root, &batch).unwrap();
    f.context.verified_checkpoint = checkpoint;
    retry(&f);
    // A retained conflict whose row is already resolved replays idempotently
    // and never rewrites the stored resolution.
    f.db.writer
        .run(|c| {
            c.execute("UPDATE sync_conflicts SET resolved_at=5", [])
                .map_err(storage)?;
            Ok(())
        })
        .unwrap();
    let report = retry(&f);
    assert!(report
        .unresolved
        .iter()
        .all(|r| r.reason != Reason::IdentityConflict));
    f.db.with_reader(|c| {
        let resolved: Option<i64> = c
            .query_row("SELECT resolved_at FROM sync_conflicts", [], |r| r.get(0))
            .map_err(storage)?;
        assert_eq!(resolved, Some(5));
        Ok(())
    })
    .unwrap();
}

#[test]
fn library_durability_trusted_sync_retry_restores_conflict_only_with_both_anchored_versions() {
    let mut f = fixture();
    let source = SqliteDatabase::open(f.directory.path().join("source.sqlite")).unwrap();
    let peer = DeviceId::new();
    source
        .authorize_sync_peer(
            f.context.verified_checkpoint.binding,
            peer,
            "peer".into(),
            "test-only-secure-handle".into(),
        )
        .unwrap();
    let candidate = MutationCandidate {
        mutation_id: MutationId::new(),
        origin_device_id: peer,
        origin_seq: 1,
        previous_mutation_id: None,
        object: ObjectKey::Playlist(f.playlist),
        base_object_version: None,
        payload: json!({"playlist_uuid":f.playlist,"display_name":"C","members":[],"created_at":1000}),
        resource_refs: vec![],
    };
    let checkpoint = source
        .stage_confirmation_at(f.context.verified_checkpoint.binding, &candidate, Some(222))
        .unwrap()
        .checkpoint
        .unwrap();
    source.mark_published(checkpoint).unwrap();
    let mut batch = LibraryBatch::new(
        uuid::Uuid::new_v4(),
        f.context.local_library_id,
        ChangeOrigin::Recovery,
        vec![],
    );
    batch.state = source
        .with_reader(|c| {
            crate::infrastructure::sqlite::sync::durability::export_sync_state(c, f.context.root)
        })
        .unwrap();
    assert!(batch
        .state
        .iter()
        .any(|s| s.kind == crate::domain::library::durability::StateKind::Conflict));
    f.control.commit(f.context.root, &batch).unwrap();
    f.context.verified_checkpoint = checkpoint;
    let report = retry(&f);
    assert!(
        report
            .unresolved
            .iter()
            .any(|r| r.reason == Reason::MissingSyncDependency),
        "pending B references old baseline A; it remains explicit"
    );
    f.db.with_reader(|c|{let values:(u64,String,String)=c.query_row("SELECT accepted_version,replaced_payload_json,accepted_payload_json FROM sync_conflicts",[],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(storage)?;assert_eq!(values.0,2);assert_eq!(serde_json::from_str::<Value>(&values.1).unwrap()["display_name"],"A");assert_eq!(serde_json::from_str::<Value>(&values.2).unwrap()["display_name"],"C");Ok(())}).unwrap();
    // 同一保留事实重试：既有 conflict 行被篡改后保持 IdentityConflict，不覆盖。
    f.db.writer
        .run(move |c| {
            c.execute(
                "UPDATE sync_conflicts SET accepted_payload_json='{\"display_name\":\"tampered\"}'",
                [],
            )
            .map_err(storage)?;
            Ok(())
        })
        .unwrap();
    let tampered = retry(&f);
    assert!(tampered
        .unresolved
        .iter()
        .any(|r| r.reason == Reason::IdentityConflict));
    f.db.with_reader(|c| {
        let kept: String = c
            .query_row(
                "SELECT accepted_payload_json FROM sync_conflicts",
                [],
                |r| r.get(0),
            )
            .map_err(storage)?;
        assert_eq!(
            serde_json::from_str::<Value>(&kept).unwrap()["display_name"],
            "tampered"
        );
        Ok(())
    })
    .unwrap();
}

#[test]
fn library_durability_trusted_sync_retry_rejects_conflict_without_schema_or_anchored_versions() {
    let f = fixture();
    let binding = f.context.verified_checkpoint.binding;
    let mut batch = LibraryBatch::new(
        uuid::Uuid::new_v4(),
        f.context.local_library_id,
        ChangeOrigin::Recovery,
        vec![],
    );
    let unproven = uuid::Uuid::new_v4();
    batch
        .state
        .push(DurableStateChange::scoped(StateKey {namespace:StateNamespace::Bound {binding},target:StateTarget::Conflict {conflict_id:unproven}},json!({"schema":"echo.sync.conflict.v2","id":unproven.to_string(),"object_type":"playlist","object_uuid":f.playlist.to_string(),"replaced_version":1,"accepted_version":2,"replaced":{"display_name":"A"},"accepted":{"display_name":"C"},"resolved_at":null})).unwrap());
    let missing = uuid::Uuid::new_v4();
    batch
        .state
        .push(DurableStateChange::scoped(StateKey {namespace:StateNamespace::Bound {binding},target:StateTarget::Conflict {conflict_id:missing}},json!({"schema":"echo.sync.conflict.v1","id":missing.to_string(),"object_type":"playlist","object_uuid":f.playlist.to_string(),"replaced_version":98,"accepted_version":99,"replaced":{"display_name":"A"},"accepted":{"display_name":"C"},"resolved_at":null})).unwrap());
    f.control.commit(f.context.root, &batch).unwrap();
    let report = retry(&f);
    let reasons: Vec<_> = report.unresolved.iter().map(|r| r.reason).collect();
    assert_eq!(reasons.len(), 2);
    assert!(reasons.contains(&Reason::InvalidStateFact));
    assert!(reasons.contains(&Reason::MissingSyncDependency));
    f.db.with_reader(|c| {
        let conflicts: u64 = c
            .query_row("SELECT COUNT(*) FROM sync_conflicts", [], |r| r.get(0))
            .map_err(storage)?;
        assert_eq!(conflicts, 0);
        Ok(())
    })
    .unwrap();
}

#[test]
fn library_durability_trusted_sync_retry_retains_resource_candidate_until_upload_dependencies_are_proven(
) {
    use crate::domain::sync::{ResourceDigest, SharedResourceKind, SharedResourceManifest};
    let f = fixture();
    let mut with_cover = f.frozen.clone();
    with_cover.candidate.mutation_id = MutationId::new();
    with_cover.candidate.origin_seq = 3;
    with_cover.candidate.previous_mutation_id = Some(f.frozen.candidate.mutation_id);
    with_cover.local_revision = Revision::from_u64(3);
    with_cover.candidate.resource_refs = vec![SharedResourceManifest::new(
        SharedResourceKind::PlaylistCover,
        ResourceDigest::from_bytes([3; 32]),
        12,
        "image/png".into(),
        "covers/cover.png",
    )
    .unwrap()];
    with_cover.original_resources_body =
        serde_json::to_string_pretty(&with_cover.candidate.resource_refs).unwrap();
    let mut batch = LibraryBatch::new(
        uuid::Uuid::new_v4(),
        f.context.local_library_id,
        ChangeOrigin::Recovery,
        vec![],
    );
    batch.state = frozen_states(
        StateNamespace::Bound {
            binding: f.context.verified_checkpoint.binding,
        },
        &with_cover,
        MutationStatus::Pending,
    );
    f.control.commit(f.context.root, &batch).unwrap();
    let report = retry(&f);
    assert_eq!(
        counts(&f),
        (1, 0, 1),
        "unverified cover event never enters the sendable queue"
    );
    assert_eq!(
        report
            .unresolved
            .iter()
            .filter(|r| r.reason == Reason::RequiresStateRecovery)
            .count(),
        2
    );
    assert!(f
        .db
        .read_recovery_records(f.context.root)
        .unwrap()
        .state
        .iter()
        .any(|s| s.payload.get("frozen") == Some(&serde_json::to_value(&with_cover).unwrap())));
    assert_eq!(retry(&f), report);
}

#[test]
fn library_durability_confirmed_preserves_pending_resources_without_consumable_frozen_pair() {
    use crate::domain::sync::{ResourceDigest, SharedResourceKind, SharedResourceManifest};
    for missing in ["frozen", "disposition", "exact_version"] {
        let f = fixture();
        retry(&f);
        let resources = vec![SharedResourceManifest::new(
            SharedResourceKind::PlaylistCover,
            ResourceDigest::from_bytes([4; 32]),
            13,
            "image/png".into(),
            "covers/local.png",
        )
        .unwrap()];
        f.db.set_local_sync_resources(ObjectKey::Playlist(f.playlist), resources.clone())
            .unwrap();
        let mutation = f.frozen.candidate.mutation_id;
        let raw = serde_json::to_string(&resources).unwrap();
        f.db.writer.run(move |c| {
            c.execute("UPDATE sync_mutations SET resource_refs_json=?2 WHERE mutation_id=?1", params![mutation.as_uuid().to_string(),raw]).map_err(storage)?;
            c.execute("DELETE FROM sync_object_versions", []).map_err(storage)?;
            match missing {
                "frozen" => c.execute("DELETE FROM library_recovery_records WHERE row_kind='state' AND json_extract(envelope_json,'$.kind')='pending'", []),
                "disposition" => c.execute("DELETE FROM library_recovery_records WHERE row_kind='state' AND json_extract(envelope_json,'$.kind')='pending_disposition'", []),
                _ => Ok(0),
            }.map_err(storage)?;
            Ok(())
        }).unwrap();
        for _ in 0..2 {
            // Retry only retained facts: a directory catch-up would restore the
            // deliberately missing half of the frozen/disposition pair.
            let report = f.db.retry_library_sync_state(f.context).unwrap();
            assert!(!report.unresolved.is_empty(), "{missing}");
            f.db.with_reader(|c| {
                assert_eq!(crate::infrastructure::sqlite::sync::manifests::load(c, "playlist", &f.playlist.to_string())?, resources, "{missing}");
                let refs: u64 = c.query_row("SELECT COUNT(*) FROM sync_resource_refs WHERE object_type='playlist' AND object_uuid=?1", [f.playlist.to_string()], |r| r.get(0)).map_err(storage)?;
                assert_eq!(refs, 1, "{missing}");
                Ok(())
            }).unwrap();
        }
    }
}

#[test]
fn library_durability_terminal_pending_restores_confirmed_resources_atomically() {
    use crate::domain::sync::{ResourceDigest, SharedResourceKind, SharedResourceManifest};
    for (status, remaining) in [
        (MutationStatus::Confirmed, false),
        (MutationStatus::Rejected, false),
        (MutationStatus::Rejected, true),
    ] {
        let f = fixture();
        retry(&f);
        let mut frozen = f.frozen.clone();
        frozen.candidate.resource_refs = vec![SharedResourceManifest::new(
            SharedResourceKind::PlaylistCover,
            ResourceDigest::from_bytes([5; 32]),
            14,
            "image/png".into(),
            "covers/pending.png",
        )
        .unwrap()];
        frozen.original_resources_body =
            serde_json::to_string(&frozen.candidate.resource_refs).unwrap();
        f.db.set_local_sync_resources(
            frozen.candidate.object,
            frozen.candidate.resource_refs.clone(),
        )
        .unwrap();
        let raw = frozen.original_resources_body.clone();
        let mutation = frozen.candidate.mutation_id;
        let root = f.context.root;
        let states = frozen_states(
            StateNamespace::Bound {
                binding: f.context.verified_checkpoint.binding,
            },
            &frozen,
            status,
        );
        f.db.writer.run(move |c| {
            c.execute("UPDATE sync_mutations SET resource_refs_json=?2 WHERE mutation_id=?1",params![mutation.as_uuid().to_string(),raw]).map_err(storage)?;
            c.execute("DELETE FROM library_recovery_records WHERE row_kind='state' AND json_extract(envelope_json,'$.kind') IN ('pending','pending_disposition')",[]).map_err(storage)?;
            retained::save(c,root,&[],&states,&[])?;
            if remaining {
                c.execute("INSERT INTO sync_mutations(mutation_id,origin_device_id,origin_seq,previous_mutation_id,object_type,object_uuid,operation,local_revision,base_object_version,payload_json,payload_digest,status,outcome_json,created_at,resource_refs_json,local_root_uuid) SELECT ?2,origin_device_id,origin_seq+1,mutation_id,object_type,object_uuid,operation,local_revision+1,base_object_version,payload_json,payload_digest,'pending',NULL,created_at,resource_refs_json,local_root_uuid FROM sync_mutations WHERE mutation_id=?1",params![mutation.as_uuid().to_string(),MutationId::new().as_uuid().to_string()]).map_err(storage)?;
            }
            c.execute_batch("CREATE TRIGGER fail_resource_fallback BEFORE DELETE ON sync_resource_refs BEGIN SELECT RAISE(ABORT,'injected resource fallback failure'); END;").map_err(storage)
        }).unwrap();
        assert!(f.db.retry_library_sync_state(f.context).is_err());
        f.db.with_reader(|c| {
            let queue_status: String = c
                .query_row(
                    "SELECT status FROM sync_mutations WHERE mutation_id=?1",
                    [mutation.as_uuid().to_string()],
                    |r| r.get(0),
                )
                .map_err(storage)?;
            assert_eq!(queue_status, "pending");
            assert_eq!(
                crate::infrastructure::sqlite::sync::manifests::load(
                    c,
                    "playlist",
                    &f.playlist.to_string()
                )?,
                frozen.candidate.resource_refs
            );
            Ok(())
        })
        .unwrap();
        f.db.writer
            .run(|c| {
                c.execute_batch("DROP TRIGGER fail_resource_fallback")
                    .map_err(storage)
            })
            .unwrap();
        for _ in 0..2 {
            assert!(f
                .db
                .retry_library_sync_state(f.context)
                .unwrap()
                .unresolved
                .is_empty());
            f.db.with_reader(|c| {
                let actual = crate::infrastructure::sqlite::sync::manifests::load(c,"playlist",&f.playlist.to_string())?;
                if remaining {
                    assert_eq!(actual, frozen.candidate.resource_refs);
                } else {
                    assert!(actual.is_empty());
                }
                let refs: u64 = c.query_row("SELECT COUNT(*) FROM sync_resource_refs WHERE object_type='playlist' AND object_uuid=?1",[f.playlist.to_string()],|r|r.get(0)).map_err(storage)?;
                assert_eq!(refs,u64::from(remaining));
                Ok(())
            }).unwrap();
            assert_eq!(
                f.db.pending_local_mutations(10).unwrap().len(),
                usize::from(remaining)
            );
        }
    }
}

#[test]
fn library_durability_confirmed_resources_ignore_pending_from_another_root() {
    use crate::domain::sync::{ResourceDigest, SharedResourceKind, SharedResourceManifest};
    let f = fixture();
    retry(&f);
    f.db.set_local_sync_resources(
        ObjectKey::Playlist(f.playlist),
        vec![SharedResourceManifest::new(
            SharedResourceKind::PlaylistCover,
            ResourceDigest::from_bytes([6; 32]),
            15,
            "image/png".into(),
            "covers/other-root.png",
        )
        .unwrap()],
    )
    .unwrap();
    let other = LibraryRootId::new();
    f.db.writer
        .run(move |c| {
            c.execute(
                "UPDATE sync_mutations SET local_root_uuid=?1",
                [other.to_string()],
            )
            .map_err(storage)?;
            c.execute("DELETE FROM sync_object_versions", [])
                .map_err(storage)?;
            Ok(())
        })
        .unwrap();
    for _ in 0..2 {
        f.db.retry_library_sync_state(f.context).unwrap();
        f.db.with_reader(|c| {
            assert!(crate::infrastructure::sqlite::sync::manifests::load(
                c,
                "playlist",
                &f.playlist.to_string()
            )?
            .is_empty());
            Ok(())
        })
        .unwrap();
    }
}

#[test]
fn library_durability_trusted_sync_retry_keeps_existing_pending_resources_effective() {
    use crate::domain::sync::{ResourceDigest, SharedResourceKind, SharedResourceManifest};
    let f = fixture();
    retry(&f);
    let mut with_cover = f.frozen.clone();
    with_cover.candidate.resource_refs = vec![SharedResourceManifest::new(
        SharedResourceKind::PlaylistCover,
        ResourceDigest::from_bytes([4; 32]),
        13,
        "image/png".into(),
        "covers/local.png",
    )
    .unwrap()];
    with_cover.original_resources_body =
        serde_json::to_string_pretty(&with_cover.candidate.resource_refs).unwrap();
    // An existing exact queue is already current-session local evidence. Model
    // its real resource capture; directory retry must not replace it with A's
    // empty confirmed declaration while retaining B as the effective value.
    let resources = with_cover.original_resources_body.clone();
    let mutation = f.frozen.candidate.mutation_id;
    f.db.writer
        .run(move |c| {
            c.execute(
                "UPDATE sync_mutations SET resource_refs_json=?2 WHERE mutation_id=?1",
                params![mutation.as_uuid().to_string(), resources],
            )
            .map_err(storage)?;
            Ok(())
        })
        .unwrap();
    let mut batch = LibraryBatch::new(
        uuid::Uuid::new_v4(),
        f.context.local_library_id,
        ChangeOrigin::Recovery,
        vec![],
    );
    batch.state = frozen_states(
        StateNamespace::Bound {
            binding: f.context.verified_checkpoint.binding,
        },
        &with_cover,
        MutationStatus::Pending,
    );
    f.control.commit(f.context.root, &batch).unwrap();
    retry(&f);
    f.db.with_reader(|c| {
        let actual = crate::infrastructure::sqlite::sync::manifests::load(
            c,
            "playlist",
            &f.playlist.to_string(),
        )?;
        assert_eq!(actual, with_cover.candidate.resource_refs);
        Ok(())
    })
    .unwrap();
    assert_eq!(
        f.db.pending_local_mutations(10).unwrap(),
        vec![with_cover.candidate]
    );
}

/// Publish two same-hash song commits on the fixture source, returning the
/// alias commit's checkpoint. The real publication path enriches the second
/// commit with the typed song alias proof and writes `sync_song_aliases`.
fn publish_same_hash_song_alias(
    source: &SqliteDatabase,
    binding: SyncBinding,
    author: DeviceId,
) -> (
    crate::domain::sync::CommitCheckpoint,
    crate::domain::ids::SongId,
    crate::domain::ids::SongId,
) {
    let last: Option<String> = source
        .with_reader(|c| {
            c.query_row(
                "SELECT last_mutation_id FROM sync_origin_cursors WHERE origin_device_id=?1",
                [author.as_uuid().to_string()],
                |r| r.get(0),
            )
            .optional()
            .map_err(storage)
        })
        .unwrap();
    let Some(last) = last else {
        panic!("fixture publishes the playlist commit first");
    };
    let mut previous = Some(MutationId::from_uuid(uuid::Uuid::parse_str(&last).unwrap()));
    let canonical = crate::domain::ids::SongId::new();
    let alias = crate::domain::ids::SongId::new();
    let mut anchor = None;
    for (index, id) in [canonical, alias].into_iter().enumerate() {
        let candidate = MutationCandidate {
            mutation_id: MutationId::new(),
            origin_device_id: author,
            origin_seq: 2 + index as u64,
            previous_mutation_id: previous,
            object: ObjectKey::Song(id),
            base_object_version: None,
            payload: json!({"song_uuid":id,"relative_path":"media/alias.flac","blake3_hash":"4".repeat(64),"title":"same bytes","added_at":42}),
            resource_refs: vec![],
        };
        let staged = source
            .stage_confirmation_at(binding, &candidate, Some(200 + index as u64))
            .unwrap();
        let Some(checkpoint) = staged.checkpoint else {
            panic!("staging rejected: {}", staged.outcome_json);
        };
        source.mark_published(checkpoint).unwrap();
        anchor = Some(checkpoint);
        previous = Some(candidate.mutation_id);
    }
    let checkpoint = anchor.expect("both songs publish");
    (checkpoint, canonical, alias)
}

#[test]
fn library_durability_trusted_sync_retry_restores_song_alias_after_canonical_record_arrives() {
    let mut f = fixture();
    let source = SqliteDatabase::open(f.directory.path().join("source.sqlite")).unwrap();
    let binding = f.context.verified_checkpoint.binding;
    let author = f.context.local_author;
    let (anchor, canonical, alias) = publish_same_hash_song_alias(&source, binding, author);
    // The live session owner has independently verified the newer ledger head;
    // the recovery context carries that anchor instead of the fixture default.
    f.context.verified_checkpoint = anchor;
    // The real exporter emits the alias fact and the enriched ledger commits;
    // nothing here is fabricated by hand.
    let state = source
        .with_reader(|c| {
            crate::infrastructure::sqlite::sync::durability::export_sync_state(c, f.context.root)
        })
        .unwrap();
    assert!(state
        .iter()
        .any(|s| s.payload["schema"] == "echo.sync.song-alias.v1"));
    let mut batch = LibraryBatch::new(
        uuid::Uuid::new_v4(),
        f.context.local_library_id,
        ChangeOrigin::Recovery,
        vec![],
    );
    batch.state = state;
    f.control.commit(f.context.root, &batch).unwrap();
    // Before the canonical song record is projected, the alias keeps its
    // unresolved evidence instead of inventing an owner.
    let report = retry(&f);
    let alias_reasons: Vec<_> = report
        .unresolved
        .iter()
        .filter(|r| {
            matches!(
                r.target,
                crate::domain::library::durability::ProjectionTarget::ScopedState { ref key }
                    if matches!(key.target, StateTarget::SongAlias { .. })
            )
        })
        .map(|r| r.reason)
        .collect();
    assert_eq!(alias_reasons, vec![Reason::MissingSong]);
    f.db.with_reader(|c| {
        let count: u64 = c
            .query_row("SELECT COUNT(*) FROM sync_song_aliases", [], |r| r.get(0))
            .map_err(storage)?;
        assert_eq!(count, 0);
        Ok(())
    })
    .unwrap();
    // The same retained fact becomes projectable once the canonical record
    // arrives; retrying must not need a fresh directory export.
    let records = LibraryBatch::new(
        uuid::Uuid::new_v4(),
        f.context.local_library_id,
        ChangeOrigin::Recovery,
        vec![crate::domain::library::PortableRecordEnvelope::new(
            PortableRecord::Song(crate::domain::library::SongRecord {
                song_uuid: canonical.as_uuid(),
                revision: Revision::INITIAL,
                updated_by_device_id: crate::domain::library::DeviceId::from_uuid(author.as_uuid()),
                hlc: crate::domain::library::HybridLogicalClock::new(1_700_000_000, 0),
                media_path: crate::domain::library::LibraryRelativePath::new("media/alias.flac")
                    .unwrap(),
                content_hash: "4".repeat(64),
                added_at: 42,
                title: Some("same bytes".to_owned()),
                artist: None,
                album: None,
            }),
            None,
        )
        .unwrap()],
    );
    f.control.commit(f.context.root, &records).unwrap();
    let report = retry(&f);
    assert!(report.unresolved.iter().all(|r| {
        !matches!(
            r.target,
            crate::domain::library::durability::ProjectionTarget::ScopedState { ref key }
                if matches!(key.target, StateTarget::SongAlias { .. })
        )
    }));
    f.db.with_reader(|c| {
        let count: u64 = c
            .query_row(
                "SELECT COUNT(*) FROM sync_song_aliases WHERE alias_song_uuid=?1 AND canonical_song_uuid=?2",
                params![alias.as_uuid().to_string(), canonical.as_uuid().to_string()],
                |r| r.get(0),
            )
            .map_err(storage)?;
        assert_eq!(count, 1);
        Ok(())
    })
    .unwrap();
}

#[test]
fn library_durability_trusted_sync_retry_rejects_member_alias_without_anchored_alias_proof() {
    let f = fixture();
    let playlist = f.playlist;
    let missing_alias = crate::domain::ids::PlaylistItemId::new();
    let unproven_alias = crate::domain::ids::PlaylistItemId::new();
    let missing_sequence = DurableStateChange::scoped(
        StateKey {
            namespace: StateNamespace::Bound {
                binding: f.context.verified_checkpoint.binding,
            },
            target: StateTarget::MemberAlias {
                playlist,
                alias: missing_alias,
            },
        },
        json!({"schema":"echo.sync.member-alias.v1","playlist":playlist,"alias":missing_alias,"canonical":uuid::Uuid::new_v4(),"sequence":99}),
    )
    .unwrap();
    // Sequence 1 is the real playlist commit, but its canonical JSON carries no
    // playlist_member alias entry, so the anchored ledger disproves the fact.
    let unproven_sequence = DurableStateChange::scoped(
        StateKey {
            namespace: StateNamespace::Bound {
                binding: f.context.verified_checkpoint.binding,
            },
            target: StateTarget::MemberAlias {
                playlist,
                alias: unproven_alias,
            },
        },
        json!({"schema":"echo.sync.member-alias.v1","playlist":playlist,"alias":unproven_alias,"canonical":uuid::Uuid::new_v4(),"sequence":1}),
    )
    .unwrap();
    let mut batch = LibraryBatch::new(
        uuid::Uuid::new_v4(),
        f.context.local_library_id,
        ChangeOrigin::Recovery,
        vec![],
    );
    batch.state = vec![missing_sequence, unproven_sequence];
    f.control.commit(f.context.root, &batch).unwrap();
    let report = retry(&f);
    let reasons: Vec<_> = report
        .unresolved
        .iter()
        .filter(|r| {
            matches!(
                r.target,
                crate::domain::library::durability::ProjectionTarget::ScopedState { ref key }
                    if matches!(key.target, StateTarget::MemberAlias { .. })
            )
        })
        .map(|r| r.reason)
        .collect();
    assert_eq!(reasons.len(), 2);
    assert!(reasons.contains(&Reason::InvalidStateFact));
    assert!(reasons.contains(&Reason::MissingSyncDependency));
    f.db.with_reader(|c| {
        let count: u64 = c
            .query_row("SELECT COUNT(*) FROM sync_member_aliases", [], |r| r.get(0))
            .map_err(storage)?;
        assert_eq!(count, 0);
        Ok(())
    })
    .unwrap();
}
