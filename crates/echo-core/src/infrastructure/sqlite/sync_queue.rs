//! Durable access to queued local mutations and replica cursors.

#![allow(clippy::doc_markdown, clippy::missing_errors_doc)]

use rusqlite::{params, OptionalExtension};
use uuid::Uuid;

use crate::application::ports::LocalSyncQueuePort;
use crate::domain::ids::{PlaylistId, PlaylistItemId, SongId};
use crate::domain::sync::{
    CommitCheckpoint, CommitSeq, DeviceId, MutationCandidate, MutationId, ObjectKey, ObjectVersion,
    SyncBinding,
};
use crate::error::{Error, Subject};

use super::support::storage;
use super::SqliteDatabase;

const MAX_BATCH: usize = 512;

impl LocalSyncQueuePort for SqliteDatabase {
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
                    "SELECT mutation_id, origin_seq, previous_mutation_id, object_type, object_uuid, base_object_version, payload_json, resource_refs_json FROM sync_mutations WHERE origin_device_id=?1 AND status='pending' ORDER BY origin_seq LIMIT ?2",
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
                let (mutation_id, origin_seq, previous, kind, id, base, payload, resources) =
                    row.map_err(storage)?;
                let parse_uuid = |value: &str, field: &'static str| {
                    Uuid::parse_str(value).map_err(|_| {
                        Error::InvariantViolation {
                            why: format!("invalid {field} in local sync queue"),
                        }
                    })
                };
                let object_uuid = parse_uuid(&id, "object UUID")?;
                let object = match kind.as_str() {
                    "song" | "tombstone" => ObjectKey::Song(SongId::from_uuid(object_uuid)),
                    "favorite" => ObjectKey::Favorite(SongId::from_uuid(object_uuid)),
                    "override" => ObjectKey::Override(SongId::from_uuid(object_uuid)),
                    "play-stats" => ObjectKey::PlayStats(SongId::from_uuid(object_uuid)),
                    "playlist" => ObjectKey::Playlist(PlaylistId::from_uuid(object_uuid)),
                    "playlist-item" => {
                        ObjectKey::PlaylistItem(PlaylistItemId::from_uuid(object_uuid))
                    }
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
                    mutation_id: MutationId::from_uuid(parse_uuid(
                        &mutation_id,
                        "mutation UUID",
                    )?),
                    origin_device_id: DeviceId::from_uuid(parse_uuid(
                        &local_device,
                        "origin device UUID",
                    )?),
                    origin_seq: u64::try_from(origin_seq).map_err(storage)?,
                    previous_mutation_id,
                    object,
                    base_object_version,
                    payload: serde_json::from_str(&payload).map_err(storage)?,
                })
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
            let local_device: String = connection
                .query_row(
                    "SELECT value FROM device_state WHERE key='device_id'",
                    [],
                    |row| row.get(0),
                )
                .map_err(storage)?;
            let changed = connection
                .execute(
                    "UPDATE sync_mutations SET status='rejected', outcome_json=?3 WHERE origin_device_id=?1 AND mutation_id=?2 AND status='pending'",
                    params![local_device, mutation_id.as_uuid().to_string(), outcome_json],
                )
                .map_err(storage)?;
            if changed == 1 {
                Ok(())
            } else {
                Err(Error::conflict("local mutation is missing or already terminal"))
            }
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
