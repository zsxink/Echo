//! Durable fixed host snapshots reconstructed from the published log prefix.

use std::collections::BTreeMap;

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Deserialize;

use crate::application::ports::{
    HostSnapshotPage, HostSnapshotPort, HostSnapshotRecord, HostSnapshotSession,
};
use crate::domain::ids::{PlaylistId, PlaylistItemId, SongId};
use crate::domain::sync::{
    CommitCheckpoint, CommitSeq, DeviceId, MutationCandidate, MutationId, ObjectKey, ObjectVersion,
    PlaylistMemberAlias, ResourceDigest, SharedResourceKind, SharedResourceManifest, SyncBinding,
};
use crate::error::{Error, Subject};

use super::super::support::{now_ms, storage};
use super::super::SqliteDatabase;

const MAX_LIFETIME_MS: u64 = 86_400_000;

#[path = "sync_snapshot/pagination.rs"]
mod pagination;
pub(super) use pagination::MAX_PAGE_BYTES;

#[cfg(test)]
#[path = "../tests/sync_snapshot_budget.rs"]
mod budget_tests;

impl HostSnapshotPort for SqliteDatabase {
    fn create_host_snapshot(
        &self,
        binding: SyncBinding,
        page_size: usize,
        lifetime_ms: u64,
    ) -> Result<HostSnapshotSession, Error> {
        if !(1..=512).contains(&page_size) || !(1..=MAX_LIFETIME_MS).contains(&lifetime_ms) {
            return Err(Error::validation(
                Subject::Other,
                "snapshot limits",
                "invalid page size or lifetime",
            ));
        }
        self.writer.run(move |connection| {
            let tx = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;
            ensure_host_binding(&tx, binding)?;
            let created_at = now_ms();
            let expires_at_ms = created_at
                .checked_add(i64::try_from(lifetime_ms).map_err(storage)?)
                .ok_or_else(|| Error::conflict("snapshot expiry exceeds clock range"))?;
            tx.execute(
                "DELETE FROM sync_snapshot_sessions WHERE expires_at<=?1",
                params![created_at],
            )
            .map_err(storage)?;
            let (checkpoint, mut records) = published_records(&tx, binding)?;
            records.extend(alias_records(&tx, checkpoint.sequence)?);
            let session = HostSnapshotSession {
                token: uuid::Uuid::new_v4().simple().to_string(),
                checkpoint,
                page_size: u32::try_from(page_size).map_err(storage)?,
                record_count: u64::try_from(records.len()).map_err(storage)?,
                expires_at_ms,
            };
            persist_session(&tx, &session, created_at)?;
            persist_records(&tx, &session.token, &records)?;
            pagination::persist_pages(&tx, &session, &records)?;
            tx.commit().map_err(storage)?;
            Ok(session)
        })
    }

    fn host_snapshot_page(
        &self,
        binding: SyncBinding,
        token: &str,
        page_index: u32,
    ) -> Result<HostSnapshotPage, Error> {
        let parsed = uuid::Uuid::parse_str(token).map_err(|_| invalid_token())?;
        if parsed.simple().to_string() != token {
            return Err(invalid_token());
        }
        let token = token.to_owned();
        self.with_reader(|connection| {
            // Binding and session checks share a read view with the records.
            let tx = connection.unchecked_transaction().map_err(storage)?;
            ensure_host_binding(&tx, binding)?;
            let session = load_session(&tx, binding, &token)?;
            if session.expires_at_ms <= now_ms() {
                return Err(Error::conflict(
                    "snapshot session expired; capture a new snapshot",
                ));
            }
            let (offset, count) = pagination::load_page(&tx, &session, page_index)?;
            let records = read_records(&tx, &session, offset, count)?;
            let next_page_index = if offset + (records.len() as u64) < session.record_count {
                Some(
                    page_index
                        .checked_add(1)
                        .ok_or_else(|| Error::conflict("snapshot page index exhausted"))?,
                )
            } else {
                None
            };
            tx.commit().map_err(storage)?;
            Ok(HostSnapshotPage {
                session,
                page_index,
                next_page_index,
                records,
            })
        })
    }
}

fn invalid_token() -> Error {
    Error::validation(Subject::Other, "snapshot token", "invalid snapshot token")
}

