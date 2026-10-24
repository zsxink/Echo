//! Real SQLite and native directory retry; no transport or credential fabrication.
use super::*;
use crate::application::library_recovery::RetryLibraryOperation;
use crate::application::ports::{
    ControlPlanePort, DeviceIdProvider, HostCommitLedgerPort, LibraryDurabilityPort,
    LibraryProjectionPort, LibraryRepository, LocalSyncQueuePort, PlaylistRepository,
};
use crate::domain::{
    entities::LibraryRoot,
    ids::{LibraryRootId, PlaylistId, Revision},
    library::{
        durability::{
            ChangeOrigin, FrozenMutation, LibraryBatch, MutationDisposition, MutationOperation,
            MutationStatus, StateKey,
        },
        LibraryId, LibraryManifest, PortableRecord, PortableSerialize,
    },
    sync::{
        AuthorityId, CommitSeq, DeviceId, Epoch, LibraryId as SyncLibraryId, MutationCandidate,
        MutationId, ObjectKey, SyncBinding,
    },
};
use crate::infrastructure::filesystem::{RootControlPlane, RootRegistry};
use serde_json::json;
use std::sync::Arc;

struct Fixture {
    directory: tempfile::TempDir,
    db: SqliteDatabase,
    control: Arc<RootControlPlane>,
    context: SyncRecoveryContext,
    batch: LibraryBatch,
    frozen: FrozenMutation,
    playlist: PlaylistId,
}
fn frozen_states(
    namespace: StateNamespace,
    frozen: &FrozenMutation,
    status: MutationStatus,
) -> Vec<DurableStateChange> {
    let author = frozen.candidate.origin_device_id;
    let mutation_id = frozen.candidate.mutation_id;
    let digest = frozen.content_hash().unwrap();
    vec![DurableStateChange::scoped(StateKey {namespace:namespace.clone(),target:StateTarget::Mutation {author,mutation_id,body_digest:digest.clone()}},json!({"schema":"echo.sync.frozen-mutation.v2","frozen":frozen})).unwrap(),DurableStateChange::scoped(StateKey {namespace,target:StateTarget::MutationDisposition {author,mutation_id,body_digest:digest.clone()}},json!({"schema":"echo.sync.mutation-disposition.v2","disposition":MutationDisposition {author,mutation_id,frozen_body_digest:digest,status,outcome:None}})).unwrap()]
}
fn fixture() -> Fixture {
    fixture_with_parent(true)
}
fn fixture_with_parent(include_parent: bool) -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let root = LibraryRootId::new();
    let local_library_id = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let binding = SyncBinding {
        library_id: SyncLibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    let source = SqliteDatabase::open(directory.path().join("source.sqlite")).unwrap();
    let path = directory.path().join("library");
    std::fs::create_dir(&path).unwrap();
    LibraryRepository::upsert(&source, &LibraryRoot::new(root, path.clone(), true, true)).unwrap();
    source.bind_sync_library(binding, true, root).unwrap();
    let (batch, frozen, playlist, author, checkpoint) =
        source_batch(&source, root, local_library_id, binding);
    let registry = RootRegistry::new();
    registry.register(root, &path);
    let control = Arc::new(RootControlPlane::new(registry));
    control
        .write_manifest(root, &LibraryManifest::new(local_library_id, "sync retry"))
        .unwrap();
    let mut committed = batch.clone();
    if !include_parent {
        committed.records.clear();
    }
    control.commit(root, &committed).unwrap();
    let db = SqliteDatabase::open(directory.path().join("target.sqlite")).unwrap();
    LibraryRepository::upsert(&db, &LibraryRoot::new(root, path, true, true)).unwrap();
    db.configure_library_control(control.clone());
    // Matching device credentials are supplied by a live session owner. They
    // are never read from or inserted by the directory recovery operation.
    db.writer
        .run(move |c| {
            c.execute(
                "UPDATE device_state SET value=?1 WHERE key='device_id'",
                [author.as_uuid().to_string()],
            )
            .map_err(storage)?;
            Ok(())
        })
        .unwrap();
    db.bind_sync_library(binding, true, root).unwrap();
    Fixture {
        directory,
        db,
        control,
        context: SyncRecoveryContext {
            root,
            local_library_id,
            local_author: author,
            role: RecoverySessionRole::Host,
            verified_checkpoint: checkpoint,
        },
        batch,
        frozen,
        playlist,
    }
}
fn source_batch(
    source: &SqliteDatabase,
    root: LibraryRootId,
    local_library_id: LibraryId,
    binding: SyncBinding,
) -> (
    LibraryBatch,
    FrozenMutation,
    PlaylistId,
    DeviceId,
    crate::domain::sync::CommitCheckpoint,
) {
    let author = DeviceId::from_uuid(source.current_device_id().as_uuid());
    let playlist = PlaylistId::new();
    let candidate = MutationCandidate {
        mutation_id: MutationId::new(),
        origin_device_id: author,
        origin_seq: 1,
        previous_mutation_id: None,
        object: ObjectKey::Playlist(playlist),
        base_object_version: None,
        payload: json!({"playlist_uuid":playlist,"display_name":"A","members":[],"created_at":1000}),
        resource_refs: vec![],
    };
    let staged = source
        .stage_confirmation_at(binding, &candidate, Some(111))
        .unwrap();
    let checkpoint = staged.checkpoint.unwrap();
    source.mark_published(checkpoint).unwrap();
    let mut records = source
        .with_reader(|c| {
            crate::infrastructure::sqlite::durability_backfill::export_library_records(c, root)
        })
        .unwrap();
    for envelope in &mut records {
        if let PortableRecord::Playlist(record) = envelope.record() {
            let mut record = record.clone();
            record.display_name = "B".into();
            *envelope = crate::domain::library::PortableRecordEnvelope::new(
                PortableRecord::Playlist(record),
                None,
            )
            .unwrap();
        }
    }
    let mut state = source
        .with_reader(|c| {
            crate::infrastructure::sqlite::sync::durability::export_sync_state(c, root)
        })
        .unwrap();
    let raw = format!(
        "{{ \"playlist_uuid\" : \"{playlist}\", \"display_name\" : \"B\", \"members\" : [] }}"
    );
    let frozen = FrozenMutation {
        candidate: MutationCandidate {
            mutation_id: MutationId::new(),
            origin_device_id: author,
            origin_seq: 2,
            previous_mutation_id: Some(candidate.mutation_id),
            object: candidate.object,
            base_object_version: Some(crate::domain::sync::ObjectVersion::from_commit_seq(
                CommitSeq::from_u64(1),
            )),
            payload: serde_json::from_str(&raw).unwrap(),
            resource_refs: vec![],
        },
        operation: MutationOperation::Upsert,
        local_revision: Revision::from_u64(2),
        frozen_at_ms: 123_456,
        original_body: raw,
        original_resources_body: "[ ]".into(),
        payload_digest: "a".repeat(64),
    };
    // This models a future collector capturing the actual freeze-time scope;
    // the production historical exporter deliberately does not make this claim.
    state.extend(frozen_states(
        StateNamespace::Bound { binding },
        &frozen,
        MutationStatus::Pending,
    ));
    let mut batch = LibraryBatch::new(
        uuid::Uuid::new_v4(),
        local_library_id,
        ChangeOrigin::Recovery,
        records,
    );
    batch.state = state;
    (batch, frozen, playlist, author, checkpoint)
}

