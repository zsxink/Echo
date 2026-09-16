//! Sync-foundation local writes (方向 A, tasks 3.10–3.14).
//!
//! This module turns the inert tables from `0005_sync_foundation.sql` into a
//! *shape-ready* store: every local logic change that二期 will treat as a
//! syncable fact writes a `sync_outbox` row and reaches a monotone object
//! `revision` in the SAME transaction as the canonical write. It deliberately
//! performs no networking, no push, no conflict resolution and no sync UI —
//! the offline boundary stays. 二期 consumes `outbox` rows and `revision` as-is.
//!
//! Object kinds (五类逻辑对象, design §3-5 + sync-foundation spec):
//!   - `song`     — import/upsert + availability/hide
//!   - `favorite` — the independent per-song favorite fact
//!   - `playlist` — create/rename/delete + membership add/remove
//!   - `override` — song_overrides revision (written by future override cases)
//!   - `tombstone`— Echo-initiated durable deletion record
//!
//! The object's *sync revision* is derived from its outbox history (`MAX(
//! outbox.revision)+1`), NOT from `songs.revision` — that column stays the
//! optimistic-concurrency / event-ordering tag from the persistence layer and
//! is intentionally not overloaded. `playlists`/`library_roots`/`song_overrides`
//! mirror the outbox-derived value so a later-phase reader can trust the column.
//!
//! Payloads are the FULL object snapshot in JSON (never an incremental diff):
//! the receiving side just overwrites on 最后写入者胜. Payloads carry only
//! object UUID + root-relative path + object fields — never absolute paths,
//! database paths, credentials or network endpoints (local-library privacy).

#![allow(
    clippy::doc_markdown,
    clippy::missing_errors_doc,
    clippy::needless_pass_by_value,
    clippy::redundant_pub_crate,
    clippy::uninlined_format_args
)]

use rusqlite::{params, Connection, OptionalExtension};

use crate::domain::ids::{PlaylistId, SongId};
use crate::error::Error;

use super::support::{now_ms, storage};

/// Outbox object kinds (二期 payload routing keys).
pub(crate) const KIND_SONG: &str = "song";
/// A favorite is an independent portable object keyed by its song UUID. This
/// keeps a favorite toggle from overwriting the song object's media metadata.
pub(crate) const KIND_FAVORITE: &str = "favorite";
pub(crate) const KIND_PLAYLIST: &str = "playlist";
/// A single playlist membership, identified independently from both its
/// playlist and song so a removal can propagate without deleting either.
pub(crate) const KIND_PLAYLIST_ITEM: &str = "playlist-item";
pub(crate) const KIND_OVERRIDE: &str = "override";
// KIND_TOMBSTONE is used by 二期 routing and the tombstone tests; 0.1.0 keeps
// song/playlist tombstones via `write_tombstone`'s kind argument.
#[allow(dead_code)]
pub(crate) const KIND_TOMBSTONE: &str = "tombstone";

/// The outbox *operation* label. `upsert` covers create/update; `delete` is
/// only ever the durable Echo-delete (a tombstone), never a plain erase.
pub(crate) const OP_UPSERT: &str = "upsert";
pub(crate) const OP_DELETE: &str = "delete";

/// Append one outbox row inside the caller's transaction. Called from the
/// statements write paths so the canonical change and its sync fact commit
/// atomically. `payload_json` is the full object snapshot (see module doc).
///
/// Revision is the object's next monotone ordinal derived from its outbox
/// history; the unique `(object_type, object_uuid, revision)` index makes a
/// repeated write idempotent rather than a duplicate row (best-effort: a
/// recovery replay that already enqueued the same ordinal becomes a no-op).
pub(crate) fn enqueue_sync(
    connection: &Connection,
    kind: &str,
    object_uuid: &str,
    payload_json: &str,
    operation: &str,
) -> Result<i64, Error> {
    let next: i64 = connection
        .query_row(
            "SELECT COALESCE(MAX(revision) + 1, 1) FROM sync_outbox WHERE object_type = ?1 AND object_uuid = ?2",
            params![kind, object_uuid],
            |row| row.get(0),
        )
        .map_err(storage)?;
    connection
        .execute(
            "INSERT INTO sync_outbox (object_type, object_uuid, revision, operation, payload_json, created_at, pushed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL) ON CONFLICT(object_type, object_uuid, revision) DO NOTHING",
            params![kind, object_uuid, next, operation, payload_json, now_ms()],
        )
        .map_err(storage)?;
    Ok(next)
}

/// Bounce a `playlists.revision` to the object's outbox ordinal & return it.
pub(crate) fn mirror_playlist_revision(
    connection: &Connection,
    id: PlaylistId,
    revision: i64,
) -> Result<(), Error> {
    connection
        .execute(
            "UPDATE playlists SET revision = ?2, updated_at = ?3 WHERE uuid = ?1",
            params![id.to_string(), revision, now_ms()],
        )
        .map_err(storage)?;
    Ok(())
}

/// Bounce a `song_overrides.revision`.
pub(crate) fn mirror_override_revision(
    connection: &Connection,
    song: SongId,
    revision: i64,
) -> Result<(), Error> {
    connection
        .execute(
            "UPDATE song_overrides SET revision = ?2 WHERE song_uuid = ?1",
            params![song.to_string(), revision],
        )
        .map_err(storage)?;
    Ok(())
}

/// Read one `sync_state` key (二期 connector config / cursor / schema marker).
pub(crate) fn load_sync_state(connection: &Connection, key: &str) -> Result<Option<String>, Error> {
    connection
        .query_row(
            "SELECT value FROM sync_state WHERE key = ?1",
            params![key],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage)
}

/// Number of outbox rows of one object kind (diagnostics/二期 read; 0.1.0
/// never consumes it for a push).
pub(crate) fn outbox_count(connection: &Connection, kind: &str) -> Result<i64, Error> {
    connection
        .query_row(
            "SELECT COUNT(*) FROM sync_outbox WHERE object_type = ?1",
            params![kind],
            |row| row.get(0),
        )
        .map_err(storage)
}

/// The object's current outbox ordinal (`MAX(revision)`) or 0 when untouched.
pub(crate) fn current_outbox_revision(
    connection: &Connection,
    kind: &str,
    object_uuid: &str,
) -> Result<i64, Error> {
    connection
        .query_row(
            "SELECT COALESCE(MAX(revision), 0) FROM sync_outbox WHERE object_type = ?1 AND object_uuid = ?2",
            params![kind, object_uuid],
            |row| row.get(0),
        )
        .map_err(storage)
}

/// Write/read the durable tombstone row for an Echo-initiated deletion. The
/// tombstone itself is a syncable object (design §3 删除/墓碑): it must exist
/// so a delete on one device propagates to the others. `object_uuid` is the
/// deleted object's UUID; `kind` is one of `KIND_*`. Idempotent under replay.
pub(crate) fn write_tombstone(
    connection: &Connection,
    kind: &str,
    object_uuid: &str,
    revision: i64,
) -> Result<(), Error> {
    connection
        .execute(
            "INSERT INTO tombstones (object_type, object_uuid, revision, deleted_at) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(object_type, object_uuid) DO UPDATE SET revision = excluded.revision, deleted_at = excluded.deleted_at",
            params![kind, object_uuid, revision, now_ms()],
        )
        .map_err(storage)?;
    Ok(())
}

// NOTE: a future `tombstones` read helper (reconcile inbound tombstones against
// local state) belongs to the 二期 sync engine; 0.1.0 has no reader.
