//! Explicit recovery of a historical tag-edit identity split.
//!
//! This queues an ordinary song mutation; the host's existing exact-file-hash
//! canonicalization owns the alias. No song, alias, manifest or media is changed.

use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use serde::Serialize;
use serde_json::Value;

use super::super::support::storage;
use super::super::SqliteDatabase;
use super::{configuration::SyncConfiguration, sync_manifests};
use crate::domain::ids::SongId;
use crate::domain::sync::{SharedResourceKind, SharedResourceManifest};
use crate::error::Error;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct ConfirmedSong {
    id: SongId,
    version: u64,
    payload: Value,
    resources: Vec<SharedResourceManifest>,
    local_path: String,
    local_hash: String,
    local_size: u64,
    local_state: String,
    revision: i64,
    favorite: bool,
    play_count: u64,
}

/// Reviewable, immutable proof tied to a specific binding, root and two versions.
/// Obtain it through `prepare_song_identity_repair`; fields cannot be supplied
/// by a caller to bypass verification.
#[derive(Clone, Serialize)]
pub struct SongIdentityRepairPlan {
    configuration: RepairConfiguration,
    root_path: PathBuf,
    old: ConfirmedSong,
    canonical: ConfirmedSong,
    pub audio_bytes: usize,
    pub audio_blake3: String,
    /// Existing alias behavior retains each device's maximum counter. It does
    /// not add counters that might describe overlapping playback history.
    pub play_count_policy: &'static str,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct RepairConfiguration {
    binding: crate::domain::sync::SyncBinding,
    root: crate::domain::ids::LibraryRootId,
    device: crate::domain::sync::DeviceId,
}

impl From<SyncConfiguration> for RepairConfiguration {
    fn from(value: SyncConfiguration) -> Self {
        Self {
            binding: value.binding,
            root: value.local_root,
            device: value.local_device,
        }
    }
}

impl SqliteDatabase {
    /// Verify two explicitly selected confirmed FLAC versions with the same
    /// logical destination and byte-for-byte identical encoded audio frames.
    ///
    /// # Errors
    /// Rejects stale versions, pending work, deletes, unsafe paths, differing
    /// audio, or alias merges which could hide a favorite/override/member.
    pub fn prepare_song_identity_repair(
        &self,
        old: SongId,
        old_version: u64,
        canonical: SongId,
        canonical_version: u64,
    ) -> Result<SongIdentityRepairPlan, Error> {
        if old == canonical {
            return Err(Error::conflict(
                "repair requires two distinct song identities",
            ));
        }
        let config = self
            .current_sync_configuration()?
            .ok_or_else(|| Error::conflict("repair requires an active sync binding"))?;
        if config.host {
            return Err(Error::conflict("repair intent must be queued on a replica"));
        }
        let configuration = RepairConfiguration::from(config);
        let (root_path, old, canonical) = self.with_reader(|c| {
            snapshot(
                c,
                &configuration,
                old,
                old_version,
                canonical,
                canonical_version,
            )
        })?;
        let mut plan = SongIdentityRepairPlan {
            configuration,
            root_path,
            old,
            canonical,
            audio_bytes: 0,
            audio_blake3: String::new(),
            play_count_policy: "maximum per device; immutable history retained",
        };
        let audio = verify_files(&plan)?;
        plan.audio_bytes = audio.0;
        plan.audio_blake3 = audio.1;
        Ok(plan)
    }

    /// Append a normal outbox intent only if the reviewed proof still holds.
    /// Host confirmation, publication and alias projection remain unchanged.
    ///
    /// # Errors
    /// Rejects any binding/root/version/local-state/file changes since prepare.
    pub fn enqueue_song_identity_repair(&self, plan: SongIdentityRepairPlan) -> Result<i64, Error> {
        self.writer.run(move |connection| {
            let tx = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;
            let (root, old, canonical) = snapshot(
                &tx,
                &plan.configuration,
                plan.old.id,
                plan.old.version,
                plan.canonical.id,
                plan.canonical.version,
            )?;
            if root != plan.root_path || old != plan.old || canonical != plan.canonical {
                return Err(Error::conflict(
                    "identity repair proof changed; prepare again",
                ));
            }
            let audio = verify_files(&plan)?;
            if audio != (plan.audio_bytes, plan.audio_blake3.clone()) {
                return Err(Error::conflict("identity repair audio proof changed"));
            }
            let mut payload = plan.canonical.payload;
            payload["song_uuid"] = serde_json::to_value(plan.old.id).map_err(storage)?;
            let revision = super::enqueue::enqueue_sync_in_root_with_resources(
                &tx,
                &plan.configuration.root.to_string(),
                "song",
                &plan.old.id.to_string(),
                &serde_json::to_string(&payload).map_err(storage)?,
                "upsert",
                None,
                &plan.canonical.resources,
            )?;
            tx.commit().map_err(storage)?;
            Ok(revision)
        })
    }
}