fn counts(f: &Fixture) -> (u64, u64, u64) {
    f.db.with_reader(|c| {
        Ok((
            c.query_row("SELECT COUNT(*) FROM sync_mutations", [], |r| r.get(0))
                .map_err(storage)?,
            c.query_row("SELECT COUNT(*) FROM sync_outbox", [], |r| r.get(0))
                .map_err(storage)?,
            c.query_row("SELECT COUNT(*) FROM sync_commits", [], |r| r.get(0))
                .map_err(storage)?,
        ))
    })
    .unwrap()
}
fn retry(f: &Fixture) -> LibraryProjectionStatus {
    RetryLibraryOperation::new(f.control.as_ref(), &f.db)
        .catch_up_sync(f.context, &f.db)
        .unwrap()
}
/// The directory must carry exactly one complete receipt, matching the row
/// SQL still holds after projection.
fn complete_receipt(state: &[DurableStateChange]) -> &DurableStateChange {
    let receipts: Vec<_> = state
        .iter()
        .filter(|state| {
            matches!(
                state.key.as_ref().map(|key| &key.target),
                Some(StateTarget::Receipt { .. })
            )
        })
        .collect();
    assert_eq!(receipts.len(), 1);
    assert_eq!(receipts[0].payload["schema"], "echo.sync.receipt.v2");
    receipts[0].validate().unwrap();
    receipts[0]
}

