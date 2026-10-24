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
