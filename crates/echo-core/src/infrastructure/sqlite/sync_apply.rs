//! Atomic application of host-confirmed commit batches on replica libraries.

#![allow(clippy::doc_markdown, clippy::missing_errors_doc)]

use rusqlite::{params, OptionalExtension, TransactionBehavior};

use crate::application::ports::{AppliedRemoteCursor, RemoteApplyPort, RemoteCommit};
use crate::domain::sync::{CommitCheckpoint, CommitSeq, DeviceId, ObjectKey, SyncBinding};
use crate::error::Error;

use super::support::{now_ms, storage};
use super::SqliteDatabase;

#[path = "sync_snapshot_apply.rs"]
mod snapshot;

impl RemoteApplyPort for SqliteDatabase {
    fn apply_remote_batch(
        &self,
        binding: SyncBinding,
        peer_device_id: DeviceId,
        anchor: CommitCheckpoint,
        commits: &[RemoteCommit],
    ) -> Result<AppliedRemoteCursor, Error> {
        if anchor.binding != binding || commits.len() > 512 {
            return Err(Error::conflict(
                "remote commit batch binding or size is invalid",
            ));
        }
        let commits = commits.to_vec();
        self.writer.run(move |connection| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(storage)?;
            let enrolling: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM sync_replica_snapshot_sessions WHERE completed=0)", [], |row| row.get(0)).map_err(storage)?;
            if enrolling {
                return Err(Error::conflict("incremental apply requires completed snapshot enrollment"));
            }
            let configured: Option<(String, String, i64, String, Option<String>)> = tx.query_row(
                "SELECT library_id, authority_id, epoch, role, local_root_uuid FROM sync_binding WHERE singleton=1",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
            ).optional().map_err(storage)?;
            let expected_binding = (
                binding.library_id.as_uuid().to_string(),
                binding.authority_id.as_uuid().to_string(),
                i64::try_from(binding.epoch.as_u64()).map_err(storage)?,
            );
            if configured.as_ref().is_none_or(|row| row.3 != "replica" || row.4.is_none() || (row.0.as_str(), row.1.as_str(), row.2) != (expected_binding.0.as_str(), expected_binding.1.as_str(), expected_binding.2)) {
                return Err(Error::conflict("remote apply requires the matching replica binding"));
            }
            let peer = peer_device_id.as_uuid().to_string();
            let stored: Option<(i64, Vec<u8>)> = tx.query_row(
                "SELECT applied_commit_seq, applied_digest FROM sync_peer_cursors WHERE peer_device_id=?1",
                params![peer],
                |row| Ok((row.get(0)?, row.get(1)?)),
            ).optional().map_err(storage)?;
            let (stored_seq, stored_digest) = match stored {
                Some((sequence, digest)) => (sequence, digest.as_slice().try_into().map_err(|_| Error::InvariantViolation { why: "invalid saved remote cursor digest".to_owned() })?),
                None => (0, [0u8; 32]),
            };
            let anchor_seq = i64::try_from(anchor.sequence.as_u64()).map_err(storage)?;
            // Validate the chain before deciding whether this is a fresh apply
            // or a retry. An already-applied exact batch is a no-op; a batch
            // that overlaps with different content is a fork.
            let mut checked_seq = anchor_seq;
            let mut checked_digest = anchor.digest;
            for commit in &commits {
                checked_seq = checked_seq.checked_add(1).ok_or_else(|| Error::InvariantViolation { why: "remote commit sequence overflow".to_owned() })?;
                if i64::try_from(commit.sequence.as_u64()).map_err(storage)? != checked_seq || commit.previous_digest != checked_digest {
                    return Err(Error::conflict("remote commit sequence has a gap or fork"));
                }
                verify_remote_commit(binding, commit)?;
                checked_digest = commit.digest;
            }
            if anchor_seq != stored_seq || anchor.digest != stored_digest {
                if commits.is_empty() {
                    return Err(Error::conflict("empty remote batch must match the durable peer cursor"));
                }
                if checked_seq <= stored_seq {
                    let anchor_digest: Option<Vec<u8>> = if anchor_seq == 0 {
                        Some(vec![0; 32])
                    } else {
                        tx.query_row(
                            "SELECT commit_digest FROM sync_commits WHERE commit_seq=?1 AND library_id=?2 AND authority_id=?3 AND epoch=?4
                             UNION ALL SELECT commit_digest FROM sync_replica_snapshot_sessions WHERE commit_seq=?1 AND library_id=?2 AND authority_id=?3 AND epoch=?4 AND peer_device_id=?5 AND completed=1 LIMIT 1",
                            params![anchor_seq, expected_binding.0, expected_binding.1, expected_binding.2, peer],
                            |row| row.get(0),
                        ).optional().map_err(storage)?
                    };
                    if anchor_digest.as_deref() != Some(anchor.digest.as_slice()) {
                        return Err(Error::conflict("replayed remote anchor differs from the durable ledger"));
                    }
                    for commit in &commits {
                        let sequence = i64::try_from(commit.sequence.as_u64()).map_err(storage)?;
                        let existing: Option<(Vec<u8>, String)> = tx.query_row(
                            "SELECT commit_digest, canonical_json FROM sync_commits WHERE commit_seq=?1 AND library_id=?2 AND authority_id=?3 AND epoch=?4",
                            params![sequence, expected_binding.0, expected_binding.1, expected_binding.2],
                            |row| Ok((row.get(0)?, row.get(1)?)),
                        ).optional().map_err(storage)?;
                        if existing.as_ref().is_none_or(|(digest, body)| digest.as_slice() != commit.digest.as_slice() || body != &commit.canonical_json) {
                            return Err(Error::conflict("replayed remote commit differs from the durable ledger"));
                        }
                    }
                    tx.commit().map_err(storage)?;
                    return Ok(AppliedRemoteCursor { sequence: CommitSeq::from_u64(u64::try_from(stored_seq).map_err(storage)?), digest: stored_digest, applied: 0 });
                }
                return Err(Error::conflict("remote apply cursor does not match the durable peer cursor"));
            }
            let mut sequence = anchor_seq;
            let mut digest = anchor.digest;
            for commit in &commits {
                sequence = sequence.checked_add(1).ok_or_else(|| Error::InvariantViolation { why: "remote commit sequence overflow".to_owned() })?;
                if i64::try_from(commit.sequence.as_u64()).map_err(storage)? != sequence || commit.previous_digest != digest {
                    return Err(Error::conflict("remote commit sequence has a gap or fork"));
                }
                verify_remote_commit(binding, commit)?;
                apply_remote_commit(&tx, &expected_binding, configured.as_ref().and_then(|row| row.4.as_deref()), commit, sequence)?;
                digest = commit.digest;
            }
            tx.execute(
                "INSERT INTO sync_peer_cursors(peer_device_id, applied_commit_seq, applied_digest, resource_bytes_complete, updated_at) VALUES(?1, ?2, ?3, 0, ?4) ON CONFLICT(peer_device_id) DO UPDATE SET applied_commit_seq=excluded.applied_commit_seq, applied_digest=excluded.applied_digest, updated_at=excluded.updated_at",
                params![peer, sequence, digest.as_slice(), now_ms()],
            ).map_err(storage)?;
            tx.commit().map_err(storage)?;
            Ok(AppliedRemoteCursor { sequence: CommitSeq::from_u64(u64::try_from(sequence).map_err(storage)?), digest, applied: commits.len() })
        })
    }
}

