//! Stable, single device identity (migration 0006 `device_state`).
//!
//! Every portable record's `updated_by_device_id` is stamped from this value.
//! It is generated once on first open, persisted, and never regenerated — a
//! device that loses its identity would break cross-device LWW ordering. It is
//! local-only and must never be serialized into the portable `echo/` control
//! surface (which is shared across devices).

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use crate::domain::library::DeviceId;
use crate::error::Error;

use super::support::storage;

/// The `device_state` key holding the device id.
pub const DEVICE_ID_KEY: &str = "device_id";

/// Read the persisted device id, minting and storing a fresh one the first time
/// (when the row is absent or the seeded `'uninitialized'` marker). Runs on the
/// writer connection so the value cannot race with a concurrent reset.
///
/// # Errors
///
/// Propagates a `Storage` failure while reading or writing `device_state`.
pub fn load_or_create_device_id(connection: &Connection) -> Result<DeviceId, Error> {
    let value: Option<String> = connection
        .query_row(
            "SELECT value FROM device_state WHERE key = ?1",
            params![DEVICE_ID_KEY],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage)?;
    match value {
        // Uninitialized (the migration's seed) or missing → mint + persist.
        Some(v) if v == "uninitialized" || v.is_empty() => {
            let device_id = DeviceId::new();
            persist(connection, device_id)?;
            Ok(device_id)
        }
        Some(v) => v
            .parse::<DeviceId>()
            .map_err(|_| Error::InvariantViolation {
                why: "device_state carries an invalid device id".to_owned(),
            }),
        None => {
            let device_id = DeviceId::new();
            persist(connection, device_id)?;
            Ok(device_id)
        }
    }
}

/// Assign pre-sync aggregate play counts to this stable device exactly once.
/// Later projections can then merge per-device maxima without erasing counts
/// that existed before the device-bucket table was introduced.
pub fn backfill_legacy_play_counts(
    connection: &mut Connection,
    device_id: DeviceId,
) -> Result<(), Error> {
    let tx = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(storage)?;
    let backfilled: Option<String> = tx
        .query_row(
            "SELECT value FROM sync_state WHERE key='legacy_play_counts_backfilled'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage)?;
    if backfilled.is_none() {
        tx.execute(
            "INSERT OR IGNORE INTO sync_device_play_counts(song_uuid, device_id, play_count)
             SELECT uuid, ?1, play_count FROM songs
             WHERE play_count > 0
               AND NOT EXISTS(SELECT 1 FROM sync_device_play_counts c WHERE c.song_uuid=songs.uuid)",
            params![device_id.to_string()],
        )
        .map_err(storage)?;
        tx.execute(
            "INSERT INTO sync_state(key, value) VALUES('legacy_play_counts_backfilled', '1')",
            [],
        )
        .map_err(storage)?;
    }
    tx.commit().map_err(storage)
}

fn persist(connection: &Connection, device_id: DeviceId) -> Result<(), Error> {
    connection
        .execute(
            "INSERT INTO device_state (key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![DEVICE_ID_KEY, device_id.to_string()],
        )
        .map_err(storage)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_play_counts_are_backfilled_once_to_the_stable_device() {
        let mut connection = Connection::open_in_memory().expect("connection");
        connection
            .execute_batch(
                "CREATE TABLE songs(uuid TEXT PRIMARY KEY, play_count INTEGER NOT NULL);
                 CREATE TABLE sync_device_play_counts(song_uuid TEXT, device_id TEXT, play_count INTEGER, PRIMARY KEY(song_uuid, device_id));
                 CREATE TABLE sync_state(key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 INSERT INTO songs VALUES('song', 12);",
            )
            .expect("schema");
        let device = DeviceId::new();
        backfill_legacy_play_counts(&mut connection, device).expect("backfill");
        let bucket: (String, i64) = connection
            .query_row(
                "SELECT device_id, play_count FROM sync_device_play_counts WHERE song_uuid='song'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("bucket");
        assert_eq!(bucket, (device.to_string(), 12));
        connection
            .execute("UPDATE songs SET play_count=20 WHERE uuid='song'", [])
            .expect("simulate later aggregate change");
        backfill_legacy_play_counts(&mut connection, DeviceId::new()).expect("idempotent");
        let count: i64 = connection
            .query_row(
                "SELECT play_count FROM sync_device_play_counts WHERE song_uuid='song'",
                [],
                |row| row.get(0),
            )
            .expect("unchanged bucket");
        assert_eq!(count, 12);
    }
}
