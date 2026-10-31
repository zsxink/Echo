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
    pub const LIBRARY_DURABILITY: &str = include_str!("migrations/0028_library_durability.sql");
    pub const RESOURCE_RETENTION: &str = include_str!("migrations/0023_resource_retention.sql");
    pub const OBJECT_CHANGE_TIME: &str = include_str!("migrations/0024_object_change_time.sql");
    pub const IGNORE_DERIVED_SONG_AUDIO_CONFLICTS: &str =
        include_str!("migrations/0025_ignore_derived_song_audio_conflicts.sql");
    pub const SYNC_SESSION_INTENT: &str = include_str!("migrations/0026_sync_session_intent.sql");
    pub const SYNC_CACHE_CLEANUP: &str = include_str!("migrations/0027_sync_cache_cleanup.sql");
    pub const DROP_SYNC_BINDING_PROTOCOL_VERSION: &str =
        include_str!("migrations/0029_drop_sync_binding_protocol_version.sql");
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
            (28, migrations::LIBRARY_DURABILITY),
            (29, migrations::DROP_SYNC_BINDING_PROTOCOL_VERSION),
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
            Some(applied) if applied == checksum => {
                continue;
            }
            Some(_) => {
                return Err(Error::InvariantViolation {
                    why: format!(
                        "migration {version:04} checksum mismatch; released migrations are immutable"
                    ),
                });
            }
            None => {}
        }
        let transaction = connection.transaction().map_err(storage)?;
        transaction.execute_batch(script).map_err(storage)?;
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

/// Does this open failure mean the file itself is damaged, rather than the
/// environment around it being unusable?
///
/// Only damage justifies quarantining a file: a corrupt page, an unreadable
/// header or a failed `quick_check` cannot be projected from, so a fresh
/// database loses nothing recoverable. A lock held by another instance, a
/// read-only volume, a missing permission or a released-migration checksum
/// mismatch must still fail the open — renaming such a database aside would
/// throw away a healthy projection the caller could otherwise retry against.
pub(crate) fn is_database_damage(error: &Error) -> bool {
    match error {
        Error::Storage { what, source } => {
            what == "integrity" || source_is_corruption(source.as_ref())
        }
        // `Io` carries a path: never treat an environment failure as damage.
        _ => false,
    }
}

fn source_is_corruption(source: &(dyn std::error::Error + 'static)) -> bool {
    source
        .downcast_ref::<rusqlite::Error>()
        .is_some_and(|error| match error {
            rusqlite::Error::SqliteFailure(code, _) => matches!(
                code.code,
                rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase
            ),
            _ => false,
        })
}

/// Move a damaged database (and its WAL/SHM companions) to a unique sibling
/// name without ever overwriting or deleting an existing file.
///
/// The name is derived from the original stem plus a UUID so repeated failures
/// accumulate evidence instead of replacing it. A same-directory rename keeps
/// the file on one filesystem; the companions must move with it because a WAL
/// left beside a fresh database would be replayed into it.
pub(crate) fn quarantine_database(path: &Path, cause: &Error) -> Result<PathBuf, Error> {
    let stem = path.file_stem().map_or_else(
        || "echo".to_owned(),
        |stem| stem.to_string_lossy().into_owned(),
    );
    let destination = path.with_file_name(format!(
        "{stem}.quarantined-{}.sqlite",
        uuid::Uuid::new_v4()
    ));
    if destination.exists() {
        return Err(Error::conflict("database quarantine name is already taken"));
    }
    std::fs::rename(path, &destination)
        .map_err(|source| Error::io("quarantine damaged database", source, &destination))?;
    for suffix in ["-wal", "-shm"] {
        let companion = PathBuf::from(format!("{}{suffix}", path.display()));
        if companion.exists() {
            let target = PathBuf::from(format!("{}{suffix}", destination.display()));
            // A companion that cannot move must not block startup: the main
            // file is already aside, and SQLite treats an orphaned WAL as
            // disposable when the database it belonged to is gone.
            if let Err(source) = std::fs::rename(&companion, &target) {
                tracing::warn!(
                    companion = %crate::logging::redact_path(&companion),
                    cause = %cause.to_log(crate::logging::DiagnosticMode::default()),
                    "could not move the quarantined database companion: {source}"
                );
            }
        }
    }
    Ok(destination)
}
