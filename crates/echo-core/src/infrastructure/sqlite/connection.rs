//! Connection lifecycle, migrations, backup and integrity.

#![allow(
    clippy::doc_markdown,
    clippy::missing_errors_doc,
    clippy::needless_pass_by_value,
    clippy::redundant_pub_crate,
    clippy::uninlined_format_args
)]

use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::backup::Backup;
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};

use crate::error::Error;

use super::support::{now_ms, storage};

mod migrations {
    pub const RESOURCE_RETENTION: &str = include_str!("migrations/0023_resource_retention.sql");
    pub const OBJECT_CHANGE_TIME: &str = include_str!("migrations/0024_object_change_time.sql");
    pub const IGNORE_DERIVED_SONG_AUDIO_CONFLICTS: &str =
        include_str!("migrations/0025_ignore_derived_song_audio_conflicts.sql");
    pub const SYNC_SESSION_INTENT: &str = include_str!("migrations/0026_sync_session_intent.sql");
    pub const SYNC_CACHE_CLEANUP: &str = include_str!("migrations/0027_sync_cache_cleanup.sql");
    pub const INITIAL: &str = include_str!("migrations/0001_initial.sql");
    pub const LIBRARY_ASSETS: &str = include_str!("migrations/0002_library_assets.sql");
    pub const ROOT_WRITE_SAFETY: &str = include_str!("migrations/0003_root_write_safety.sql");
    pub const SONG_AUDIO_PARAMETERS: &str =
        include_str!("migrations/0004_song_audio_parameters.sql");
    pub const SYNC_FOUNDATION: &str = include_str!("migrations/0005_sync_foundation.sql");
    pub const PORTABLE_LAYOUT: &str = include_str!("migrations/0006_portable_layout.sql");
    pub const PLAYLIST_COVERS: &str = include_str!("migrations/0007_playlist_covers.sql");
    pub const FAVORITE_ORDER: &str = include_str!("migrations/0008_favorite_order.sql");
    pub const ARTIST_COVERS: &str = include_str!("migrations/0009_artist_covers.sql");
    pub const LAN_SYNC_LEDGER: &str = include_str!("migrations/0010_lan_sync_ledger.sql");
    pub const SEPARATE_LOCAL_ORIGIN_CURSOR: &str =
        include_str!("migrations/0011_separate_local_origin_cursor.sql");
    pub const BIND_SYNC_LOCAL_ROOT: &str = include_str!("migrations/0012_bind_sync_local_root.sql");
    pub const FIXED_HOST_SNAPSHOTS: &str = include_str!("migrations/0013_fixed_host_snapshots.sql");
    pub const REPLICA_SNAPSHOT_STAGING: &str =
        include_str!("migrations/0014_replica_snapshot_staging.sql");
    pub const MUTATION_RESOURCE_REFS: &str =
        include_str!("migrations/0016_mutation_resource_refs.sql");
    pub const BUDGETED_SNAPSHOT_PAGES: &str =
        include_str!("migrations/0015_budgeted_snapshot_pages.sql");
    pub const HOST_INITIAL_BASELINE: &str =
        include_str!("migrations/0017_host_initial_baseline.sql");
    pub const HOST_RECOVERY: &str = include_str!("migrations/0021_host_recovery.sql");
    pub const ROOT_SYNC_CONTEXTS: &str = include_str!("migrations/0020_root_sync_contexts.sql");
    pub const REMOTE_RESOURCE_PUBLICATION: &str =
        include_str!("migrations/0018_remote_resource_publication.sql");
}

const SQLITE_BUSY_TIMEOUT: Duration = Duration::from_secs(5);
const READER_COUNT: usize = 3;
// A development build briefly published version 20 with `IF NOT EXISTS` on
// the archive tables. Its schema is equivalent to the released migration,
// but databases created by that build carry this checksum.
const LEGACY_ROOT_SYNC_CONTEXTS_CHECKSUM: &str =
    "d73256ef23345b827007dcf425894a11bdd25e4c545a984b29ba0dd133bd6f7a";

/// Maximum size of the read-only connection pool.
pub(crate) const fn reader_count() -> usize {
    READER_COUNT
}

