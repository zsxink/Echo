//! Export context provenance without borrowing the globally selected binding.
use super::*;
use crate::application::ports::{LibraryRepository, LocalSyncQueuePort};
use crate::domain::{
    entities::LibraryRoot,
    library::durability::StateTarget,
    sync::{AuthorityId, Epoch, LibraryId, SyncBinding},
};
use crate::infrastructure::sqlite::SqliteDatabase;
use serde_json::json;

struct Fixture {
    directory: tempfile::TempDir,
    db: SqliteDatabase,
    a: LibraryRootId,
    b: LibraryRootId,
    object: Uuid,
    a_binding: SyncBinding,
    b_binding: SyncBinding,
}
fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let db = SqliteDatabase::open(dir.path().join("scope.sqlite")).unwrap();
    let a = LibraryRootId::new();
    let b = LibraryRootId::new();
    for root in [a, b] {
        LibraryRepository::upsert(
            &db,
            &LibraryRoot::new(root, dir.path().join(root.to_string()), false, true),
        )
        .unwrap();
    }
    let a_binding = SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    let mut b_binding = a_binding;
    b_binding.authority_id = AuthorityId::new();
    let object = Uuid::new_v4();
    db.bind_sync_library(a_binding, true, a).unwrap();
    insert_confirmed(&db, object, "A");
    db.bind_sync_library(b_binding, true, b).unwrap();
    insert_confirmed(&db, object, "B");
    Fixture {
        directory: dir,
        db,
        a,
        b,
        object,
        a_binding,
        b_binding,
    }
}
#[test]
fn library_durability_sync_export_keeps_target_active_and_archived_binding() {
    let fixture = fixture();
    let db = &fixture.db;
    let (a, b, a_binding, b_binding) = (fixture.a, fixture.b, fixture.a_binding, fixture.b_binding);
    let a_states = db.with_reader(|c| export_sync_state(c, a)).unwrap();
    let b_states = db.with_reader(|c| export_sync_state(c, b)).unwrap();
    let a_state = a_states
        .iter()
        .find(|s| s.kind == StateKind::Confirmed)
        .unwrap();
    let b_state = b_states
        .iter()
        .find(|s| s.kind == StateKind::Confirmed)
        .unwrap();
    assert_eq!(
        a_state.key.as_ref().unwrap().namespace,
        StateNamespace::Bound { binding: a_binding }
    );
    assert_eq!(
        b_state.key.as_ref().unwrap().namespace,
        StateNamespace::Bound { binding: b_binding }
    );
    assert_ne!(a_state.id, b_state.id);
    assert_eq!(a_state.payload["payload"]["title"], "A");
    assert_eq!(b_state.payload["payload"]["title"], "B");
    db.writer
        .run(move |c| {
            c.execute("UPDATE sync_binding SET epoch=2", [])
                .map_err(storage)?;
            Ok(())
        })
        .unwrap();
    let changed = db.with_reader(|c| export_sync_state(c, b)).unwrap();
    assert_ne!(
        b_state.id,
        changed
            .iter()
            .find(|s| s.kind == StateKind::Confirmed)
            .unwrap()
            .id
    );
}
#[test]
fn library_durability_sync_export_versions_legacy_reanchor_without_changing_event_id() {
    let fixture = fixture();
    let db = &fixture.db;
    let (b, object) = (fixture.b, fixture.object);
    let author: Uuid = db
        .with_reader(|c| {
            let raw: String = c
                .query_row(
                    "SELECT value FROM device_state WHERE key='device_id'",
                    [],
                    |r| r.get(0),
                )
                .map_err(storage)?;
            raw.parse().map_err(storage)
        })
        .unwrap();
    let mutation = Uuid::new_v4();
    db.writer.run(move |c| {
        c.execute("INSERT INTO sync_mutations(mutation_id,origin_device_id,origin_seq,previous_mutation_id,object_type,object_uuid,operation,local_revision,base_object_version,payload_json,payload_digest,status,created_at,resource_refs_json,local_root_uuid) VALUES(?1,?2,3,?3,'song',?4,'upsert',9,2,?5,?6,'pending',1234,'[]',?7)",params![mutation.to_string(),author.to_string(),Uuid::new_v4().to_string(),object.to_string(),"{ \"title\" : \"original\" }","a".repeat(64),b.to_string()]).map_err(storage)?;
        Ok(())
    }).unwrap();
    let before = db.with_reader(|c| export_sync_state(c, b)).unwrap();
    let frozen = before
        .iter()
        .find(|s| s.kind == StateKind::Pending)
        .unwrap();
    assert_eq!(
        frozen.key.as_ref().unwrap().namespace,
        StateNamespace::LegacyEvidence { source_root: b }
    );
    assert!(matches!(
        frozen.key.as_ref().unwrap().target,
        StateTarget::Mutation { .. }
    ));
    assert_ne!(frozen.id, mutation);
    assert_eq!(
        frozen.payload["frozen"]["original_body"],
        "{ \"title\" : \"original\" }"
    );
    assert_eq!(
        frozen.payload["frozen"]["candidate"]["mutation_id"],
        json!(mutation)
    );
    // The real queue rebase remains legal. It creates a new immutable evidence
    // version while preserving the original event identity and old body.
    let predecessor = crate::domain::sync::MutationId::new();
    assert_eq!(
        db.rebase_local_chain(crate::domain::sync::OriginAnchor {
            origin_seq: 70,
            last_mutation_id: Some(predecessor)
        })
        .unwrap(),
        1
    );
    let rebased = db.with_reader(|c| export_sync_state(c, b)).unwrap();
    let rebased_frozen = rebased
        .iter()
        .find(|s| s.kind == StateKind::Pending)
        .unwrap();
    assert_ne!(frozen.id, rebased_frozen.id);
    assert_eq!(
        rebased_frozen.payload["frozen"]["candidate"]["mutation_id"],
        json!(mutation)
    );
    assert_eq!(
        rebased_frozen.payload["frozen"]["candidate"]["origin_seq"],
        71
    );
    assert_eq!(
        rebased_frozen.payload["frozen"]["candidate"]["previous_mutation_id"],
        json!(predecessor)
    );
    let history = crate::domain::library::durability::LibraryReplay {
        checkpoint: Some(crate::domain::library::durability::LibraryCheckpoint {
            library_id: crate::domain::library::LibraryId::from_uuid(Uuid::new_v4()),
            library_commit_seq: 1,
            head_digest: "a".repeat(64),
            records: vec![],
            state: before.clone(),
            resources: vec![],
        }),
        commits: vec![],
    };
    history.validate_state_extension(&rebased).unwrap();
    db.writer.run(move |c| {
        c.execute("UPDATE sync_mutations SET status='confirmed',outcome_json='{\"sequence\":99}' WHERE mutation_id=?1",[mutation.to_string()]).map_err(storage)?;
        Ok(())
    }).unwrap();
    let after = db.with_reader(|c| export_sync_state(c, b)).unwrap();
    assert_eq!(
        rebased_frozen,
        after.iter().find(|s| s.kind == StateKind::Pending).unwrap()
    );
    let before_disposition = rebased
        .iter()
        .find(|s| s.kind == StateKind::PendingDisposition)
        .unwrap();
    let after_disposition = after
        .iter()
        .find(|s| s.kind == StateKind::PendingDisposition)
        .unwrap();
    assert_eq!(before_disposition.id, after_disposition.id);
    assert_ne!(before_disposition.payload, after_disposition.payload);
}
fn insert_confirmed(db: &SqliteDatabase, object: Uuid, title: &str) {
    let body = json!({"title":title}).to_string();
    db.writer.run(move |c| {
        c.execute("INSERT INTO sync_object_versions(object_type,object_uuid,object_version,commit_seq,payload_json,is_tombstone,changed_at_ms) VALUES('song',?1,1,1,?2,0,10)",params![object.to_string(),body]).map_err(storage)?;
        Ok(())
    }).unwrap();
}

#[test]
fn library_durability_sync_export_preserves_incomplete_replica_receipts() {
    let fixture = fixture();
    let author = Uuid::new_v4();
    let mutation = Uuid::new_v4();
    let raw = "{ \"applied\" : true }";
    fixture.db.writer.run(move |c| {
        c.execute("INSERT INTO sync_inbox(origin_device_id,mutation_id,payload_digest,outcome_json,commit_seq,received_at) VALUES(?1,?2,?3,?4,1,42)",params![author.to_string(),mutation.to_string(),"a".repeat(64),raw]).map_err(storage)?;
        Ok(())
    }).unwrap();
    let states = fixture
        .db
        .with_reader(|c| export_sync_state(c, fixture.b))
        .unwrap();
    let receipt = states
        .iter()
        .find(|state| state.kind == StateKind::Receipt)
        .unwrap();
    assert_eq!(receipt.payload["schema"], "echo.sync.receipt.v1");
    assert_eq!(receipt.payload["original_outcome"], raw);
    assert_eq!(receipt.payload["outcome"], json!({"applied":true}));
    assert_eq!(receipt.payload["received_at"], 42);
    assert_eq!(receipt.payload["sequence"], 1);
    receipt.validate().unwrap();
}

mod resources;
