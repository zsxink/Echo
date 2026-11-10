//! Stable recovery keys preserve private evidence without restoring network trust.
use super::*;
use crate::domain::library::durability::{
    DurableStateChange, ProjectionTarget, ProjectionUnresolvedReason, StateKind,
};

/// The retained catalog's key for a record.
fn object_key(envelope: &PortableRecordEnvelope) -> String {
    serde_json::to_string(&envelope.record().object_key().unwrap()).unwrap()
}

#[test]
fn library_durability_projection_reads_private_evidence_with_stable_typed_keys() {
    let (directory, db, root) = sync_database();
    let library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let id = uuid::Uuid::from_u128(1234);
    let states = vec![
        DurableStateChange::new(
            StateKind::Confirmed,
            id,
            serde_json::json!({"schema":"echo.sync.object.v1","payload":{"name":"A"}}),
        ),
        DurableStateChange::new(
            StateKind::Pending,
            id,
            serde_json::json!({"schema":"echo.sync.mutation.v1","payload":{"name":"B"},"predecessor":null,"status":"sent"}),
        ),
    ];
    let mut replay = LibraryReplay::default();
    append(&mut replay, library, vec![]);
    let commit = replay.commits.last_mut().unwrap();
    commit.batch.state.clone_from(&states);
    commit.digest = LibraryCommit::calculate_digest(&commit.batch, 1, None).unwrap();
    db.project_library_commits(root, &replay).unwrap();
    let retained = db.read_recovery_records(root).unwrap();
    assert_eq!(retained.state, states);
    let report = db.projection_status(root).unwrap();
    assert_eq!(report.unresolved.len(), 2);
    for state in &states {
        assert!(report.unresolved.iter().any(|item| {
            item.target
                == ProjectionTarget::State {
                    state_kind: state.kind,
                    id,
                }
                && item.reason == ProjectionUnresolvedReason::RequiresStateRecovery
        }));
    }
    db.with_reader(|connection| {
        let mut query = connection.prepare("SELECT object_key FROM library_recovery_records WHERE library_root_uuid=?1 ORDER BY object_key").map_err(storage)?;
        let keys = query.query_map([root.to_string()], |row| row.get::<_, String>(0)).map_err(storage)?;
        let actual: Vec<String> = keys.collect::<Result<_, _>>().map_err(storage)?;
        assert_eq!(actual, vec![
            format!("{{\"kind\":\"state\",\"state_kind\":\"confirmed\",\"id\":\"{id}\"}}"),
            format!("{{\"kind\":\"state\",\"state_kind\":\"pending\",\"id\":\"{id}\"}}"),
        ]);
        let active: u64 = connection.query_row("SELECT COUNT(*) FROM sync_binding", [], |row| row.get(0)).map_err(storage)?;
        let mutations: u64 = connection.query_row("SELECT COUNT(*) FROM sync_mutations", [], |row| row.get(0)).map_err(storage)?;
        assert_eq!((active, mutations), (1, 0));
        Ok(())
    }).unwrap();
    drop(db);
    let reopened = SqliteDatabase::open(directory.path().join("projection.sqlite")).unwrap();
    assert_eq!(reopened.read_recovery_records(root).unwrap(), retained);
    reopened.project_library_commits(root, &replay).unwrap();
    assert_eq!(reopened.read_recovery_records(root).unwrap(), retained);
    assert_eq!(reopened.projection_status(root).unwrap(), report);
}