fn ensure_host_binding(connection: &Connection, binding: SyncBinding) -> Result<(), Error> {
    let matches: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_binding WHERE singleton=1 AND role='host' AND local_root_uuid IS NOT NULL AND library_id=?1 AND authority_id=?2 AND epoch=?3)",
            params![binding.library_id.as_uuid().to_string(), binding.authority_id.as_uuid().to_string(), i64::try_from(binding.epoch.as_u64()).map_err(storage)?],
            |row| row.get(0),
        )
        .map_err(storage)?;
    if matches {
        Ok(())
    } else {
        Err(Error::conflict(
            "snapshot requires the matching host binding",
        ))
    }
}

#[derive(Deserialize)]
struct SnapshotCommit {
    protocol_version: u16,
    binding: SyncBinding,
    commit_seq: CommitSeq,
    previous_digest: [u8; 32],
    mutation: SnapshotMutation,
    #[serde(default)]
    resource_refs: Vec<SnapshotResource>,
}

#[derive(Deserialize)]
struct SnapshotMutation {
    protocol_version: u16,
    binding: SyncBinding,
    origin_device_id: DeviceId,
    mutation_id: MutationId,
    origin_seq: u64,
    previous_mutation_id: Option<MutationId>,
    object: ObjectKey,
    base_object_version: Option<ObjectVersion>,
    payload: serde_json::Value,
}

impl From<SnapshotMutation> for MutationCandidate {
    fn from(value: SnapshotMutation) -> Self {
        Self {
            mutation_id: value.mutation_id,
            origin_device_id: value.origin_device_id,
            origin_seq: value.origin_seq,
            previous_mutation_id: value.previous_mutation_id,
            object: value.object,
            base_object_version: value.base_object_version,
            payload: value.payload,
            resource_refs: Vec::new(),
        }
    }
}

#[derive(Deserialize)]
struct SnapshotResource {
    kind: SharedResourceKind,
    content_hash: String,
    length: u64,
    media_type: String,
    relative_path: String,
}

impl SnapshotResource {
    fn into_manifest(self) -> Result<SharedResourceManifest, Error> {
        if self.content_hash.len() != 64
            || !self
                .content_hash
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(Error::conflict("snapshot resource hash is invalid"));
        }
        let mut digest = [0u8; 32];
        for (index, byte) in digest.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&self.content_hash[index * 2..index * 2 + 2], 16)
                .map_err(storage)?;
        }
        SharedResourceManifest::new(
            self.kind,
            ResourceDigest::from_bytes(digest),
            self.length,
            self.media_type,
            &self.relative_path,
        )
    }
}

fn published_records(
    connection: &Connection,
    binding: SyncBinding,
) -> Result<(CommitCheckpoint, Vec<HostSnapshotRecord>), Error> {
    let mut checkpoint = CommitCheckpoint {
        binding,
        sequence: CommitSeq::GENESIS,
        digest: [0; 32],
    };
    let mut records = BTreeMap::<String, HostSnapshotRecord>::new();
    let mut statement = connection.prepare(
        "SELECT commit_seq, canonical_json, previous_digest, commit_digest FROM sync_commits WHERE publication_state='published' AND library_id=?1 AND authority_id=?2 AND epoch=?3 ORDER BY commit_seq",
    ).map_err(storage)?;
    let rows = statement
        .query_map(
            params![
                binding.library_id.as_uuid().to_string(),
                binding.authority_id.as_uuid().to_string(),
                i64::try_from(binding.epoch.as_u64()).map_err(storage)?
            ],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Vec<u8>>(2)?,
                    row.get::<_, Vec<u8>>(3)?,
                ))
            },
        )
        .map_err(storage)?;
    for row in rows {
        let (sequence, canonical, previous, digest) = row.map_err(storage)?;
        let sequence = CommitSeq::from_u64(u64::try_from(sequence).map_err(storage)?);
        let previous: [u8; 32] = previous.as_slice().try_into().map_err(storage)?;
        let digest: [u8; 32] = digest.as_slice().try_into().map_err(storage)?;
        verify_commit(checkpoint, sequence, previous, digest, &canonical)?;
        let commit: SnapshotCommit = serde_json::from_str(&canonical).map_err(storage)?;
        if commit.protocol_version != 1
            || commit.binding != binding
            || commit.commit_seq != sequence
            || commit.previous_digest != previous
            || commit.mutation.protocol_version != 1
            || commit.mutation.binding != binding
        {
            return Err(Error::conflict(
                "snapshot commit identity does not match its published row",
            ));
        }
        let resources = commit
            .resource_refs
            .into_iter()
            .map(SnapshotResource::into_manifest)
            .collect::<Result<Vec<_>, _>>()?;
        let mut candidate: MutationCandidate = commit.mutation.into();
        candidate.resource_refs.clone_from(&resources);
        let key = serde_json::to_string(&candidate.object).map_err(storage)?;
        if matches!(candidate.object, ObjectKey::PlayStats(_)) {
            merge_play_stats(records.get(&key), &mut candidate)?;
        }
        records.insert(
            key,
            HostSnapshotRecord::Object {
                candidate,
                version: ObjectVersion::from_commit_seq(sequence),
                resources,
            },
        );
        checkpoint.sequence = sequence;
        checkpoint.digest = digest;
    }
    Ok((checkpoint, records.into_values().collect()))
}

