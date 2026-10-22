//! Durable retry markers for retiring a detached binding's disposable cache.
//!
//! The transfer cache lives outside SQLite (per-binding directory partitions),
//! so its removal cannot share a transaction with `detach_lan_sync_context`.
//! Writing the marker before the binding identity is discarded makes a failed
//! directory removal retryable on the next session instead of leaking orphan
//! bytes forever. It deliberately records nothing about publication journals or
//! security checkpoints: those are never reclaimed by ordinary session cleanup.

use rusqlite::{params, OptionalExtension};

use super::super::support::{now_ms, storage};
use super::super::SqliteDatabase;
use crate::domain::sync::SyncBinding;
use crate::error::Error;

impl SqliteDatabase {
    /// Record (or refresh) the cleanup intent for one retired binding.
    ///
    /// # Errors
    ///
    /// Storage failures propagate; callers must not discard the binding on
    /// failure without keeping their own recovery evidence.
    pub fn record_sync_cache_cleanup(&self, binding: SyncBinding) -> Result<(), Error> {
        let encoded = serde_json::to_string(&binding).map_err(storage)?;
        let key = cache_cleanup_key(binding)?;
        self.writer.run(move |connection| {
            connection
                .execute(
                    "INSERT INTO sync_cache_cleanups(binding_key,binding_json,attempts,created_at) VALUES(?1,?2,0,?3)\n                     ON CONFLICT(binding_key) DO UPDATE SET binding_json=excluded.binding_json",
                    params![key, encoded, now_ms()],
                )
                .map_err(storage)?;
            Ok(())
        })
    }

    /// Pending cleanup intents, oldest first. Each entry is decoded before it
    /// is returned so an unreadable row is reported rather than skipped.
    ///
    /// # Errors
    ///
    /// Storage and decode failures propagate.
    pub fn pending_sync_cache_cleanups(&self) -> Result<Vec<SyncBinding>, Error> {
        self.with_reader(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT binding_json FROM sync_cache_cleanups ORDER BY created_at,binding_key",
                )
                .map_err(storage)?;
            let rows = statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(storage)?;
            rows.map(|row| {
                let encoded = row.map_err(storage)?;
                serde_json::from_str::<SyncBinding>(&encoded).map_err(|_| {
                    Error::InvariantViolation {
                        why: "invalid pending sync cache cleanup marker".to_owned(),
                    }
                })
            })
            .collect()
        })
    }

    /// Drop one cleanup marker after its partition is verifiably gone.
    ///
    /// # Errors
    ///
    /// Storage failures propagate.
    pub fn complete_sync_cache_cleanup(&self, binding: SyncBinding) -> Result<(), Error> {
        let key = cache_cleanup_key(binding)?;
        self.writer.run(move |connection| {
            connection
                .execute(
                    "DELETE FROM sync_cache_cleanups WHERE binding_key=?1",
                    params![key],
                )
                .map_err(storage)?;
            Ok(())
        })
    }

    /// Count a failed removal attempt without discarding the retry marker.
    ///
    /// # Errors
    ///
    /// Storage failures propagate.
    pub fn retry_sync_cache_cleanup(&self, binding: SyncBinding) -> Result<u64, Error> {
        let key = cache_cleanup_key(binding)?;
        self.writer.run(move |connection| {
            connection
                .execute(
                    "UPDATE sync_cache_cleanups SET attempts=attempts+1 WHERE binding_key=?1",
                    params![key],
                )
                .map_err(storage)?;
            let attempts: Option<i64> = connection
                .query_row(
                    "SELECT attempts FROM sync_cache_cleanups WHERE binding_key=?1",
                    params![key],
                    |row| row.get(0),
                )
                .optional()
                .map_err(storage)?;
            Ok(attempts.map_or(0, |attempts| u64::try_from(attempts).unwrap_or(0)))
        })
    }
}

/// Stable, filesystem-safe identity of one binding's cleanup marker.
fn cache_cleanup_key(binding: SyncBinding) -> Result<String, Error> {
    let encoded = serde_json::to_vec(&binding).map_err(storage)?;
    Ok(blake3::hash(&encoded).to_hex().to_string())
}
