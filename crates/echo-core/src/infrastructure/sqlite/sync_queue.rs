//! Durable access to queued local mutations and replica cursors.

#![allow(clippy::doc_markdown, clippy::missing_errors_doc)]

use rusqlite::{params, OptionalExtension};
use uuid::Uuid;

use crate::application::ports::LocalSyncQueuePort;
use crate::domain::ids::{PlaylistId, PlaylistItemId, SongId};
use crate::domain::sync::{
    CommitCheckpoint, CommitSeq, DeviceId, MutationCandidate, MutationId, ObjectKey, ObjectVersion,
    OriginAnchor, SyncBinding,
};
use crate::error::{Error, Subject};

use super::support::storage;
use super::SqliteDatabase;

const MAX_BATCH: usize = 512;

impl SqliteDatabase {
    /// Exact pending queue size and unique immutable resource declarations for
    /// the selected root. Unlike a transport batch, this query is not capped.
    ///
    /// # Errors
    /// Reports unavailable storage or malformed persisted resource declarations.
    pub fn pending_local_sync_summary(
        &self,
    ) -> Result<(u64, Vec<crate::domain::sync::SharedResourceManifest>), Error> {
        self.with_reader(|connection| {
            let mut statement = connection.prepare(
                "SELECT resource_refs_json FROM sync_mutations WHERE origin_device_id=(SELECT value FROM device_state WHERE key='device_id') AND local_root_uuid=(SELECT local_root_uuid FROM sync_active_context WHERE singleton=1) AND status='pending'",
            ).map_err(storage)?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0)).map_err(storage)?;
            let mut count = 0;
            let mut resources = std::collections::BTreeMap::new();
            for row in rows {
                count += 1;
                let declarations: Vec<crate::domain::sync::SharedResourceManifest> =
                    serde_json::from_str(&row.map_err(storage)?).map_err(storage)?;
                for resource in declarations {
                    resources.entry(resource.digest.to_hex()).or_insert(resource);
                }
            }
            Ok((count, resources.into_values().collect()))
        })
    }

    /// This device's confirmed position in its own origin chain.
    ///
    /// The host confirms its own edits through the same ledger a peer's edits
    /// go through, so this is the authority its local queue must line up with.
    ///
    /// # Errors
    /// Reports unavailable storage or a malformed persisted cursor.
    pub fn local_origin_anchor(&self) -> Result<OriginAnchor, Error> {
        self.with_reader(|connection| {
            let device: String = connection
                .query_row(
                    "SELECT value FROM device_state WHERE key='device_id'",
                    [],
                    |row| row.get(0),
                )
                .map_err(storage)?;
            let cursor: Option<(i64, Option<String>)> = connection
                .query_row(
                    "SELECT origin_seq, last_mutation_id FROM sync_origin_cursors WHERE origin_device_id=?1",
                    params![device],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(storage)?;
            let Some((sequence, last)) = cursor else {
                return Ok(OriginAnchor::GENESIS);
            };
            Ok(OriginAnchor {
                origin_seq: u64::try_from(sequence).map_err(storage)?,
                last_mutation_id: last
                    .map(|id| {
                        Uuid::parse_str(&id)
                            .map(MutationId::from_uuid)
                            .map_err(storage)
                    })
                    .transpose()?,
            })
        })
    }
}

