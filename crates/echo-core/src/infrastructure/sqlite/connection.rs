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
    pub const INITIAL: &str = include_str!("migrations/0001_initial.sql");
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
    apply_migration_set(connection, &[(1, migrations::INITIAL)])
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
            Some(applied) if applied == checksum => continue,
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
