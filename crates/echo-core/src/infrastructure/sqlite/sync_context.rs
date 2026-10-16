//! Atomic selection of one local root's authority-owned protocol history.
//! Local mutations are stored by root independently of this selected context,
//! so scanning a candidate never exchanges the running host's ledger.

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use super::support::storage;
use super::SqliteDatabase;
use crate::domain::ids::LibraryRootId;
use crate::error::Error;

// Child tables precede their parents. Restore uses the reverse order.
const TABLES: &[&str] = &[
    "sync_snapshot_pages",
    "sync_snapshot_resource_refs",
    "sync_snapshot_records",
    "sync_snapshot_sessions",
    "sync_replica_snapshot_pages",
    "sync_replica_snapshot_records",
    "sync_replica_snapshot_sessions",
    "sync_host_baselines",
    "sync_transfers",
    "sync_peer_cursors",
    "sync_conflicts",
    "sync_member_aliases",
    "sync_song_aliases",
    "sync_object_versions",
    "sync_commits",
    "sync_inbox",
    "sync_origin_cursors",
    "sync_peers",
    "sync_binding",
];

impl SqliteDatabase {
    /// Select a root after its previous sync workers have stopped. The writer
    /// transaction retains every previous context and exposes no intermediate
    /// state to readers. Root activation also calls this in its own transaction.
    pub fn select_sync_context(&self, root: LibraryRootId) -> Result<(), Error> {
        self.writer.run(move |connection| {
            let tx = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;
            select(&tx, &root.to_string())?;
            tx.commit().map_err(storage)
        })
    }
}

pub(super) fn select(connection: &Connection, root: &str) -> Result<(), Error> {
    let exists: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM library_roots WHERE uuid=?1)",
            params![root],
            |row| row.get(0),
        )
        .map_err(storage)?;
    if !exists {
        return Err(Error::conflict(
            "sync context requires an existing local library root",
        ));
    }
    let previous: String = connection
        .query_row(
            "SELECT local_root_uuid FROM sync_active_context WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .map_err(storage)?;
    if previous == root {
        return Ok(());
    }
    for table in TABLES {
        connection
            .execute(
                &format!("DELETE FROM archived_{table} WHERE context_root=?1"),
                params![previous],
            )
            .map_err(storage)?;
        connection
            .execute(
                &format!("INSERT INTO archived_{table} SELECT ?1, live.* FROM {table} live"),
                params![previous],
            )
            .map_err(storage)?;
    }
    for table in TABLES {
        connection
            .execute(&format!("DELETE FROM {table}"), [])
            .map_err(storage)?;
    }
    for table in TABLES.iter().rev() {
        let mut statement = connection
            .prepare(&format!("PRAGMA table_info({table})"))
            .map_err(storage)?;
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))
            .map_err(storage)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(storage)?
            .join(",");
        connection.execute(&format!("INSERT INTO {table}({columns}) SELECT {columns} FROM archived_{table} WHERE context_root=?1"), params![root]).map_err(storage)?;
    }
    // Archived copies of the live context would become stale during writes.
    for table in TABLES {
        connection
            .execute(
                &format!("DELETE FROM archived_{table} WHERE context_root=?1"),
                params![root],
            )
            .map_err(storage)?;
    }
    connection
        .execute(
            "UPDATE sync_active_context SET local_root_uuid=?1 WHERE singleton=1",
            params![root],
        )
        .map_err(storage)?;
    Ok(())
}

/// Resolve before deleting canonical rows. UUIDs are globally unique; root
/// scope comes from canonical ownership, never from the currently selected UI.
pub(super) fn object_root(
    connection: &Connection,
    kind: &str,
    id: &str,
    payload: &str,
) -> Result<String, Error> {
    let query = match kind {
        "song" | "favorite" | "override" | "play-stats" | "tombstone" => "SELECT library_root_uuid FROM songs WHERE uuid=?1",
        "playlist" => "SELECT library_root_uuid FROM playlists WHERE uuid=?1",
        "playlist-item" => "SELECT p.library_root_uuid FROM playlist_songs m JOIN playlists p ON p.uuid=m.playlist_uuid WHERE m.member_uuid=?1",
        "library-root" => return Ok(id.to_owned()),
        _ => return Err(Error::conflict("unknown local sync object kind")),
    };
    if let Some(root) = connection
        .query_row(query, params![id], |row| row.get::<_, String>(0))
        .optional()
        .map_err(storage)?
    {
        return Ok(root);
    }
    // Membership removal snapshots retain the owning playlist after the row
    // itself is deleted. Other local objects must still exist when enqueued.
    if kind == "playlist-item" {
        let payload: serde_json::Value = serde_json::from_str(payload).map_err(storage)?;
        if let Some(playlist) = payload["playlist_uuid"].as_str() {
            return connection
                .query_row(
                    "SELECT library_root_uuid FROM playlists WHERE uuid=?1",
                    params![playlist],
                    |row| row.get(0),
                )
                .map_err(storage);
        }
    }
    Err(Error::conflict(
        "local sync object has no owning library root",
    ))
}
