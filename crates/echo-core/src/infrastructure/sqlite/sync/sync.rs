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
//! The object's local sync revision advances in `sync_local_highwater` inside
//! the caller's transaction; pruning outbox rows can therefore never rewind a
//! revision. `playlists`/`library_roots`/`song_overrides` mirror that durable
//! high-water while their existing `songs.revision` remains independent.
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

use rusqlite::{Connection, OptionalExtension, params};

use crate::domain::ids::{PlaylistId, SongId};
use crate::error::Error;

use super::super::support::{now_ms, storage};

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
pub(crate) const KIND_PLAY_STATS: &str = "play-stats";
// KIND_TOMBSTONE is used by 二期 routing and the tombstone tests; 0.1.0 keeps
// song/playlist tombstones via `write_tombstone`'s kind argument.
#[allow(dead_code)]
pub(crate) const KIND_TOMBSTONE: &str = "tombstone";

/// The outbox *operation* label. `upsert` covers create/update; `delete` is
/// only ever the durable Echo-delete (a tombstone), never a plain erase.
pub(crate) const OP_UPSERT: &str = "upsert";
pub(crate) const OP_DELETE: &str = "delete";

/// The digest an enqueue would record for this exact change: kind, object,
/// operation, payload, and declared resources. Two equal digests mean the
/// queued candidate already carries this exact state.
pub(crate) fn snapshot_digest(
    kind: &str,
    object_uuid: &str,
    payload_json: &str,
    operation: &str,
    resource_refs_json: &str,
) -> String {
    let mut digest = blake3::Hasher::new();
    digest.update(kind.as_bytes());
    digest.update(&[0]);
    digest.update(object_uuid.as_bytes());
    digest.update(&[0]);
    digest.update(operation.as_bytes());
    digest.update(&[0]);
    digest.update(payload_json.as_bytes());
    if !resource_refs_json.is_empty() {
        digest.update(&[0]);
        digest.update(resource_refs_json.as_bytes());
    }
    digest.finalize().to_hex().to_string()
}

