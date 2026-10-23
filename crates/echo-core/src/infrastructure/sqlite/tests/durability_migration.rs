//! Upgrade a real released schema, preserving evidence across a failed migration.
use super::*;
use crate::application::ports::LibraryHistoryPort;
use crate::domain::library::durability::{DurableStateChange, ProjectionTarget, StateKind};

fn through_v27(connection: &mut rusqlite::Connection) {
    let scripts = [
        (1, include_str!("../migrations/0001_initial.sql")),
        (2, include_str!("../migrations/0002_library_assets.sql")),
        (3, include_str!("../migrations/0003_root_write_safety.sql")),
        (
            4,
            include_str!("../migrations/0004_song_audio_parameters.sql"),
        ),
        (5, include_str!("../migrations/0005_sync_foundation.sql")),
        (6, include_str!("../migrations/0006_portable_layout.sql")),
        (7, include_str!("../migrations/0007_playlist_covers.sql")),
        (8, include_str!("../migrations/0008_favorite_order.sql")),
        (9, include_str!("../migrations/0009_artist_covers.sql")),
        (10, include_str!("../migrations/0010_lan_sync_ledger.sql")),
        (
            11,
            include_str!("../migrations/0011_separate_local_origin_cursor.sql"),
        ),
        (
            12,
            include_str!("../migrations/0012_bind_sync_local_root.sql"),
        ),
        (
            13,
            include_str!("../migrations/0013_fixed_host_snapshots.sql"),
        ),
        (
            14,
            include_str!("../migrations/0014_replica_snapshot_staging.sql"),
        ),
        (
            15,
            include_str!("../migrations/0015_budgeted_snapshot_pages.sql"),
        ),
        (
            16,
            include_str!("../migrations/0016_mutation_resource_refs.sql"),
        ),
        (
            17,
            include_str!("../migrations/0017_host_initial_baseline.sql"),
        ),
        (
            18,
            include_str!("../migrations/0018_remote_resource_publication.sql"),
        ),
        (
            20,
            include_str!("../migrations/0020_root_sync_contexts.sql"),
        ),
        (21, include_str!("../migrations/0021_host_recovery.sql")),
        (
            23,
            include_str!("../migrations/0023_resource_retention.sql"),
        ),
        (
            24,
            include_str!("../migrations/0024_object_change_time.sql"),
        ),
        (
            25,
            include_str!("../migrations/0025_ignore_derived_song_audio_conflicts.sql"),
        ),
        (
            26,
            include_str!("../migrations/0026_sync_session_intent.sql"),
        ),
        (
            27,
            include_str!("../migrations/0027_sync_cache_cleanup.sql"),
        ),
    ];
    connection.execute_batch("CREATE TABLE schema_migrations (version INTEGER PRIMARY KEY, checksum TEXT NOT NULL, applied_at INTEGER NOT NULL)").unwrap();
    apply_migration_set(connection, &scripts).unwrap();
}

#[test]
fn migration_0028_preserves_legacy_evidence_and_rolls_back_partial_upgrade() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("legacy.sqlite");
    let mut connection = open_writer(&path).unwrap();
    through_v27(&mut connection);
    let root = LibraryRootId::new();
    connection.execute("INSERT INTO library_roots(uuid,absolute_path,normalized_path_key,is_active,write_capable,created_at,updated_at) VALUES(?1,'/synthetic/library','/synthetic/library',1,1,123,456)", [root.to_string()]).unwrap();
    let prior: String = connection.query_row("SELECT group_concat(version || ':' || checksum, ',') FROM (SELECT version,checksum FROM schema_migrations ORDER BY version)", [], |row| row.get(0)).unwrap();
    let script = include_str!("../migrations/0028_library_durability.sql");
    let failing = format!("{script}\nINSERT INTO table_that_does_not_exist VALUES(1);");
    assert!(apply_migration_set(&mut connection, &[(28, &failing)]).is_err());
    for table in [
        "library_projection_watermarks",
        "library_applied_operations",
        "library_recovery_records",
    ] {
        assert_eq!(
            connection
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |row| row.get::<_, u64>(0)
                )
                .unwrap(),
            0
        );
    }
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM schema_migrations WHERE version=28",
                [],
                |row| row.get::<_, u64>(0)
            )
            .unwrap(),
        0
    );
    apply_migrations(&mut connection).unwrap();
    let first = connection.total_changes();
    apply_migrations(&mut connection).unwrap();
    assert_eq!(
        connection.total_changes(),
        first,
        "reopening must not rerun migration writes"
    );
    let preserved: String = connection.query_row("SELECT group_concat(version || ':' || checksum, ',') FROM (SELECT version,checksum FROM schema_migrations WHERE version<28 ORDER BY version)", [], |row| row.get(0)).unwrap();
    assert_eq!(preserved, prior);
    let timestamps: (u64, u64) = connection
        .query_row(
            "SELECT created_at,updated_at FROM library_roots WHERE uuid=?1",
            [root.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(timestamps, (123, 456));
    assert!(connection
        .execute(
            "INSERT INTO library_projection_watermarks VALUES(?1,?2,0,NULL)",
            params![
                LibraryRootId::new().to_string(),
                uuid::Uuid::new_v4().to_string()
            ]
        )
        .is_err());
    let evidence = DurableStateChange::new(
        StateKind::Pending,
        uuid::Uuid::new_v4(),
        serde_json::json!({"frozen":"original"}),
    );
    let json = serde_json::to_string(&evidence).unwrap();
    let key = serde_json::to_string(&ProjectionTarget::State {
        state_kind: evidence.kind,
        id: evidence.id,
    })
    .unwrap();
    connection.execute("INSERT INTO library_recovery_records(library_root_uuid,object_key,envelope_json,row_kind,unresolved_reason) VALUES(?1,?2,?3,'state','\"requires_state_recovery\"')", params![root.to_string(),key,json]).unwrap();
    assert!(connection.execute("INSERT INTO library_recovery_records(library_root_uuid,object_key,envelope_json,row_kind) VALUES(?1,'invalid','{}','unknown')", [root.to_string()]).is_err());
    drop(connection);
    let reopened = SqliteDatabase::open(&path).unwrap();
    assert!(reopened.quick_check().is_ok());
    assert_eq!(
        reopened.read_recovery_records(root).unwrap().state,
        vec![evidence]
    );
}