fn apply_remote_commit(
    tx: &rusqlite::Transaction<'_>,
    expected_binding: &(String, String, i64),
    local_root_uuid: Option<&str>,
    commit: &RemoteCommit,
    sequence: i64,
) -> Result<(), Error> {
    let (object_type, object_uuid) = object_identity(commit.object);
    if !commit.is_tombstone && super::sync_projection::is_tombstoned(tx, object_type, &object_uuid)?
    {
        return Err(Error::conflict(
            "remote commit attempts to revive a finally deleted object",
        ));
    }
    let payload = if commit.is_tombstone {
        None
    } else {
        Some(commit.payload_json.as_str())
    };
    let resources = super::sync_manifests::from_commit_json(&commit.canonical_json)?;
    if commit.is_tombstone && !resources.is_empty() {
        return Err(Error::conflict(
            "deleted objects cannot declare resource references",
        ));
    }
    super::sync_manifests::replace(tx, object_type, &object_uuid, &resources)?;
    apply_commit_aliases(tx, &commit.canonical_json, sequence)?;
    tx.execute(
                    "INSERT INTO sync_commits(commit_seq, library_id, authority_id, epoch, origin_device_id, mutation_id, canonical_json, previous_digest, commit_digest, publication_state, created_at) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, 'published', ?10)",
                    params![sequence, expected_binding.0, expected_binding.1, expected_binding.2, commit.origin_device_id.as_uuid().to_string(), commit.mutation_id.as_uuid().to_string(), commit.canonical_json, commit.previous_digest.as_slice(), commit.digest.as_slice(), now_ms()],
                ).map_err(storage)?;
    tx.execute(
                    "INSERT INTO sync_object_versions(object_type, object_uuid, object_version, commit_seq, payload_json, is_tombstone) VALUES(?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT(object_type, object_uuid) DO UPDATE SET object_version=excluded.object_version, commit_seq=excluded.commit_seq, payload_json=excluded.payload_json, is_tombstone=excluded.is_tombstone",
                    params![object_type, object_uuid, sequence, sequence, payload, i64::from(commit.is_tombstone)],
                ).map_err(storage)?;
    tx.execute(
                    "UPDATE sync_mutations SET status='confirmed', outcome_json=?3 WHERE origin_device_id=?1 AND mutation_id=?2 AND status='pending'",
                    params![commit.origin_device_id.as_uuid().to_string(), commit.mutation_id.as_uuid().to_string(), "{\"accepted\":true}"],
                )
                .map_err(storage)?;
    if commit.is_tombstone {
        super::sync_projection::project_confirmed_tombstone(tx, object_type, &object_uuid)?;
    } else {
        let projection: serde_json::Value =
            serde_json::from_str(&commit.payload_json).map_err(storage)?;
        super::sync_projection::project_confirmed_song_state(
            tx,
            object_type,
            &object_uuid,
            &projection,
            local_root_uuid,
        )?;
    }
    project_song_aliases(tx, &commit.canonical_json)?;
    tx.execute(
                    "INSERT INTO sync_inbox(origin_device_id, mutation_id, payload_digest, outcome_json, commit_seq, received_at) VALUES(?1, ?2, ?3, '{\"applied\":true}', ?4, ?5)",
                    params![commit.origin_device_id.as_uuid().to_string(), commit.mutation_id.as_uuid().to_string(), blake3::hash(commit.canonical_json.as_bytes()).to_hex().to_string(), sequence, now_ms()],
                ).map_err(storage)?;
    Ok(())
}