#[test]
fn library_durability_trusted_sync_retry_restores_exact_baseline_and_pending_without_echo() {
    let f = fixture();
    let confirmed = f
        .batch
        .state
        .iter()
        .find(|state| {
            matches!(
                state.key.as_ref().map(|key| &key.target),
                Some(StateTarget::Confirmed { .. })
            )
        })
        .unwrap();
    assert_eq!(confirmed.payload["schema"], "echo.sync.confirmed.v2");
    confirmed.validate().unwrap();
    let exported_receipt = complete_receipt(&f.batch.state);
    let retry_api = RetryLibraryOperation::new(f.control.as_ref(), &f.db);
    retry_api.catch_up(f.context.root).unwrap();
    assert_eq!(
        counts(&f),
        (0, 0, 0),
        "ordinary directory recovery is offline"
    );
    assert!(!f
        .db
        .projection_status(f.context.root)
        .unwrap()
        .unresolved
        .is_empty());
    let report = retry(&f);
    assert!(report.unresolved.is_empty(), "{report:?}");
    assert_eq!(counts(&f), (1, 0, 1));
    assert_eq!(
        PlaylistRepository::name(&f.db, f.playlist)
            .unwrap()
            .unwrap(),
        "B"
    );
    f.db.with_reader(|c| {
        let baseline: String = c
            .query_row(
                "SELECT payload_json FROM sync_object_versions WHERE object_type='playlist'",
                [],
                |r| r.get(0),
            )
            .map_err(storage)?;
        assert_eq!(
            serde_json::from_str::<Value>(&baseline).unwrap()["display_name"],
            "A"
        );
        let receipt: (String, String, u64, u64) = c
            .query_row(
                "SELECT payload_digest,outcome_json,commit_seq,received_at FROM sync_inbox",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .map_err(storage)?;
        let exported = &exported_receipt.payload;
        assert_eq!(receipt.0, exported["payload_digest"].as_str().unwrap());
        assert_eq!(receipt.1, exported["original_outcome"].as_str().unwrap());
        assert_eq!(receipt.2, exported["sequence"].as_u64().unwrap());
        assert_eq!(receipt.3, exported["received_at"].as_u64().unwrap());
        let pending: (String, String, u64, String) = c
            .query_row(
                "SELECT payload_json,resource_refs_json,created_at,mutation_id FROM sync_mutations",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .map_err(storage)?;
        assert_eq!(
            pending,
            (
                f.frozen.original_body.clone(),
                f.frozen.original_resources_body.clone(),
                f.frozen.frozen_at_ms,
                f.frozen.candidate.mutation_id.as_uuid().to_string()
            )
        );
        Ok(())
    })
    .unwrap();
    let before = f.db.read_recovery_records(f.context.root).unwrap();
    assert_eq!(retry(&f), report);
    assert_eq!(counts(&f), (1, 0, 1));
    assert_eq!(f.db.read_recovery_records(f.context.root).unwrap(), before);
    assert_eq!(f.control.recover(f.context.root).unwrap().commits.len(), 1);
    let queue = f.db.pending_local_mutations(10).unwrap();
    // The fixture's device cache was created independently, but SQL selection
    // uses the actual current author and preserves the recovered candidate.
    assert_eq!(queue, vec![f.frozen.candidate.clone()]);
    assert!(f.directory.path().join("target.sqlite").exists());
}

#[test]
fn library_durability_trusted_sync_retry_rejects_stale_binding_author_role_and_directory() {
    let f = fixture();
    RetryLibraryOperation::new(f.control.as_ref(), &f.db)
        .catch_up(f.context.root)
        .unwrap();
    for field in ["binding", "author", "role", "library"] {
        let mut context = f.context;
        match field {
            "binding" => context.verified_checkpoint.binding.epoch = Epoch::from_u64(2),
            "author" => context.local_author = DeviceId::new(),
            "role" => context.role = RecoverySessionRole::Replica,
            _ => context.local_library_id = LibraryId::from_uuid(uuid::Uuid::new_v4()),
        }
        assert!(f.db.retry_library_sync_state(context).is_err(), "{field}");
        assert_eq!(counts(&f), (0, 0, 0));
    }
    f.db.writer
        .run(|c| {
            c.execute("DELETE FROM sync_binding", []).map_err(storage)?;
            Ok(())
        })
        .unwrap();
    assert!(f.db.retry_library_sync_state(f.context).is_err());
    assert_eq!(counts(&f), (0, 0, 0));
}

#[test]
fn library_durability_trusted_sync_retry_keeps_legacy_queue_untrusted() {
    let mut f = fixture();
    f.batch.operation_id = uuid::Uuid::new_v4();
    f.batch.state.retain(|s| {
        !matches!(
            s.kind,
            crate::domain::library::durability::StateKind::Pending
                | crate::domain::library::durability::StateKind::PendingDisposition
        )
    });
    f.batch.state.extend(frozen_states(
        StateNamespace::LegacyEvidence {
            source_root: f.context.root,
        },
        &f.frozen,
        MutationStatus::Pending,
    ));
    // A second directory commit adds legacy evidence alongside the genuine
    // Bound fact; use a fresh database retained set to isolate the legacy case.
    f.db.project_library_commits(f.context.root, &f.control.recover(f.context.root).unwrap())
        .unwrap();
    let states = f.batch.state.clone();
    let root = f.context.root;
    f.db.writer.run(move|c|{c.execute("DELETE FROM library_recovery_records WHERE row_kind='state' AND json_extract(envelope_json,'$.kind') IN ('pending','pending_disposition')",[]).map_err(storage)?;for s in &states {if s.key.as_ref().is_some_and(|k|matches!(k.namespace,StateNamespace::LegacyEvidence {..})){retained::save(c,root,&[],std::slice::from_ref(s),&[])?;}}Ok(())}).unwrap();
    let report = f.db.retry_library_sync_state(f.context).unwrap();
    assert_eq!(counts(&f), (0, 0, 1));
    assert_eq!(report.unresolved.len(), 2);
    assert!(report
        .unresolved
        .iter()
        .all(|r| r.reason == Reason::RequiresStateRecovery));
}

#[test]
fn library_durability_trusted_sync_retry_keeps_missing_parent_and_retries_real_directory_append() {
    let f = fixture_with_parent(false);
    let report = retry(&f);
    assert!(report
        .unresolved
        .iter()
        .any(|item| item.reason == Reason::MissingPlaylist));
    assert_eq!(counts(&f), (0, 0, 1));
    assert!(PlaylistRepository::by_id(&f.db, f.playlist)
        .unwrap()
        .is_none());
    let mut parent = f.batch.clone();
    parent.operation_id = uuid::Uuid::new_v4();
    parent.state.clear();
    f.control.commit(f.context.root, &parent).unwrap();
    assert!(retry(&f).unresolved.is_empty());
    assert_eq!(counts(&f), (1, 0, 1));
    assert_eq!(
        PlaylistRepository::name(&f.db, f.playlist)
            .unwrap()
            .as_deref(),
        Some("B")
    );
}

#[test]
fn library_durability_trusted_sync_retry_rolls_back_all_protocol_facts_on_sql_failure() {
    let f = fixture();
    RetryLibraryOperation::new(f.control.as_ref(), &f.db)
        .catch_up(f.context.root)
        .unwrap();
    f.db.writer.run(|c|{c.execute_batch("CREATE TRIGGER fail_recovered_mutation BEFORE INSERT ON sync_mutations BEGIN SELECT RAISE(ABORT,'injected sync projection failure'); END;").map_err(storage)}).unwrap();
    assert!(f.db.retry_library_sync_state(f.context).is_err());
    assert_eq!(counts(&f), (0, 0, 0));
    f.db.with_reader(|c|{let counts:(u64,u64)=c.query_row("SELECT (SELECT COUNT(*) FROM sync_object_versions),(SELECT COUNT(*) FROM sync_inbox)",[],|r|Ok((r.get(0)?,r.get(1)?))).map_err(storage)?;assert_eq!(counts,(0,0));Ok(())}).unwrap();
    assert_eq!(
        PlaylistRepository::name(&f.db, f.playlist)
            .unwrap()
            .as_deref(),
        Some("B")
    );
    f.db.writer
        .run(|c| {
            c.execute_batch("DROP TRIGGER fail_recovered_mutation")
                .map_err(storage)
        })
        .unwrap();
    assert!(f
        .db
        .retry_library_sync_state(f.context)
        .unwrap()
        .unresolved
        .is_empty());
    assert_eq!(counts(&f), (1, 0, 1));
}

#[test]
fn library_durability_trusted_sync_retry_never_selects_between_unsealed_frozen_versions() {
    let f = fixture();
    let mut newer = f.frozen.clone();
    newer.candidate.payload["display_name"] = json!("C");
    newer.original_body = serde_json::to_string(&newer.candidate.payload).unwrap();
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
        &newer,
        MutationStatus::Pending,
    );
    f.control.commit(f.context.root, &batch).unwrap();
    let report = retry(&f);
    assert_eq!(counts(&f), (0, 0, 1));
    assert_eq!(
        report
            .unresolved
            .iter()
            .filter(|r| r.reason == Reason::AmbiguousMutationVersion)
            .count(),
        4
    );
    assert_eq!(
        f.db.read_recovery_records(f.context.root)
            .unwrap()
            .state
            .iter()
            .filter(|s| s.kind == crate::domain::library::durability::StateKind::Pending)
            .count(),
        2
    );
}

#[test]
fn library_durability_trusted_sync_retry_late_old_disposition_does_not_terminate_rebased_version() {
    let f = fixture();
    retry(&f);
    let anchor = crate::domain::sync::OriginAnchor {
        origin_seq: 10,
        last_mutation_id: Some(MutationId::new()),
    };
    f.db.rebase_local_chain(anchor).unwrap();
    let mut newer = f.frozen.clone();
    newer.candidate.origin_seq = 11;
    newer.candidate.previous_mutation_id = anchor.last_mutation_id;
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
        &newer,
        MutationStatus::Pending,
    );
    batch.state.push(
        frozen_states(
            StateNamespace::Bound {
                binding: f.context.verified_checkpoint.binding,
            },
            &f.frozen,
            MutationStatus::Confirmed,
        )
        .remove(1),
    );
    f.control.commit(f.context.root, &batch).unwrap();
    let report = retry(&f);
    assert!(report
        .unresolved
        .iter()
        .any(|r| r.reason == Reason::AmbiguousMutationVersion));
    assert_eq!(
        f.db.pending_local_mutations(10).unwrap(),
        vec![newer.candidate]
    );
    assert_eq!(counts(&f), (1, 0, 1));
}