fn snapshot(
    c: &Connection,
    config: &RepairConfiguration,
    old: SongId,
    old_version: u64,
    canonical: SongId,
    canonical_version: u64,
) -> Result<(PathBuf, ConfirmedSong, ConfirmedSong), Error> {
    let root: Option<String> = c.query_row(
        "SELECT r.absolute_path FROM sync_binding b JOIN library_roots r ON r.uuid=b.local_root_uuid JOIN sync_active_context a ON a.local_root_uuid=r.uuid JOIN device_state d ON d.key='device_id' WHERE b.singleton=1 AND b.role='replica' AND b.library_id=?1 AND b.authority_id=?2 AND b.epoch=?3 AND r.uuid=?4 AND r.is_active=1 AND r.write_safety_locked=0 AND d.value=?5",
        params![config.binding.library_id.as_uuid().to_string(), config.binding.authority_id.as_uuid().to_string(), i64::try_from(config.binding.epoch.as_u64()).map_err(storage)?, config.root.to_string(), config.device.as_uuid().to_string()], |r|r.get(0)
    ).optional().map_err(storage)?;
    let root = PathBuf::from(
        root.ok_or_else(|| Error::conflict("identity repair binding or active root changed"))?,
    );
    let old = confirmed(c, config, old, old_version)?;
    let canonical = confirmed(c, config, canonical, canonical_version)?;
    if old
        .payload
        .get("relative_path")
        .and_then(Value::as_str)
        .filter(|p| !p.is_empty())
        != canonical
            .payload
            .get("relative_path")
            .and_then(Value::as_str)
    {
        return Err(Error::conflict(
            "identity repair logical media paths differ",
        ));
    }
    if old.favorite && !canonical.favorite {
        return Err(Error::conflict(
            "alias would hide a confirmed favorite; explicit favorite reconciliation required",
        ));
    }
    let overrides: bool = c
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM song_overrides WHERE song_uuid=?1)",
            params![old.id.to_string()],
            |r| r.get(0),
        )
        .map_err(storage)?;
    if overrides {
        return Err(Error::conflict(
            "alias has metadata overrides; explicit reconciliation required",
        ));
    }
    let collisions: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM playlist_songs a JOIN playlist_songs b ON a.playlist_uuid=b.playlist_uuid WHERE a.song_uuid=?1 AND b.song_uuid=?2)", params![old.id.to_string(),canonical.id.to_string()], |r|r.get(0)).map_err(storage)?;
    if collisions {
        return Err(Error::conflict(
            "alias has colliding playlist memberships; explicit reconciliation required",
        ));
    }
    Ok((root, old, canonical))
}

/// (object_version, payload_json, relative_path, blake3_hash, file_size,
/// availability, revision, is_favorite, play_count)
type ConfirmedSongRow = (i64, String, String, String, i64, String, i64, bool, i64);