impl LocalSyncQueuePort for SqliteDatabase {
    fn local_mutation_changed_at_ms(
        &self,
        mutation_id: crate::domain::sync::MutationId,
    ) -> Result<Option<u64>, Error> {
        self.with_reader(|connection| {
            let changed_at: Option<i64> = connection
                .query_row(
                    "SELECT created_at FROM sync_mutations WHERE mutation_id=?1 AND local_root_uuid=(SELECT local_root_uuid FROM sync_active_context WHERE singleton=1)",
                    params![mutation_id.as_uuid().to_string()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(storage)?;
            changed_at
                .map(|value| u64::try_from(value).map_err(storage))
                .transpose()
        })
    }

    fn advance_rejected_local_mutations(&self) -> Result<usize, Error> {
        self.writer.run(|connection| {
            let tx = connection
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(storage)?;
            let local_device: String = tx
                .query_row(
                    "SELECT value FROM device_state WHERE key='device_id'",
                    [],
                    |row| row.get(0),
                )
                .map_err(storage)?;
            let mut cursor: Option<(i64, Option<String>)> = tx
                .query_row(
                    "SELECT origin_seq,last_mutation_id FROM sync_origin_cursors WHERE origin_device_id=?1",
                    params![local_device],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(storage)?;
            let mut statement = tx
                .prepare(
                    "SELECT mutation_id,origin_seq,previous_mutation_id FROM sync_mutations WHERE origin_device_id=?1 AND local_root_uuid=(SELECT local_root_uuid FROM sync_active_context WHERE singleton=1) AND status='rejected' ORDER BY origin_seq",
                )
                .map_err(storage)?;
            let rejected = statement
                .query_map(params![local_device], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                })
                .map_err(storage)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage)?;
            drop(statement);

            let mut advanced = 0;
            for (mutation_id, sequence, predecessor) in rejected {
                let expected_sequence = cursor
                    .as_ref()
                    .map_or(1, |(sequence, _)| sequence.saturating_add(1));
                let expected_predecessor = cursor.as_ref().and_then(|(_, id)| id.as_deref());
                if sequence != expected_sequence || predecessor.as_deref() != expected_predecessor {
                    continue;
                }
                tx.execute(
                    "INSERT INTO sync_origin_cursors(origin_device_id,origin_seq,last_mutation_id) VALUES(?1,?2,?3) ON CONFLICT(origin_device_id) DO UPDATE SET origin_seq=excluded.origin_seq,last_mutation_id=excluded.last_mutation_id",
                    params![local_device, sequence, mutation_id],
                )
                .map_err(storage)?;
                cursor = Some((sequence, Some(mutation_id)));
                advanced += 1;
            }
            tx.commit().map_err(storage)?;
            Ok(advanced)
        })
    }

    fn pending_local_mutations(&self, limit: usize) -> Result<Vec<MutationCandidate>, Error> {
        if !(1..=MAX_BATCH).contains(&limit) {
            return Err(Error::validation(
                Subject::Other,
                "sync mutation batch",
                "batch limit must be between 1 and 512",
            ));
        }
        self.with_reader(|connection| {
            let local_device: String = connection
                .query_row(
                    "SELECT value FROM device_state WHERE key='device_id'",
                    [],
                    |row| row.get(0),
                )
                .map_err(storage)?;
            let mut statement = connection
                .prepare(
                    "SELECT m.mutation_id, m.origin_seq, m.previous_mutation_id, m.object_type, m.object_uuid, m.base_object_version, m.payload_json, m.resource_refs_json FROM sync_mutations m WHERE m.origin_device_id=?1 AND m.local_root_uuid=(SELECT local_root_uuid FROM sync_active_context WHERE singleton=1) AND m.status='pending' ORDER BY m.origin_seq LIMIT ?2",
                )
                .map_err(storage)?;
            let rows = statement
                .query_map(params![local_device, i64::try_from(limit).map_err(storage)?], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<String>>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, String>(4)?,
                        row.get::<_, Option<i64>>(5)?,
                        row.get::<_, String>(6)?,
                        row.get::<_, String>(7)?,
                    ))
                })
                .map_err(storage)?;
            rows.map(|row| {
                decode_pending_mutation(row.map_err(storage)?, &local_device)
            })
            .collect()
        })
    }

    fn reject_local_mutation(
        &self,
        mutation_id: MutationId,
        outcome_json: String,
    ) -> Result<(), Error> {
        let outcome = serde_json::from_str::<serde_json::Value>(&outcome_json).map_err(storage)?;
        if outcome_json.len() > 64 * 1024 || !outcome.is_object() {
            return Err(Error::validation(
                Subject::Other,
                "sync mutation outcome",
                "outcome must be a bounded JSON object",
            ));
        }
        self.writer.run(move |connection| {
            let tx = connection
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(storage)?;
            let local_device: String = tx
                .query_row(
                    "SELECT value FROM device_state WHERE key='device_id'",
                    [],
                    |row| row.get(0),
                )
                .map_err(storage)?;
            let pending: Option<(i64, Option<String>)> = tx
                .query_row(
                    "SELECT origin_seq,previous_mutation_id FROM sync_mutations WHERE origin_device_id=?1 AND mutation_id=?2 AND local_root_uuid=(SELECT local_root_uuid FROM sync_active_context WHERE singleton=1) AND status='pending'",
                    params![local_device, mutation_id.as_uuid().to_string()],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(storage)?;
            // Already confirmed or rejected — by an earlier cycle, a retried
            // acknowledgement, or a concurrent drain. There is nothing left to
            // record, and this is not a ledger failure: mapping the absent row
            // to a storage error told the user the local ledger was unreadable
            // and sent every later diagnostic down the wrong path.
            let Some((origin_seq, previous_mutation_id)) = pending else {
                tx.commit().map_err(storage)?;
                return Ok(());
            };
            let changed = tx
                .execute(
                    "UPDATE sync_mutations SET status='rejected', outcome_json=?3 WHERE origin_device_id=?1 AND mutation_id=?2 AND local_root_uuid=(SELECT local_root_uuid FROM sync_active_context WHERE singleton=1) AND status='pending'",
                    params![local_device, mutation_id.as_uuid().to_string(), outcome_json],
                )
                .map_err(storage)?;
            if changed == 1 {
                let cursor: Option<(i64, Option<String>)> = tx
                    .query_row(
                        "SELECT origin_seq,last_mutation_id FROM sync_origin_cursors WHERE origin_device_id=?1",
                        params![local_device],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()
                    .map_err(storage)?;
                let expected_sequence = cursor
                    .as_ref()
                    .map_or(1, |(sequence, _)| sequence.saturating_add(1));
                if origin_seq == expected_sequence
                    && previous_mutation_id.as_deref()
                        == cursor.as_ref().and_then(|(_, id)| id.as_deref())
                {
                    tx.execute(
                        "INSERT INTO sync_origin_cursors(origin_device_id,origin_seq,last_mutation_id) VALUES(?1,?2,?3) ON CONFLICT(origin_device_id) DO UPDATE SET origin_seq=excluded.origin_seq,last_mutation_id=excluded.last_mutation_id",
                        params![local_device, origin_seq, mutation_id.as_uuid().to_string()],
                    )
                    .map_err(storage)?;
                }
                tx.commit().map_err(storage)
            } else {
                Err(Error::conflict("local mutation is missing or already terminal"))
            }
        })
    }

    fn rebase_local_chain(&self, anchor: OriginAnchor) -> Result<usize, Error> {
        self.writer.run(move |connection| {
            let tx = connection
                .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
                .map_err(storage)?;
            let local_device: String = tx
                .query_row(
                    "SELECT value FROM device_state WHERE key='device_id'",
                    [],
                    |row| row.get(0),
                )
                .map_err(storage)?;
            let root: String = tx
                .query_row(
                    "SELECT local_root_uuid FROM sync_active_context WHERE singleton=1",
                    [],
                    |row| row.get(0),
                )
                .map_err(storage)?;
            let mut statement = tx
                .prepare(
                    "SELECT mutation_id FROM sync_mutations WHERE origin_device_id=?1 AND local_root_uuid=?2 AND status='pending' ORDER BY origin_seq, created_at, mutation_id",
                )
                .map_err(storage)?;
            let pending = statement
                .query_map(params![local_device, root], |row| row.get::<_, String>(0))
                .map_err(storage)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage)?;
            drop(statement);

            let mut sequence = anchor.origin_seq;
            let mut previous = anchor.last_mutation_id;
            let mut rebased = 0;
            if !pending.is_empty() {
                // `sync_mutations` is UNIQUE(local_root_uuid, origin_device_id,
                // origin_seq) for *every* status, so renumbering cannot happen
                // in place: assigning `anchor + 1` to the row that already holds
                // that number — or to the one after it, when the target range
                // overlaps the current one — aborts the whole repair with
                // `UNIQUE constraint failed`. Lift the colliding rows above
                // every number this device holds, then write the final values
                // into slots that are now empty. Every step stays an UPDATE, so
                // no audit row is lost and relative order is preserved.
                let occupied: i64 = tx
                    .query_row(
                        "SELECT COALESCE(MAX(origin_seq), 0) FROM sync_mutations WHERE origin_device_id=?1 AND local_root_uuid=?2",
                        params![local_device, root],
                        |row| row.get(0),
                    )
                    .map_err(storage)?;
                let count = i64::try_from(pending.len()).map_err(storage)?;
                let target_end = i64::try_from(anchor.origin_seq)
                    .map_err(storage)?
                    .saturating_add(count);
                let staging = occupied
                    .max(target_end)
                    .saturating_add(count)
                    .saturating_add(1);
                // Only the rows that can collide move: every pending row (they
                // are the ones being reassigned) and any terminal row sitting
                // inside the target range. A retired event outside that range
                // keeps the number it was audited under.
                tx.execute(
                    "UPDATE sync_mutations SET origin_seq = origin_seq + ?5 WHERE origin_device_id=?1 AND local_root_uuid=?2 AND (status='pending' OR origin_seq BETWEEN ?3 AND ?4)",
                    params![
                        local_device,
                        root,
                        i64::try_from(anchor.origin_seq).map_err(storage)?.saturating_add(1),
                        target_end,
                        staging,
                    ],
                )
                .map_err(storage)?;
            }
            for mutation_id in &pending {
                sequence = sequence.checked_add(1).ok_or_else(|| {
                    Error::validation(
                        Subject::Other,
                        "sync origin sequence",
                        "sequence exhausted",
                    )
                })?;
                tx.execute(
                    "UPDATE sync_mutations SET origin_seq=?2, previous_mutation_id=?3 WHERE origin_device_id=?4 AND mutation_id=?1",
                    params![
                        mutation_id,
                        i64::try_from(sequence).map_err(storage)?,
                        previous.map(|id| id.as_uuid().to_string()),
                        local_device,
                    ],
                )
                .map_err(storage)?;
                previous = Some(
                    MutationId::from_uuid(Uuid::parse_str(mutation_id).map_err(storage)?),
                );
                rebased += 1;
            }
            // Both cursor tables follow the same position: the local allocator
            // so later writes continue this chain, and the host-confirmed mirror
            // so the host role reads the same authority it just accepted.
            let last = previous.map(|id| id.as_uuid().to_string());
            let sequence = i64::try_from(sequence).map_err(storage)?;
            tx.execute(
                "INSERT INTO sync_local_origin_cursors(origin_device_id, origin_seq, last_mutation_id, local_root_uuid) VALUES(?1, ?2, ?3, ?4) ON CONFLICT(local_root_uuid,origin_device_id) DO UPDATE SET origin_seq=excluded.origin_seq, last_mutation_id=excluded.last_mutation_id",
                params![local_device, sequence, last, root],
            )
            .map_err(storage)?;
            tx.execute(
                "INSERT INTO sync_origin_cursors(origin_device_id, origin_seq, last_mutation_id) VALUES(?1, ?2, ?3) ON CONFLICT(origin_device_id) DO UPDATE SET origin_seq=excluded.origin_seq, last_mutation_id=excluded.last_mutation_id",
                params![local_device, sequence, last],
            )
            .map_err(storage)?;
            tx.commit().map_err(storage)?;
            Ok(rebased)
        })
    }

    fn replica_checkpoint(&self, host_device_id: DeviceId) -> Result<CommitCheckpoint, Error> {
        self.with_reader(|connection| {
            let binding: Option<(String, String, i64)> = connection
                .query_row(
                    "SELECT library_id, authority_id, epoch FROM sync_binding WHERE singleton=1 AND role='replica' AND local_root_uuid IS NOT NULL",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .optional()
                .map_err(storage)?;
            let (library, authority, epoch) = binding.ok_or_else(|| {
                Error::conflict("replica checkpoint requires a bound local library")
            })?;
            let binding = SyncBinding {
                library_id: crate::domain::sync::LibraryId::from_uuid(
                    Uuid::parse_str(&library).map_err(storage)?,
                ),
                authority_id: crate::domain::sync::AuthorityId::from_uuid(
                    Uuid::parse_str(&authority).map_err(storage)?,
                ),
                epoch: crate::domain::sync::Epoch::from_u64(
                    u64::try_from(epoch).map_err(storage)?,
                ),
            };
            let cursor: Option<(i64, Vec<u8>)> = connection
                .query_row(
                    "SELECT applied_commit_seq, applied_digest FROM sync_peer_cursors WHERE peer_device_id=?1",
                    params![host_device_id.as_uuid().to_string()],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(storage)?;
            let (sequence, digest) = match cursor {
                Some((sequence, digest)) => (
                    u64::try_from(sequence).map_err(storage)?,
                    digest.try_into().map_err(|_| Error::InvariantViolation {
                        why: "invalid persisted replica digest length".to_owned(),
                    })?,
                ),
                None => (0, [0; 32]),
            };
            Ok(CommitCheckpoint {
                binding,
                sequence: CommitSeq::from_u64(sequence),
                digest,
            })
        })
    }
}

#[cfg(test)]
#[path = "tests/sync_queue.rs"]
mod tests;

/// One `sync_mutations` row in [`pending_local_mutations`] column order.
type PendingMutationRow = (
    String,
    i64,
    Option<String>,
    String,
    String,
    Option<i64>,
    String,
    String,
);

/// Decode one queued mutation. The declared resource set is part of the
/// mutation's immutable body: availability may change after a request or ACK
/// loss, and a retry must still submit the exact original declarations.
fn decode_pending_mutation(
    row: PendingMutationRow,
    local_device: &str,
) -> Result<MutationCandidate, Error> {
    let (mutation_id, origin_seq, previous, kind, id, base, payload, resources) = row;
    let parse_uuid = |value: &str, field: &'static str| {
        Uuid::parse_str(value).map_err(|_| Error::InvariantViolation {
            why: format!("invalid {field} in local sync queue"),
        })
    };
    let object_uuid = parse_uuid(&id, "object UUID")?;
    let object = match kind.as_str() {
        "song" | "tombstone" => ObjectKey::Song(SongId::from_uuid(object_uuid)),
        "favorite" => ObjectKey::Favorite(SongId::from_uuid(object_uuid)),
        "override" => ObjectKey::Override(SongId::from_uuid(object_uuid)),
        "play-stats" => ObjectKey::PlayStats(SongId::from_uuid(object_uuid)),
        "playlist" => ObjectKey::Playlist(PlaylistId::from_uuid(object_uuid)),
        "playlist-item" => ObjectKey::PlaylistItem(PlaylistItemId::from_uuid(object_uuid)),
        "library-root" => ObjectKey::LibraryRoot(object_uuid),
        _ => {
            return Err(Error::InvariantViolation {
                why: "unknown object kind in local sync queue".to_owned(),
            })
        }
    };
    let previous_mutation_id = previous
        .as_deref()
        .map(|value| parse_uuid(value, "previous mutation UUID"))
        .transpose()?
        .map(MutationId::from_uuid);
    let base_object_version = base
        .map(|value| {
            u64::try_from(value)
                .map(CommitSeq::from_u64)
                .map(ObjectVersion::from_commit_seq)
                .map_err(storage)
        })
        .transpose()?;
    Ok(MutationCandidate {
        resource_refs: serde_json::from_str(&resources).map_err(storage)?,
        mutation_id: MutationId::from_uuid(parse_uuid(&mutation_id, "mutation UUID")?),
        origin_device_id: DeviceId::from_uuid(parse_uuid(local_device, "origin device UUID")?),
        origin_seq: u64::try_from(origin_seq).map_err(storage)?,
        previous_mutation_id,
        object,
        base_object_version,
        payload: serde_json::from_str(&payload).map_err(storage)?,
    })
}