fn apply_commit_aliases(
    tx: &rusqlite::Transaction<'_>,
    canonical_json: &str,
    sequence: i64,
) -> Result<(), Error> {
    let body: serde_json::Value = serde_json::from_str(canonical_json).map_err(storage)?;
    let Some(aliases) = body.get("aliases").and_then(serde_json::Value::as_array) else {
        return Ok(());
    };
    for typed in aliases {
        if typed.get("kind").and_then(serde_json::Value::as_str) == Some("song") {
            let alias = typed
                .pointer("/alias/alias")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| Error::conflict("invalid song alias"))?;
            let canonical = typed
                .pointer("/alias/canonical")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| Error::conflict("invalid canonical song identity"))?;
            super::sync_song_aliases::persist(tx, alias, canonical, sequence)?;
            continue;
        }
        if typed.get("kind").and_then(serde_json::Value::as_str) != Some("playlist_member") {
            return Err(Error::conflict(
                "remote commit contains an unsupported alias kind",
            ));
        }
        let alias = typed
            .get("alias")
            .ok_or_else(|| Error::conflict("member alias is malformed"))?;
        let playlist = alias
            .get("playlist_id")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| Error::conflict("member alias is missing playlist identity"))?;
        let member = alias
            .get("alias")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| Error::conflict("member alias is missing alias identity"))?;
        let canonical = alias
            .get("canonical")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| Error::conflict("member alias is missing canonical identity"))?;
        if member == canonical {
            return Err(Error::conflict("member alias cannot target itself"));
        }
        let previous: Option<String> = tx.query_row(
            "SELECT canonical_member_uuid FROM sync_member_aliases WHERE playlist_uuid=?1 AND alias_member_uuid=?2",
            rusqlite::params![playlist, member],
            |row| row.get(0),
        ).optional().map_err(storage)?;
        if previous.as_deref().is_some_and(|value| value != canonical) {
            return Err(Error::conflict("member alias identity was remapped"));
        }
        tx.execute(
            "INSERT OR IGNORE INTO sync_member_aliases(playlist_uuid,alias_member_uuid,canonical_member_uuid,commit_seq) VALUES(?1,?2,?3,?4)",
            rusqlite::params![playlist, member, canonical, sequence],
        ).map_err(storage)?;
        tx.execute(
            "UPDATE playlist_songs SET member_uuid=?3 WHERE playlist_uuid=?1 AND member_uuid=?2",
            rusqlite::params![playlist, member, canonical],
        )
        .map_err(storage)?;
    }
    Ok(())
}