#[test]
fn library_durability_projection_scoped_same_object_history_has_complete_stable_keys() {
    use crate::domain::library::durability::{StateKey, StateNamespace, StateTarget};
    use crate::domain::sync::{AuthorityId, Epoch, LibraryId as SyncId, ObjectKey, SyncBinding};
    let (_directory, db, root) = sync_database();
    let library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let object = SongId::new();
    let binding = SyncBinding {
        library_id: SyncId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    let mut states = vec![];
    for epoch in [1, 2] {
        let mut binding = binding;
        binding.epoch = Epoch::from_u64(epoch);
        states.push(DurableStateChange::scoped(StateKey { namespace:StateNamespace::Bound { binding },target:StateTarget::Confirmed { object:ObjectKey::Song(object) } },serde_json::json!({"object_type":"song","object_uuid":object,"payload":{"title":format!("epoch {epoch}")}})).unwrap());
    }
    let mut replay = LibraryReplay::default();
    append(&mut replay, library, vec![]);
    let commit = replay.commits.last_mut().unwrap();
    commit.batch.state = states.clone();
    commit.digest = LibraryCommit::calculate_digest(&commit.batch, 1, None).unwrap();
    db.project_library_commits(root, &replay).unwrap();
    let retained = db.read_recovery_records(root).unwrap();
    assert_eq!(retained.state.len(), 2);
    let report = db.projection_status(root).unwrap();
    for state in states {
        assert!(retained.state.contains(&state));
        assert!(report.unresolved.iter().any(|item| item.target
            == ProjectionTarget::ScopedState {
                key: state.key.clone().unwrap()
            }));
    }
    db.with_reader(|c| {
        let count: u64 = c
            .query_row("SELECT COUNT(*) FROM sync_binding", [], |r| r.get(0))
            .map_err(storage)?;
        assert_eq!(count, 1);
        Ok(())
    })
    .unwrap();
}

/// The projection reads the retained catalog **on demand**: a pass reads the
/// pending records, the tombstone records, the whole state and resource set,
/// and only the song records a pending publication refers to.
///
/// The narrowed read must therefore decide exactly the same thing as the full
/// read it replaced. The retry consults a record only to retry it (it must be
/// pending) or to resolve a dependency against it (it must be a tombstone),
/// and it consults **every** state and **every** resource. This asserts that
/// equivalence directly: for the same catalog, the narrowed read retains
/// exactly the pending and tombstone records, and the same states and
/// resources as the full read — and it retains each row **once**, even when
/// two of the narrowed queries reach the same row.
#[test]
fn library_durability_on_demand_retained_read_matches_the_full_read() {
    let (_directory, db, root) = sync_database();
    let library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let fixture = Issue72Fixture::with_database_evidence();
    // A first commit leaves the relations pending (their songs are not
    // projected yet), so the catalog holds both pending and resolved rows.
    let mut replay = LibraryReplay::default();
    let relations: Vec<_> = fixture
        .database
        .iter()
        .filter(|record| !matches!(record, PortableRecord::Song(_)))
        .cloned()
        .collect();
    append(&mut replay, library, relations);
    db.project_library_commits(root, &replay).unwrap();
    // A second commit projects the songs, resolving most of the catalog. It
    // also carries **state** and **resource** rows: the narrowed read reaches
    // those rows through more than one of its queries, so a row retained
    // twice would show up here.
    let songs: Vec<_> = fixture
        .database
        .iter()
        .filter(|record| matches!(record, PortableRecord::Song(_)))
        .cloned()
        .collect();
    append(&mut replay, library, songs);
    let predecessor = replay.commits[0].digest.clone();
    let commit = replay.commits.last_mut().unwrap();
    commit.batch.state = vec![
        DurableStateChange::new(
            StateKind::Pending,
            uuid::Uuid::from_u128(4_001),
            serde_json::json!({"schema":"echo.sync.object.v1","payload":{"name":"state-a"}}),
        ),
        DurableStateChange::new(
            StateKind::Confirmed,
            uuid::Uuid::from_u128(4_002),
            serde_json::json!({"schema":"echo.sync.object.v1","payload":{"name":"state-b"}}),
        ),
    ];
    commit.batch.resources = vec![
        crate::domain::library::durability::DurableResource {
            relative_path: "media/a.flac".into(),
            content_hash: blake3::hash(b"a").to_hex().to_string(),
            byte_length: 1,
        },
        crate::domain::library::durability::DurableResource {
            relative_path: "media/b.flac".into(),
            content_hash: blake3::hash(b"b").to_hex().to_string(),
            byte_length: 1,
        },
    ];
    commit.digest =
        LibraryCommit::calculate_digest(&commit.batch, 2, Some(predecessor.as_str())).unwrap();
    db.project_library_commits(root, &replay).unwrap();
    assert!(
        !db.read_recovery_records(root).unwrap().state.is_empty(),
        "the fixture must retain state rows"
    );
    assert!(
        !db.read_recovery_records(root).unwrap().resources.is_empty(),
        "the fixture must retain resource rows"
    );

    let full = db.read_recovery_records(root).unwrap();
    let pending = db
        .with_reader(|connection| super::super::retained::pending_keys_for_test(connection, root))
        .unwrap();
    let on_demand = db
        .with_reader(|connection| {
            super::super::retained::read_for_retry_for_test(connection, root, &pending)
        })
        .unwrap();

    // The retry consults every state and every resource: the allocation index
    // pairs a mutation with its disposition across the whole state set, and a
    // publication is complete only when every resource it names is retained.
    assert_eq!(
        on_demand.state, full.state,
        "the narrowed read must retain every state"
    );
    assert_eq!(
        on_demand.resources, full.resources,
        "the narrowed read must retain every resource"
    );

    // The retry consults a record only to retry it — it must be pending — or
    // to resolve a dependency against it — it must be a tombstone.
    let expected: Vec<_> = full
        .records
        .iter()
        .filter(|envelope| {
            pending.contains(&object_key(envelope)) || envelope.record().is_tombstone()
        })
        .cloned()
        .collect();
    assert_eq!(
        on_demand.records, expected,
        "the narrowed read must retain exactly the pending and tombstone records"
    );

    // Every row is retained exactly once: the pending query and the
    // state/resource query can reach the same row, and the tombstone query
    // can reach a pending row again.
    let mut keys: Vec<String> = on_demand.records.iter().map(object_key).collect();
    let total = keys.len();
    keys.sort();
    keys.dedup();
    assert_eq!(
        keys.len(),
        total,
        "the narrowed read retains each record once"
    );
}