pub(crate) fn open_writer(path: &Path) -> Result<Connection, Error> {
    let connection = Connection::open(path).map_err(storage)?;
    configure_connection(&connection, true)?;
    Ok(connection)
}

pub(crate) fn open_reader(path: &Path) -> Result<Connection, Error> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(storage)?;
    configure_connection(&connection, false)?;
    Ok(connection)
}

fn configure_connection(connection: &Connection, writer: bool) -> Result<(), Error> {
    connection
        .busy_timeout(SQLITE_BUSY_TIMEOUT)
        .map_err(storage)?;
    connection
        .execute_batch("PRAGMA foreign_keys = ON;")
        .map_err(storage)?;
    if writer {
        connection
            .execute_batch("PRAGMA journal_mode = WAL; PRAGMA synchronous = FULL;")
            .map_err(storage)?;
    }
    Ok(())
}

pub(crate) fn apply_migrations(connection: &mut Connection) -> Result<(), Error> {
    connection.execute_batch("CREATE TABLE IF NOT EXISTS schema_migrations (version INTEGER PRIMARY KEY, checksum TEXT NOT NULL, applied_at INTEGER NOT NULL);").map_err(storage)?;
    apply_migration_set(
        connection,
        &[
            (1, migrations::INITIAL),
            (2, migrations::LIBRARY_ASSETS),
            (3, migrations::ROOT_WRITE_SAFETY),
            (4, migrations::SONG_AUDIO_PARAMETERS),
            (5, migrations::SYNC_FOUNDATION),
            (6, migrations::PORTABLE_LAYOUT),
            (7, migrations::PLAYLIST_COVERS),
            (8, migrations::FAVORITE_ORDER),
            (9, migrations::ARTIST_COVERS),
            (10, migrations::LAN_SYNC_LEDGER),
            (11, migrations::SEPARATE_LOCAL_ORIGIN_CURSOR),
            (12, migrations::BIND_SYNC_LOCAL_ROOT),
            (13, migrations::FIXED_HOST_SNAPSHOTS),
            (14, migrations::REPLICA_SNAPSHOT_STAGING),
            (15, migrations::BUDGETED_SNAPSHOT_PAGES),
            (16, migrations::MUTATION_RESOURCE_REFS),
            (17, migrations::HOST_INITIAL_BASELINE),
            (18, migrations::REMOTE_RESOURCE_PUBLICATION),
            (20, migrations::ROOT_SYNC_CONTEXTS),
            (21, migrations::HOST_RECOVERY),
            (23, migrations::RESOURCE_RETENTION),
            (24, migrations::OBJECT_CHANGE_TIME),
            (25, migrations::IGNORE_DERIVED_SONG_AUDIO_CONFLICTS),
            (26, migrations::SYNC_SESSION_INTENT),
            (27, migrations::SYNC_CACHE_CLEANUP),
        ],
    )
}