#[test]
fn library_durability_trusted_sync_retry_rejects_same_version_with_changed_timestamp() {
    let f = fixture();
    assert!(retry(&f).unresolved.is_empty());
    f.db.writer
        .run(|c| {
            c.execute(
                "UPDATE sync_object_versions SET changed_at_ms=changed_at_ms+1",
                [],
            )
            .map_err(storage)?;
            Ok(())
        })
        .unwrap();
    let report = retry(&f);
    assert!(report
        .unresolved
        .iter()
        .any(|item| item.reason == Reason::IdentityConflict));
    assert_eq!(counts(&f), (1, 0, 1));
}

#[test]
fn library_durability_confirmed_v2_rejects_mismatched_original_confirmation() {
    let f = fixture();
    let baseline = f
        .batch
        .state
        .iter()
        .find(|s| s.kind == crate::domain::library::durability::StateKind::Confirmed)
        .unwrap();
    let ledger = f
        .batch
        .state
        .iter()
        .find(|s| s.kind == crate::domain::library::durability::StateKind::SyncLedger)
        .unwrap();
    let mut payload = baseline.payload.clone();
    payload["schema"] = json!("echo.sync.confirmed.v2");
    for field in [
        "author",
        "mutation_id",
        "original_body",
        "previous_digest",
        "digest",
    ] {
        payload[field] = ledger.payload[field].clone();
    }
    let key = baseline.key.clone().unwrap();
    DurableStateChange::scoped(key.clone(), payload.clone()).unwrap();
    for field in [
        "author",
        "mutation_id",
        "original_body",
        "previous_digest",
        "digest",
        "changed_at_ms",
        "version",
        "sequence",
        "payload",
        "tombstone",
    ] {
        let mut invalid = payload.clone();
        invalid.as_object_mut().unwrap().remove(field);
        assert!(
            DurableStateChange::scoped(key.clone(), invalid).is_err(),
            "accepted missing {field}"
        );
    }
    for (field, value) in [
        ("author", json!(DeviceId::new())),
        ("mutation_id", json!(MutationId::new())),
        ("digest", json!("b".repeat(64))),
        ("payload", json!({"display_name":"forged"})),
        ("sequence", json!(2)),
        ("tombstone", json!(true)),
    ] {
        let mut invalid = payload.clone();
        invalid[field] = value;
        assert!(
            DurableStateChange::scoped(key.clone(), invalid).is_err(),
            "accepted changed {field}"
        );
    }
}

