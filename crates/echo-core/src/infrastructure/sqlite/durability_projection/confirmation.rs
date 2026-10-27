//! Only a verified ledger event may confirm the value a record envelope carries.
//!
//! A v2 envelope's `sync` block is a claim, not evidence: it can survive a
//! local edit that replaced the value while the old object version stayed
//! attached. A claim is trusted only when the ledger holds a hash-verified
//! event at exactly that version for this object, whose payload agrees with
//! the value on disk. Anything else is an unproven candidate: the bytes are
//! retained and reported, and no confirmed baseline is minted for them.

use super::super::support::storage;
use crate::application::ports::RemoteCommit;
use crate::domain::library::{PortableRecord, PortableRecordEnvelope};
use crate::domain::sync::CommitSeq;
use crate::error::Error;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

/// Whether the envelope claims a confirmation the ledger cannot prove.
///
/// # Errors
/// Propagates storage failures and an unencodable object identity.
pub(super) fn unproven(c: &Connection, envelope: &PortableRecordEnvelope) -> Result<bool, Error> {
    let Some(sync) = envelope.sync_metadata() else {
        return Ok(false);
    };
    let sequence = sync.object_version.commit_seq();
    // Genesis means "no host version", so the block claims no confirmation.
    if sequence == CommitSeq::GENESIS {
        return Ok(false);
    }
    let row = c
        .query_row(
            "SELECT canonical_json,previous_digest,commit_digest FROM sync_commits WHERE commit_seq=?1 AND library_id=?2 AND authority_id=?3 AND epoch=?4",
            params![
                sequence.as_u64(),
                sync.binding.library_id.as_uuid().to_string(),
                sync.binding.authority_id.as_uuid().to_string(),
                sync.binding.epoch.as_u64()
            ],
            |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, Vec<u8>>(1)?,
                    r.get::<_, Vec<u8>>(2)?,
                ))
            },
        )
        .optional()
        .map_err(storage)?;
    let Some((canonical, previous, digest)) = row else {
        return Ok(true);
    };
    let (Ok(previous), Ok(digest)) = (previous.try_into(), digest.try_into()) else {
        return Ok(true);
    };
    let Ok(body) = serde_json::from_str::<Value>(&canonical) else {
        return Ok(true);
    };
    let candidate = &body["mutation"];
    let (Ok(origin_device_id), Ok(mutation_id), Ok(object)) = (
        serde_json::from_value(candidate["origin_device_id"].clone()),
        serde_json::from_value(candidate["mutation_id"].clone()),
        serde_json::from_value(candidate["object"].clone()),
    ) else {
        return Ok(true);
    };
    let commit = RemoteCommit {
        sequence,
        previous_digest: previous,
        digest,
        origin_device_id,
        mutation_id,
        object,
        payload_json: serde_json::to_string(&candidate["payload"]).map_err(storage)?,
        is_tombstone: candidate["payload"] == json!({"tombstone": true}),
        canonical_json: canonical,
    };
    // A body that fails its own binding and digest checks proves nothing.
    if crate::infrastructure::sqlite::sync::apply::verify_remote_commit(sync.binding, &commit)
        .is_err()
        || commit.object != envelope.record().object_key()?
        || !agrees(envelope.record(), &candidate["payload"])
    {
        return Ok(true);
    }
    Ok(false)
}

/// Portable agreement: a field the confirmed payload does not assert is not a
/// disagreement, but a field it does assert must match the value on disk. The
/// song's `relative_path` is host-scoped and deliberately not compared.
fn agrees(record: &PortableRecord, payload: &Value) -> bool {
    let same =
        |key: &str, expected: Value| payload.get(key).is_none_or(|actual| actual == &expected);
    match record {
        PortableRecord::Song(s) => {
            same("song_uuid", json!(s.song_uuid))
                && same("title", json!(s.title))
                && same("artist", json!(s.artist))
                && same("album", json!(s.album))
                && same("blake3_hash", json!(s.content_hash))
                && same("added_at", json!(s.added_at))
        }
        PortableRecord::Favorite(r) => {
            same("song_uuid", json!(r.song_uuid))
                && same("is_favorite", json!(r.is_favorite))
                && same("favorited_at", json!(r.favorited_at))
        }
        PortableRecord::Playlist(r) => {
            same("playlist_uuid", json!(r.playlist_uuid))
                && same("display_name", json!(r.display_name))
                && same("created_at", json!(r.created_at))
                && same("cover_asset", json!(r.cover_asset))
        }
        PortableRecord::PlaylistItem(r) => {
            same("playlist_uuid", json!(r.playlist_uuid))
                && same("member_uuid", json!(r.item_uuid))
                && same("song_uuid", json!(r.song_uuid))
                && same("position", json!(r.position))
                && same("added_at", json!(r.added_at))
        }
        PortableRecord::Override(r) => {
            same("song_uuid", json!(r.song_uuid))
                && same("title", json!(r.title))
                && same("artist", json!(r.artist))
                && same("album", json!(r.album))
                && same("lyrics_text", json!(r.lyrics_text))
        }
        PortableRecord::PlayStats(r) => {
            same("song_uuid", json!(r.song_uuid)) && same("by_device", json!(r.by_device))
        }
        PortableRecord::Tombstone(_) => payload == &json!({"tombstone": true}),
    }
}