fn latest_mutation_digest(
    connection: &Connection,
    kind: &str,
    object_uuid: &str,
) -> Result<Option<(i64, String, String)>, Error> {
    connection
        .query_row(
            "SELECT local_revision, payload_digest, status FROM sync_mutations WHERE object_type = ?1 AND object_uuid = ?2 ORDER BY local_revision DESC LIMIT 1",
            params![kind, object_uuid],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(storage)
}

/// Enqueue a song snapshot unless the object's latest non-rejected mutation
/// already carries this exact digest. An unchanged rescan replays the same
/// snapshot; the queued candidate already represents that state, so minting
/// another one would advance the revision without a change (local-library
/// delta: an unchanged rescan adds no revision, play count, or candidate).
/// A rejected tail keeps minting so the repair path stays open.
pub(crate) fn enqueue_song_snapshot_if_changed(
    connection: &Connection,
    object_uuid: &str,
    payload_json: &str,
) -> Result<i64, Error> {
    let local_root = super::context::object_root(connection, KIND_SONG, object_uuid, payload_json)?;
    let resources = super::manifests::load(connection, KIND_SONG, object_uuid)?;
    let resource_refs_json = serde_json::to_string(&resources).map_err(storage)?;
    let digest = snapshot_digest(
        KIND_SONG,
        object_uuid,
        payload_json,
        OP_UPSERT,
        &resource_refs_json,
    );
    if let Some((revision, queued_digest, status)) =
        latest_mutation_digest(connection, KIND_SONG, object_uuid)?
    {
        if status != "rejected" && queued_digest == digest {
            return Ok(revision);
        }
    }
    enqueue_sync_in_root_with_resources(
        connection,
        &local_root,
        KIND_SONG,
        object_uuid,
        payload_json,
        OP_UPSERT,
        None,
        &resources,
    )
}

/// Append one outbox row inside the caller's transaction. Called from the
/// statements write paths so the canonical change and its sync fact commit
/// atomically. `payload_json` is the full object snapshot (see module doc).
///
/// Revision comes from a dedicated persistent high-water, never from outbox
/// history. The caller's SQLite transaction makes the high-water increment
/// atomic with the canonical object write and outbox append.
pub(crate) fn enqueue_sync(
    connection: &Connection,
    kind: &str,
    object_uuid: &str,
    payload_json: &str,
    operation: &str,
) -> Result<i64, Error> {
    enqueue_sync_with_id(connection, kind, object_uuid, payload_json, operation, None)
}

/// Enqueue a local fact under an identity allocated with the original delete
/// intent when one exists. This lets crash recovery finalize the exact same
/// delete mutation rather than allocating a fresh event ID.
pub(crate) fn enqueue_sync_with_id(
    connection: &Connection,
    kind: &str,
    object_uuid: &str,
    payload_json: &str,
    operation: &str,
    stable_mutation_id: Option<&str>,
) -> Result<i64, Error> {
    enqueue_sync_at(
        connection,
        kind,
        object_uuid,
        payload_json,
        operation,
        stable_mutation_id,
        None,
    )
}

/// As [`enqueue_sync_with_id`], also freezing the fact timestamps. A retried
/// operation whose `prepared` payload is already saved must replan to the same
/// bytes, so the supplied intent time replaces the retry clock.
pub(crate) fn enqueue_sync_at(
    connection: &Connection,
    kind: &str,
    object_uuid: &str,
    payload_json: &str,
    operation: &str,
    stable_mutation_id: Option<&str>,
    at_ms: Option<u64>,
) -> Result<i64, Error> {
    let local_root = super::context::object_root(connection, kind, object_uuid, payload_json)?;
    enqueue_sync_in_root_at(
        connection,
        &local_root,
        kind,
        object_uuid,
        payload_json,
        operation,
        stable_mutation_id,
        at_ms,
    )
}

/// Recovery may enqueue a final tombstone after its canonical row has already
/// been removed. Its root comes from the archived authority, never UI selection.
pub(super) fn enqueue_sync_in_root(
    connection: &Connection,
    local_root: &str,
    kind: &str,
    object_uuid: &str,
    payload_json: &str,
    operation: &str,
    stable_mutation_id: Option<&str>,
) -> Result<i64, Error> {
    enqueue_sync_in_root_at(
        connection,
        local_root,
        kind,
        object_uuid,
        payload_json,
        operation,
        stable_mutation_id,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn enqueue_sync_in_root_at(
    connection: &Connection,
    local_root: &str,
    kind: &str,
    object_uuid: &str,
    payload_json: &str,
    operation: &str,
    stable_mutation_id: Option<&str>,
    at_ms: Option<u64>,
) -> Result<i64, Error> {
    let resources = if operation == "delete" || kind == "tombstone" {
        Vec::new()
    } else {
        super::manifests::load(connection, kind, object_uuid)?
    };
    enqueue_sync_in_root_with_resources_at(
        connection,
        local_root,
        kind,
        object_uuid,
        payload_json,
        operation,
        stable_mutation_id,
        &resources,
        at_ms,
    )
}

/// Explicit immutable resources for a repair intent; does not replace the
/// confirmed manifest or local song facts.
#[allow(clippy::too_many_arguments)]
pub(super) fn enqueue_sync_in_root_with_resources(
    connection: &Connection,
    local_root: &str,
    kind: &str,
    object_uuid: &str,
    payload_json: &str,
    operation: &str,
    stable_mutation_id: Option<&str>,
    resource_refs: &[crate::domain::sync::SharedResourceManifest],
) -> Result<i64, Error> {
    enqueue_sync_in_root_with_resources_at(
        connection,
        local_root,
        kind,
        object_uuid,
        payload_json,
        operation,
        stable_mutation_id,
        resource_refs,
        None,
    )
}

#[allow(clippy::too_many_arguments)]
pub(super) fn enqueue_sync_in_root_with_resources_at(
    connection: &Connection,
    local_root: &str,
    kind: &str,
    object_uuid: &str,
    payload_json: &str,
    operation: &str,
    stable_mutation_id: Option<&str>,
    resource_refs: &[crate::domain::sync::SharedResourceManifest],
    at_ms: Option<u64>,
) -> Result<i64, Error> {
    let changed_at = at_ms.map_or_else(now_ms, |ms| i64::try_from(ms).unwrap_or(i64::MAX));
    super::manifests::validate(resource_refs)?;
    connection
        .execute(
            "INSERT INTO sync_local_highwater (object_type, object_uuid, local_revision) VALUES (?1, ?2, 1) ON CONFLICT(object_type, object_uuid) DO UPDATE SET local_revision = local_revision + 1",
            params![kind, object_uuid],
        )
        .map_err(storage)?;
    let next: i64 = connection
        .query_row(
            "SELECT local_revision FROM sync_local_highwater WHERE object_type = ?1 AND object_uuid = ?2",
            params![kind, object_uuid],
            |row| row.get(0),
        )
        .map_err(storage)?;
    if super::durability::sync_namespace(connection, local_root)?.is_none() {
        // Keep the per-object revision needed by local portable records, but
        // do not create a sync mutation/outbox entry for standalone edits.
        return Ok(next);
    }
    let origin_device_id: String = connection
        .query_row(
            "SELECT value FROM device_state WHERE key = 'device_id'",
            [],
            |row| row.get(0),
        )
        .map_err(storage)?;
    let cursor: Option<(i64, String)> = connection
        .query_row(
            "SELECT origin_seq, last_mutation_id FROM sync_local_origin_cursors WHERE origin_device_id = ?1 AND local_root_uuid = ?2",
            params![origin_device_id, local_root],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(storage)?;
    let origin_seq = cursor.as_ref().map_or(Ok(1), |(seq, _)| {
        seq.checked_add(1).ok_or_else(|| {
            crate::error::Error::validation(
                crate::error::Subject::Other,
                "sync origin sequence",
                "sequence exhausted",
            )
        })
    })?;
    let previous_mutation_id = cursor.map(|(_, mutation_id)| mutation_id);
    let mutation_id =
        stable_mutation_id.map_or_else(|| uuid::Uuid::new_v4().to_string(), str::to_owned);
    let base_object_version: Option<i64> = connection
        .query_row(
            "SELECT object_version FROM sync_object_versions WHERE object_type = ?1 AND object_uuid = ?2 AND is_tombstone = 0 UNION ALL SELECT object_version FROM archived_sync_object_versions WHERE context_root = ?3 AND object_type = ?1 AND object_uuid = ?2 AND is_tombstone = 0 LIMIT 1",
            params![kind, object_uuid, local_root],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage)?;
    let resource_refs_json = serde_json::to_string(&resource_refs).map_err(storage)?;
    let payload_digest = snapshot_digest(
        kind,
        object_uuid,
        payload_json,
        operation,
        &resource_refs_json,
    );
    connection
        .execute(
            "INSERT INTO sync_mutations (mutation_id, origin_device_id, origin_seq, previous_mutation_id, object_type, object_uuid, operation, local_revision, base_object_version, payload_json, payload_digest, status, created_at, resource_refs_json, local_root_uuid) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'pending', ?12, ?13, ?14)",
            params![mutation_id, origin_device_id, origin_seq, previous_mutation_id, kind, object_uuid, operation, next, base_object_version, payload_json, payload_digest, changed_at, resource_refs_json, local_root],
        )
        .map_err(storage)?;
    connection
        .execute(
            "INSERT INTO sync_local_origin_cursors (origin_device_id, origin_seq, last_mutation_id, local_root_uuid) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(local_root_uuid,origin_device_id) DO UPDATE SET origin_seq = excluded.origin_seq, last_mutation_id = excluded.last_mutation_id",
            params![origin_device_id, origin_seq, mutation_id, local_root],
        )
        .map_err(storage)?;
    connection
        .execute(
            "INSERT INTO sync_outbox (object_type, object_uuid, revision, operation, payload_json, created_at, pushed_at, mutation_id, local_root_uuid) VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7, ?8) ON CONFLICT(object_type, object_uuid, revision) DO NOTHING",
            params![kind, object_uuid, next, operation, payload_json, changed_at, mutation_id, local_root],
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

/// The object's durable local sync revision, with outbox history as a
/// defensive fallback for pre-ledger migration callers.
pub(crate) fn current_outbox_revision(
    connection: &Connection,
    kind: &str,
    object_uuid: &str,
) -> Result<i64, Error> {
    connection
        .query_row(
            "SELECT COALESCE((SELECT local_revision FROM sync_local_highwater WHERE object_type = ?1 AND object_uuid = ?2), (SELECT MAX(revision) FROM sync_outbox WHERE object_type = ?1 AND object_uuid = ?2), 0)",
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