fn verify_commit(
    anchor: CommitCheckpoint,
    sequence: CommitSeq,
    previous: [u8; 32],
    digest: [u8; 32],
    canonical: &str,
) -> Result<(), Error> {
    if anchor.sequence.next() != Some(sequence) || anchor.digest != previous {
        return Err(Error::conflict(
            "snapshot published log contains a sequence gap or fork",
        ));
    }
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"echo.lan.commit.v1\0");
    hasher.update(&1u16.to_be_bytes());
    hasher.update(&sequence.as_u64().to_be_bytes());
    hasher.update(&previous);
    hasher.update(
        &u32::try_from(canonical.len())
            .map_err(storage)?
            .to_be_bytes(),
    );
    hasher.update(canonical.as_bytes());
    if hasher.finalize().as_bytes() != &digest {
        return Err(Error::conflict(
            "snapshot published commit digest does not verify",
        ));
    }
    Ok(())
}

fn merge_play_stats(
    previous: Option<&HostSnapshotRecord>,
    candidate: &mut MutationCandidate,
) -> Result<(), Error> {
    if candidate.payload == serde_json::json!({"tombstone":true}) {
        return Ok(());
    }
    let incoming = candidate
        .payload
        .get("by_device")
        .and_then(serde_json::Value::as_object)
        .ok_or_else(|| Error::conflict("snapshot play stats have no device buckets"))?;
    let mut merged = if let Some(HostSnapshotRecord::Object {
        candidate: previous,
        ..
    }) = previous
    {
        previous
            .payload
            .get("by_device")
            .and_then(serde_json::Value::as_object)
            .cloned()
            .unwrap_or_default()
    } else {
        serde_json::Map::new()
    };
    for (device, value) in incoming {
        uuid::Uuid::parse_str(device).map_err(storage)?;
        let count = value
            .as_u64()
            .ok_or_else(|| Error::conflict("snapshot play count is invalid"))?
            .min(i64::MAX as u64);
        let previous = merged
            .get(device)
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        merged.insert(device.clone(), serde_json::Value::from(previous.max(count)));
    }
    candidate.payload["by_device"] = serde_json::Value::Object(merged);
    Ok(())
}

fn alias_records(
    connection: &Connection,
    sequence: CommitSeq,
) -> Result<Vec<HostSnapshotRecord>, Error> {
    let mut records = Vec::new();
    let mut songs = connection.prepare("SELECT alias_song_uuid, canonical_song_uuid FROM sync_song_aliases WHERE commit_seq<=?1 ORDER BY alias_song_uuid").map_err(storage)?;
    let rows = songs
        .query_map(
            params![i64::try_from(sequence.as_u64()).map_err(storage)?],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .map_err(storage)?;
    for row in rows {
        let (alias, canonical) = row.map_err(storage)?;
        records.push(HostSnapshotRecord::SongAlias {
            alias: SongId::from_uuid(uuid::Uuid::parse_str(&alias).map_err(storage)?),
            canonical: SongId::from_uuid(uuid::Uuid::parse_str(&canonical).map_err(storage)?),
        });
    }
    let mut members = connection.prepare("SELECT playlist_uuid, alias_member_uuid, canonical_member_uuid FROM sync_member_aliases WHERE commit_seq<=?1 ORDER BY playlist_uuid, alias_member_uuid").map_err(storage)?;
    let rows = members
        .query_map(
            params![i64::try_from(sequence.as_u64()).map_err(storage)?],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            },
        )
        .map_err(storage)?;
    for row in rows {
        let (playlist, alias, canonical) = row.map_err(storage)?;
        records.push(HostSnapshotRecord::PlaylistMemberAlias(
            PlaylistMemberAlias {
                playlist_id: PlaylistId::from_uuid(
                    uuid::Uuid::parse_str(&playlist).map_err(storage)?,
                ),
                alias_member_id: PlaylistItemId::from_uuid(
                    uuid::Uuid::parse_str(&alias).map_err(storage)?,
                ),
                canonical_member_id: PlaylistItemId::from_uuid(
                    uuid::Uuid::parse_str(&canonical).map_err(storage)?,
                ),
            },
        ));
    }
    Ok(records)
}

