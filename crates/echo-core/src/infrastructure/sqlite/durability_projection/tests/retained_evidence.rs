//! Stable recovery keys preserve private evidence without restoring network trust.
use super::*;
use crate::domain::library::durability::{
    DurableStateChange, ProjectionTarget, ProjectionUnresolvedReason, StateKind,
};

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