pub(crate) fn apply_migration_set(
    connection: &mut Connection,
    migrations: &[(i64, &str)],
) -> Result<(), Error> {
    for (version, script) in migrations {
        let checksum = blake3::hash(script.as_bytes()).to_hex().to_string();
        let current: Option<String> = connection
            .query_row(
                "SELECT checksum FROM schema_migrations WHERE version = ?1",
                params![version],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?;
        match current {
            Some(applied)
                if applied == checksum
                    || (*version == 20 && applied == LEGACY_ROOT_SYNC_CONTEXTS_CHECKSUM) =>
            {
                continue;
            }
            Some(_) => {
                return Err(Error::InvariantViolation {
                    why: format!(
                    "migration {version:04} checksum mismatch; released migrations are immutable"
                ),
                })
            }
            None => {}
        }
        let transaction = connection.transaction().map_err(storage)?;
        if *version == 20 && *script == migrations::ROOT_SYNC_CONTEXTS {
            apply_root_sync_contexts(&transaction, script)?;
        } else {
            transaction.execute_batch(script).map_err(storage)?;
        }
        transaction
            .execute(
                "INSERT INTO schema_migrations (version, checksum, applied_at) VALUES (?1, ?2, ?3)",
                params![version, checksum, now_ms()],
            )
            .map_err(storage)?;
        transaction.commit().map_err(storage)?;
    }
    Ok(())
}

// Development databases already contain version 19's root-scoped local
// queues. Preserve them byte for byte; version 18 needs the queue conversion.
// The checksum remains that of the complete immutable version 20 script.
fn apply_root_sync_contexts(connection: &Connection, script: &str) -> Result<(), Error> {
    let unrooted_binding: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_binding WHERE local_root_uuid IS NULL)",
            [],
            |row| row.get(0),
        )
        .map_err(storage)?;
    if unrooted_binding {
        return Err(Error::conflict(
            "historical sync ledger has no local root; explicit repair is required",
        ));
    }
    let legacy: Option<String> = connection
        .query_row(
            "SELECT checksum FROM schema_migrations WHERE version=19",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage)?;
    if legacy.as_deref().is_some_and(|checksum| {
        checksum != "60de0539e8220bb0ff740079b772a7853597bd68f540c711ef9fae3e3633f5c9"
    }) {
        return Err(Error::InvariantViolation {
            why: "unrecognized historical migration 0019 checksum".to_owned(),
        });
    }
    let (prefix, remainder) = script
        .split_once("-- BEGIN ROOT QUEUE UPGRADE\n")
        .ok_or_else(|| Error::InvariantViolation {
            why: "missing root queue migration boundary".to_owned(),
        })?;
    let (queues, suffix) = remainder
        .split_once("-- END ROOT QUEUE UPGRADE\n")
        .ok_or_else(|| Error::InvariantViolation {
            why: "missing root queue migration boundary".to_owned(),
        })?;
    connection.execute_batch(prefix).map_err(storage)?;
    let mut statement = connection
        .prepare("PRAGMA table_info(sync_mutations)")
        .map_err(storage)?;
    let scoped = statement
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage)?
        .iter()
        .any(|column| column == "local_root_uuid");
    drop(statement);
    if !scoped {
        connection.execute_batch(queues).map_err(storage)?;
    }
    connection.execute_batch(suffix).map_err(storage)?;
    // The old singleton binding is the only reliable owner of its ledger.
    // A UI marker from an earlier implementation may already name another
    // root; archive by the binding before selecting the active library.
    let binding_root: Option<Option<String>> = connection
        .query_row(
            "SELECT local_root_uuid FROM sync_binding WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage)?;
    if let Some(Some(root)) = &binding_root {
        connection
            .execute(
                "UPDATE sync_active_context SET local_root_uuid=?1 WHERE singleton=1",
                params![root],
            )
            .map_err(storage)?;
    }
    // An unrooted historical binding must remain untouched for explicit repair.
    if binding_root != Some(None) {
        let active: Option<String> = connection
            .query_row(
                "SELECT uuid FROM library_roots WHERE is_active=1",
                [],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?;
        if let Some(root) = active {
            super::sync_context::select(connection, &root)?;
        }
    }
    Ok(())
}

pub(crate) fn backup_connection(source: &Connection, destination: &Path) -> Result<(), Error> {
    let mut target = Connection::open(destination).map_err(storage)?;
    let backup = Backup::new(source, &mut target).map_err(storage)?;
    backup
        .run_to_completion(128, Duration::ZERO, None)
        .map_err(storage)
}

pub(crate) fn quick_check_connection(connection: &Connection) -> Result<(), Error> {
    let result: String = connection
        .query_row("PRAGMA quick_check", [], |row| row.get(0))
        .map_err(storage)?;
    if result == "ok" {
        Ok(())
    } else {
        Err(Error::Storage {
            what: "integrity".to_owned(),
            source: format!("SQLite quick_check: {result}").into(),
        })
    }
}

pub(crate) fn file_is_non_empty(path: &Path) -> Result<bool, Error> {
    std::fs::metadata(path)
        .map(|metadata| metadata.len() > 0)
        .map_err(|source| Error::io("inspect database", source, path))
}
pub(crate) fn backup_path(path: &Path) -> PathBuf {
    path.with_extension("pre-migration.bak")
}
