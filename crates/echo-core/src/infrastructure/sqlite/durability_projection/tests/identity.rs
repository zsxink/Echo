//! Reject actual local-library ownership mismatches before any SQL mutation.
use super::*;
use crate::domain::library::durability::LibraryCheckpoint;

fn assert_rejected_without_writes(db: &SqliteDatabase, root: LibraryRootId, replay: LibraryReplay) {
    db.writer
        .run(move |connection| {
            let before = connection.total_changes();
            assert!(project_commits(connection, root, &replay).is_err());
            assert_eq!(connection.total_changes(), before);
            Ok(())
        })
        .unwrap();
}

#[test]
fn library_durability_projection_rejects_cross_directory_chain_and_anchor_without_sql_writes() {
    let (_directory, db, root) = database();
    let library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let foreign = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let fixture = Issue72Fixture::with_database_evidence();
    let mut replay = LibraryReplay::default();
    append(&mut replay, library, vec![fixture.database[0].clone()]);
    append(&mut replay, foreign, vec![fixture.database[1].clone()]);
    assert_rejected_without_writes(&db, root, replay.clone());
    // A foreign suffix cannot borrow a locally owned coverage image either.
    let first = replay.commits.remove(0);
    replay.checkpoint = Some(LibraryCheckpoint {
        library_id: library,
        library_commit_seq: first.library_commit_seq,
        head_digest: first.digest,
        records: first.batch.records,
        state: vec![],
        resources: vec![],
    });
    assert_rejected_without_writes(&db, root, replay);
    assert_eq!(
        db.projection_status(root)
            .unwrap()
            .applied_library_commit_seq,
        0
    );
    assert!(db.read_recovery_records(root).unwrap().records.is_empty());
    assert!(db.all_in_root(root).unwrap().is_empty());
}

#[test]
fn library_durability_projection_rejects_foreign_watermark_without_sql_writes() {
    let (_directory, db, root) = database();
    let library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let fixture = Issue72Fixture::with_database_evidence();
    let mut original = LibraryReplay::default();
    append(&mut original, library, vec![fixture.database[0].clone()]);
    db.project_library_commits(root, &original).unwrap();
    let before = db.read_recovery_records(root).unwrap();
    let status = db.projection_status(root).unwrap();
    let commit = &original.commits[0];
    // Same seq/digest but a different local directory ID: the existing SQLite
    // watermark must reject this even when no suffix requires application.
    let replay = LibraryReplay {
        checkpoint: Some(LibraryCheckpoint {
            library_id: LibraryId::from_uuid(uuid::Uuid::new_v4()),
            library_commit_seq: commit.library_commit_seq,
            head_digest: commit.digest.clone(),
            records: commit.batch.records.clone(),
            state: vec![],
            resources: vec![],
        }),
        commits: vec![],
    };
    assert_rejected_without_writes(&db, root, replay);
    assert_eq!(db.read_recovery_records(root).unwrap(), before);
    assert_eq!(db.projection_status(root).unwrap(), status);
}