fn persist_session(
    connection: &Connection,
    session: &HostSnapshotSession,
    created_at: i64,
) -> Result<(), Error> {
    connection.execute(
        "INSERT INTO sync_snapshot_sessions(token,library_id,authority_id,epoch,commit_seq,commit_digest,page_size,record_count,created_at,expires_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
        params![session.token, session.checkpoint.binding.library_id.as_uuid().to_string(), session.checkpoint.binding.authority_id.as_uuid().to_string(), i64::try_from(session.checkpoint.binding.epoch.as_u64()).map_err(storage)?, i64::try_from(session.checkpoint.sequence.as_u64()).map_err(storage)?, session.checkpoint.digest.as_slice(), session.page_size, i64::try_from(session.record_count).map_err(storage)?, created_at, session.expires_at_ms],
    ).map_err(storage)?;
    Ok(())
}

fn persist_records(
    connection: &Connection,
    token: &str,
    records: &[HostSnapshotRecord],
) -> Result<(), Error> {
    let mut insert = connection
        .prepare(
            "INSERT INTO sync_snapshot_records(token,record_index,record_json) VALUES(?1,?2,?3)",
        )
        .map_err(storage)?;
    for (index, record) in records.iter().enumerate() {
        insert
            .execute(params![
                token,
                i64::try_from(index).map_err(storage)?,
                serde_json::to_string(record).map_err(storage)?
            ])
            .map_err(storage)?;
        if let HostSnapshotRecord::Object { resources, .. } = record {
            for resource in resources {
                connection.execute("INSERT OR IGNORE INTO sync_snapshot_resource_refs(token,content_hash) VALUES(?1,?2)", params![token, resource.digest.as_bytes().as_slice()]).map_err(storage)?;
            }
        }
    }
    Ok(())
}

fn load_session(
    connection: &Connection,
    binding: SyncBinding,
    token: &str,
) -> Result<HostSnapshotSession, Error> {
    let row = connection.query_row(
        "SELECT commit_seq,commit_digest,page_size,record_count,expires_at FROM sync_snapshot_sessions WHERE token=?1 AND library_id=?2 AND authority_id=?3 AND epoch=?4",
        params![token,binding.library_id.as_uuid().to_string(),binding.authority_id.as_uuid().to_string(),i64::try_from(binding.epoch.as_u64()).map_err(storage)?],
        |row| Ok((row.get::<_, i64>(0)?,row.get::<_, Vec<u8>>(1)?,row.get::<_, u32>(2)?,row.get::<_, i64>(3)?,row.get::<_, i64>(4)?)),
    ).optional().map_err(storage)?.ok_or_else(|| Error::conflict("snapshot token is unavailable for this binding"))?;
    Ok(HostSnapshotSession {
        token: token.to_owned(),
        checkpoint: CommitCheckpoint {
            binding,
            sequence: CommitSeq::from_u64(u64::try_from(row.0).map_err(storage)?),
            digest: row.1.as_slice().try_into().map_err(storage)?,
        },
        page_size: row.2,
        record_count: u64::try_from(row.3).map_err(storage)?,
        expires_at_ms: row.4,
    })
}

fn read_records(
    connection: &Connection,
    session: &HostSnapshotSession,
    offset: u64,
    count: u64,
) -> Result<Vec<HostSnapshotRecord>, Error> {
    let mut statement = connection.prepare("SELECT record_index,record_json FROM sync_snapshot_records WHERE token=?1 AND record_index>=?2 ORDER BY record_index LIMIT ?3").map_err(storage)?;
    let rows = statement
        .query_map(
            params![
                session.token,
                i64::try_from(offset).map_err(storage)?,
                i64::try_from(count).map_err(storage)?
            ],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .map_err(storage)?;
    let mut records = Vec::new();
    for row in rows {
        let (index, json) = row.map_err(storage)?;
        if u64::try_from(index).map_err(storage)? != offset + records.len() as u64 {
            return Err(Error::conflict("snapshot record ordering is incomplete"));
        }
        records.push(serde_json::from_str(&json).map_err(storage)?);
    }
    if records.len() as u64 != count {
        return Err(Error::conflict("snapshot page is incomplete"));
    }
    Ok(records)
}