fn confirmed(
    c: &Connection,
    config: &RepairConfiguration,
    id: SongId,
    version: u64,
) -> Result<ConfirmedSong, Error> {
    let row: Option<ConfirmedSongRow> = c.query_row(
        "SELECT v.object_version,v.payload_json,s.relative_path,s.blake3_hash,s.file_size,s.availability,s.revision,s.is_favorite,s.play_count FROM sync_object_versions v JOIN songs s ON s.uuid=v.object_uuid WHERE v.object_type='song' AND v.object_uuid=?1 AND v.is_tombstone=0 AND s.library_root_uuid=?2 AND s.availability IN ('available','missing')",
        params![id.to_string(),config.root.to_string()], |r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?,r.get(5)?,r.get(6)?,r.get(7)?,r.get(8)?))
    ).optional().map_err(storage)?;
    let (actual, payload, path, hash, size, state, revision, favorite, plays) =
        row.ok_or_else(|| Error::conflict("repair requires existing confirmed live songs"))?;
    if u64::try_from(actual).map_err(storage)? != version {
        return Err(Error::conflict("identity repair object version changed"));
    }
    let blocked: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_mutations WHERE local_root_uuid=?2 AND status='pending') OR EXISTS(SELECT 1 FROM sync_local_delete_intents WHERE object_uuid=?1 AND state='open') OR EXISTS(SELECT 1 FROM sync_song_aliases WHERE alias_song_uuid=?1 OR canonical_song_uuid=?1)",
        params![id.to_string(),config.root.to_string()], |r|r.get(0)
    ).map_err(storage)?;
    if blocked || super::projection::is_tombstoned(c, "song", &id.to_string())? {
        return Err(Error::conflict(
            "repair blocked by pending work, deletion or existing alias",
        ));
    }
    let payload: Value = serde_json::from_str(&payload).map_err(storage)?;
    if payload.get("blake3_hash").and_then(Value::as_str) != Some(hash.as_str())
        || payload.get("file_size").and_then(Value::as_u64) != u64::try_from(size).ok()
    {
        return Err(Error::conflict(
            "local file facts do not match the confirmed version",
        ));
    }
    let resources = sync_manifests::load(c, "song", &id.to_string())?;
    if !resources.iter().any(|r| {
        r.kind == SharedResourceKind::Audio
            && r.digest.to_hex() == hash
            && r.length == u64::try_from(size).unwrap_or(0)
    }) {
        return Err(Error::conflict("confirmed audio resource is missing"));
    }
    Ok(ConfirmedSong {
        id,
        version,
        payload,
        resources,
        local_path: path,
        local_hash: hash,
        local_size: u64::try_from(size).map_err(storage)?,
        local_state: state,
        revision,
        favorite,
        play_count: u64::try_from(plays).map_err(storage)?,
    })
}

fn file_bytes(root: &Path, song: &ConfirmedSong) -> Result<Vec<u8>, Error> {
    let root = root
        .canonicalize()
        .map_err(|e| Error::io("resolve repair root", e, root.to_path_buf()))?;
    let path = root.join(&song.local_path);
    let resolved = path
        .canonicalize()
        .map_err(|e| Error::io("resolve repair audio", e, path.clone()))?;
    if !resolved.starts_with(&root) || resolved == root {
        return Err(Error::conflict("repair file escapes library root"));
    }
    let bytes = std::fs::read(&resolved)
        .map_err(|e| Error::io("read repair audio", e, resolved.clone()))?;
    if bytes.len() as u64 != song.local_size
        || blake3::hash(&bytes).to_hex().as_str() != song.local_hash
    {
        return Err(Error::conflict(
            "repair file no longer matches confirmed hash",
        ));
    }
    Ok(bytes)
}

fn verify_files(plan: &SongIdentityRepairPlan) -> Result<(usize, String), Error> {
    let old = file_bytes(&plan.root_path, &plan.old)?;
    let new = file_bytes(&plan.root_path, &plan.canonical)?;
    let old_audio = flac_audio(&old)?;
    let new_audio = flac_audio(&new)?;
    if old_audio != new_audio {
        return Err(Error::conflict(
            "FLAC audio frames differ; identities cannot be repaired",
        ));
    }
    // Re-read after comparison to catch external taggers replacing either file.
    if file_bytes(&plan.root_path, &plan.old)? != old
        || file_bytes(&plan.root_path, &plan.canonical)? != new
    {
        return Err(Error::conflict("repair audio changed while being verified"));
    }
    Ok((
        old_audio.len(),
        blake3::hash(old_audio).to_hex().to_string(),
    ))
}

fn flac_audio(bytes: &[u8]) -> Result<&[u8], Error> {
    let invalid = || {
        Error::conflict("identity repair requires native FLAC with valid metadata and audio frames")
    };
    if bytes.get(..4) != Some(b"fLaC") {
        return Err(invalid());
    }
    let mut offset = 4usize;
    let mut first = true;
    loop {
        let header = bytes.get(offset..offset + 4).ok_or_else(invalid)?;
        let kind = header[0] & 127;
        let len =
            (usize::from(header[1]) << 16) | (usize::from(header[2]) << 8) | usize::from(header[3]);
        if kind == 127 || (first && (kind != 0 || len != 34)) {
            return Err(invalid());
        }
        offset = offset
            .checked_add(4 + len)
            .filter(|o| *o <= bytes.len())
            .ok_or_else(invalid)?;
        first = false;
        if header[0] & 128 != 0 {
            break;
        }
    }
    let audio = bytes
        .get(offset..)
        .filter(|b| b.len() > 2 && b[0] == 0xff && b[1] & 0xfe == 0xf8)
        .ok_or_else(invalid)?;
    Ok(audio)
}