fn project_song_aliases(tx: &rusqlite::Transaction<'_>, canonical_json: &str) -> Result<(), Error> {
    let body: serde_json::Value = serde_json::from_str(canonical_json).map_err(storage)?;
    if let Some(aliases) = body.get("aliases").and_then(serde_json::Value::as_array) {
        for typed in aliases {
            if typed.get("kind").and_then(serde_json::Value::as_str) == Some("song") {
                if let (Some(alias), Some(canonical)) = (
                    typed
                        .pointer("/alias/alias")
                        .and_then(serde_json::Value::as_str),
                    typed
                        .pointer("/alias/canonical")
                        .and_then(serde_json::Value::as_str),
                ) {
                    super::sync_song_aliases::project(tx, alias, canonical)?;
                }
            }
        }
    }
    Ok(())
}

fn verify_remote_commit(binding: SyncBinding, commit: &RemoteCommit) -> Result<(), Error> {
    let body: serde_json::Value = serde_json::from_str(&commit.canonical_json).map_err(storage)?;
    let payload: serde_json::Value = serde_json::from_str(&commit.payload_json).map_err(storage)?;
    let canonical_tombstone = payload == serde_json::json!({"tombstone": true});
    if body
        .get("protocol_version")
        .and_then(serde_json::Value::as_u64)
        != Some(1)
        || body.get("binding") != Some(&serde_json::to_value(binding).map_err(storage)?)
        || body.get("commit_seq").and_then(serde_json::Value::as_u64)
            != Some(commit.sequence.as_u64())
        || body.get("previous_digest")
            != Some(&serde_json::to_value(commit.previous_digest).map_err(storage)?)
        || body.pointer("/mutation/mutation_id")
            != Some(&serde_json::to_value(commit.mutation_id).map_err(storage)?)
        || body.pointer("/mutation/origin_device_id")
            != Some(&serde_json::to_value(commit.origin_device_id).map_err(storage)?)
        || body.pointer("/mutation/object")
            != Some(&serde_json::to_value(commit.object).map_err(storage)?)
        || body.pointer("/mutation/payload") != Some(&payload)
        || canonical_tombstone != commit.is_tombstone
    {
        return Err(Error::conflict(
            "remote commit body does not match its typed projection",
        ));
    }
    let body_len = u32::try_from(commit.canonical_json.len()).map_err(storage)?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"echo.lan.commit.v1\0");
    hasher.update(&1u16.to_be_bytes());
    hasher.update(&commit.sequence.as_u64().to_be_bytes());
    hasher.update(&commit.previous_digest);
    hasher.update(&body_len.to_be_bytes());
    hasher.update(commit.canonical_json.as_bytes());
    if hasher.finalize().as_bytes() != &commit.digest {
        return Err(Error::conflict("remote commit digest verification failed"));
    }
    Ok(())
}

fn object_identity(object: ObjectKey) -> (&'static str, String) {
    match object {
        ObjectKey::Song(id) => ("song", id.to_string()),
        ObjectKey::Playlist(id) => ("playlist", id.to_string()),
        ObjectKey::PlaylistItem(id) => ("playlist-item", id.to_string()),
        ObjectKey::LibraryRoot(id) => ("library-root", id.to_string()),
        ObjectKey::Favorite(id) => ("favorite", id.to_string()),
        ObjectKey::Override(id) => ("override", id.to_string()),
        ObjectKey::PlayStats(id) => ("play-stats", id.to_string()),
    }
}