#[test]
fn library_durability_confirmed_tombstone_restores_sql_null_and_retries() {
    let mut f = fixture();
    let source = SqliteDatabase::open(f.directory.path().join("source.sqlite")).unwrap();
    let deleted = PlaylistId::new();
    // A peer deletion keeps this scenario independent of the local author's
    // pending fixture event; the origin still has to be an authorized device.
    let peer = DeviceId::new();
    source
        .authorize_sync_peer(
            f.context.verified_checkpoint.binding,
            peer,
            "peer".into(),
            "test-only-secure-handle".into(),
        )
        .unwrap();
    let deletion = MutationCandidate {
        mutation_id: MutationId::new(),
        origin_device_id: peer,
        origin_seq: 1,
        previous_mutation_id: None,
        object: ObjectKey::Playlist(deleted),
        base_object_version: None,
        payload: json!({"tombstone":true}),
        resource_refs: vec![],
    };
    let checkpoint = source
        .stage_confirmation_at(f.context.verified_checkpoint.binding, &deletion, Some(222))
        .unwrap()
        .checkpoint
        .unwrap();
    source.mark_published(checkpoint).unwrap();
    let mut state = source
        .with_reader(|c| {
            crate::infrastructure::sqlite::sync::durability::export_sync_state(c, f.context.root)
        })
        .unwrap();
    let tombstone = state
        .iter()
        .find(|state| {
            state.key.as_ref().is_some_and(|key| {
                key.target
                    == StateTarget::Confirmed {
                        object: ObjectKey::Playlist(deleted),
                    }
            })
        })
        .unwrap();
    assert_eq!(tombstone.payload["schema"], "echo.sync.confirmed.v2");
    assert_eq!(tombstone.payload["payload"], json!({"tombstone":true}));
    // The exporter deliberately does not claim the freeze-time pending scope,
    // so the projection fixture supplies that synthetic pending state.
    for frozen_state in &f.batch.state {
        if !state
            .iter()
            .any(|exported| exported.key == frozen_state.key)
        {
            state.push(frozen_state.clone());
        }
    }
    let mut batch = LibraryBatch::new(
        uuid::Uuid::new_v4(),
        f.context.local_library_id,
        ChangeOrigin::Recovery,
        vec![],
    );
    batch.state = state;
    f.control.commit(f.context.root, &batch).unwrap();
    f.context.verified_checkpoint = checkpoint;
    for _ in 0..2 {
        let report = retry(&f);
        assert!(report.unresolved.is_empty(), "{report:?}");
        f.db
            .with_reader(|c| {
                let baseline: (bool, Option<String>, u64) = c
                    .query_row(
                        "SELECT is_tombstone,payload_json,changed_at_ms FROM sync_object_versions WHERE object_type='playlist' AND object_uuid=?1",
                        [deleted.as_uuid().to_string()],
                        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                    )
                    .map_err(storage)?;
                assert_eq!(baseline, (true, None, 222));
                Ok(())
            })
            .unwrap();
        assert!(PlaylistRepository::by_id(&f.db, deleted).unwrap().is_none());
    }
}

mod boundaries;
