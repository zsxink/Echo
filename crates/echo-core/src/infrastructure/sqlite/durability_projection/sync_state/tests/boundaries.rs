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
