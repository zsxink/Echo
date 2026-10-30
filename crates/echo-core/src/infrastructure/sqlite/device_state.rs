//! Stable, single device identity (migration 0006 `device_state`).
//!
//! Every portable record's `updated_by_device_id` is stamped from this value.
//! It is generated once on first open, persisted, and never regenerated — a
//! device that loses its identity would break cross-device LWW ordering. It is
//! local-only and must never be serialized into the portable `echo/` control
//! surface (which is shared across devices).

use rusqlite::{params, Connection, OptionalExtension};

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
