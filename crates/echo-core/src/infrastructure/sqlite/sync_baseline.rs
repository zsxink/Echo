//! Atomic initial snapshots for libraries whose canonical rows predate sync.

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use crate::domain::ids::{LibraryRootId, PlaylistId, SongId};
use crate::domain::sync::{SharedResourceKind, SharedResourceManifest, SyncBinding};
use crate::error::Error;

use super::support::{now_ms, parse_id, storage};
use super::{statements, sync, sync_manifests, sync_projection, SqliteDatabase};

impl SqliteDatabase {
    /// Seed a replica's existing local catalog into its durable outbox before
    /// the host snapshot is projected. This makes pending-local overlays win
    /// over the initial remote state without replacing the local catalog.
    pub fn ensure_replica_sync_baseline(
        &self,
        binding: SyncBinding,
        root: LibraryRootId,
    ) -> Result<usize, Error> {
        self.writer.run(move |connection| {
            let tx = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;
            let root_uuid = root.to_string();
            super::sync_context::select(&tx, &root_uuid)?;
            let authorized: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sync_binding WHERE singleton=1 AND role='replica' AND library_id=?1 AND authority_id=?2 AND epoch=?3 AND local_root_uuid=?4)",
                    params![
                        binding.library_id.as_uuid().to_string(),
                        binding.authority_id.as_uuid().to_string(),
                        i64::try_from(binding.epoch.as_u64()).map_err(storage)?,
                        root_uuid,
                    ],
                    |row| row.get(0),
                )
                .map_err(storage)?;
            if !authorized {
                return Err(Error::conflict(
                    "replica baseline requires the current host binding and local root",
                ));
            }
            let marker = format!(
                "replica_enrollment_baseline:{root_uuid}:{}:{}:{}",
                binding.library_id.as_uuid(),
                binding.authority_id.as_uuid(),
                binding.epoch.as_u64(),
            );
            let completed: bool = tx
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sync_state WHERE key=?1)",
                    params![marker],
                    |row| row.get(0),
                )
                .map_err(storage)?;
            if completed {
                tx.commit().map_err(storage)?;
                return Ok(0);
            }
            let count = enqueue_current(&tx, &root_uuid)?;
            tx.execute(
                "INSERT INTO sync_state(key,value) VALUES(?1,?2)",
                params![marker, count.to_string()],
            )
            .map_err(storage)?;
            tx.commit().map_err(storage)?;
            Ok(count)
        })
    }

    /// Append current shared facts once, after any already queued local edits.
    /// The durable marker and every snapshot commit atomically. Local missing
    /// files and undoable deletes remain live shared objects until finalization.
    ///
    /// # Errors
    ///
    /// Rejects a replica, a different binding/root, corrupt canonical state or
    /// a storage failure. No queue rows or marker survive a failed transaction.
    pub fn ensure_host_sync_baseline(
        &self,
        binding: SyncBinding,
        root: LibraryRootId,
    ) -> Result<usize, Error> {
        self.writer.run(move |connection| {
            let tx = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;
            let library = binding.library_id.as_uuid().to_string();
            let authority = binding.authority_id.as_uuid().to_string();
            let epoch = i64::try_from(binding.epoch.as_u64()).map_err(storage)?;
            let root_uuid = root.to_string();
            let authorized: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_binding WHERE singleton=1 AND role='host' AND library_id=?1 AND authority_id=?2 AND epoch=?3 AND local_root_uuid=?4)",
                params![library, authority, epoch, root_uuid], |row| row.get(0),
            ).map_err(storage)?;
            if !authorized {
                return Err(Error::conflict("baseline requires the current host binding and local root"));
            }
            let completed: Option<String> = tx.query_row(
                "SELECT local_root_uuid FROM sync_host_baselines WHERE library_id=?1 AND authority_id=?2 AND epoch=?3",
                params![library, authority, epoch], |row| row.get(0),
            ).optional().map_err(storage)?;
            if let Some(completed_root) = completed {
                if completed_root != root_uuid {
                    return Err(Error::conflict("baseline belongs to another local root"));
                }
                return Ok(0);
            }
            let count = enqueue_current(&tx, &root_uuid)?;
            tx.execute(
                "INSERT INTO sync_host_baselines(library_id,authority_id,epoch,local_root_uuid,object_count,completed_at) VALUES(?1,?2,?3,?4,?5,?6)",
                params![library,authority,epoch,root_uuid,i64::try_from(count).map_err(storage)?,now_ms()],
            ).map_err(storage)?;
            tx.commit().map_err(storage)?;
            Ok(count)
        })
    }

    /// Read the exact frozen declarations awaiting upload for this local root.
    /// Different historical hashes for one path remain distinct declarations.
    ///
    /// # Errors
    ///
    /// Invalid declarations or database failures are reported without rewriting
    /// any immutable mutation payload.
    pub fn local_sync_resource_manifests(
        &self,
        root: LibraryRootId,
    ) -> Result<Vec<SharedResourceManifest>, Error> {
        self.with_reader(|connection| {
            let mut statement = connection.prepare(
                "SELECT m.resource_refs_json FROM sync_mutations m JOIN device_state d ON d.key='device_id' AND d.value=m.origin_device_id WHERE m.status='pending' AND (EXISTS(SELECT 1 FROM songs s WHERE s.uuid=m.object_uuid AND s.library_root_uuid=?1 AND m.object_type IN ('song','favorite','play-stats','override')) OR EXISTS(SELECT 1 FROM playlists p WHERE p.uuid=m.object_uuid AND p.library_root_uuid=?1 AND m.object_type='playlist')) ORDER BY m.origin_seq",
            ).map_err(storage)?;
            let rows = statement.query_map(params![root.to_string()], |row| row.get::<_,String>(0)).map_err(storage)?;
            let mut manifests = Vec::new();
            for row in rows {
                let frozen: Vec<SharedResourceManifest> = serde_json::from_str(&row.map_err(storage)?).map_err(storage)?;
                sync_manifests::validate(&frozen)?;
                for manifest in frozen {
                    if !manifests.contains(&manifest) {
                        manifests.push(manifest);
                    }
                }
            }
            Ok(manifests)
        })
    }

    /// Return resource declarations owned by objects in one local library root.
    /// Unlike `local_sync_resource_manifests`, this includes confirmed objects;
    /// the host needs their declarations to serve resources from its snapshot.
    pub fn local_library_resource_manifests(
        &self,
        root: LibraryRootId,
    ) -> Result<Vec<SharedResourceManifest>, Error> {
        self.with_reader(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT DISTINCT m.object_type,m.object_uuid FROM sync_resource_manifests m
                     WHERE (m.object_type IN ('song','favorite','play-stats','override') AND EXISTS (
                       SELECT 1 FROM songs s WHERE s.uuid=m.object_uuid AND s.library_root_uuid=?1))
                       OR (m.object_type IN ('playlist','playlist-item') AND EXISTS (
                       SELECT 1 FROM playlists p WHERE p.uuid=m.object_uuid AND p.library_root_uuid=?1))
                     ORDER BY m.object_type,m.object_uuid",
                )
                .map_err(storage)?;
            let objects = statement
                .query_map(params![root.to_string()], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })
                .map_err(storage)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage)?;
            let mut manifests = Vec::new();
            for (kind, id) in objects {
                for manifest in sync_manifests::load(connection, &kind, &id)? {
                    if !manifests.contains(&manifest) {
                        manifests.push(manifest);
                    }
                }
            }
            Ok(manifests)
        })
    }

    /// Resolve an audio resource to the catalog path that belongs to one local
    /// library root. Sync manifests can retain a publication path that differs
    /// from the current catalog path after a library move or a local rename.
    pub fn local_library_audio_path_for_resource(
        &self,
        root: LibraryRootId,
        content_hash: &str,
        length: u64,
    ) -> Result<Option<String>, Error> {
        let length = i64::try_from(length).map_err(storage)?;
        self.with_reader(|connection| {
            connection
                .query_row(
                    "SELECT relative_path FROM songs WHERE library_root_uuid=?1 AND blake3_hash=?2 AND file_size=?3 ORDER BY uuid LIMIT 1",
                    params![root.to_string(), content_hash, length],
                    |row| row.get(0),
                )
                .optional()
                .map_err(storage)
        })
    }
}

fn enqueue_current(connection: &Connection, root: &str) -> Result<usize, Error> {
    let mut count = 0;
    let mut statement = connection.prepare(
        "SELECT s.uuid,s.is_favorite,s.favorited_at FROM songs s WHERE s.library_root_uuid=?1 AND NOT EXISTS(SELECT 1 FROM tombstones t WHERE t.object_type='song' AND t.object_uuid=s.uuid) AND NOT EXISTS(SELECT 1 FROM sync_object_versions v WHERE v.object_type='song' AND v.object_uuid=s.uuid AND v.is_tombstone=1) AND NOT EXISTS(SELECT 1 FROM sync_song_aliases a WHERE a.alias_song_uuid=s.uuid) ORDER BY s.uuid",
    ).map_err(storage)?;
    let songs = statement
        .query_map(params![root], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, bool>(1)?,
                row.get::<_, Option<i64>>(2)?,
            ))
        })
        .map_err(storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage)?;
    for (uuid, favorite, favorited_at) in songs {
        let song = parse_id::<SongId>(&uuid, "SongId")?;
        let song_payload = statements::song_full_snapshot_payload(connection, song)?;
        declare_audio(connection, &uuid, &song_payload)?;
        let favorite_payload = serde_json::json!({"song_uuid":uuid,"is_favorite":favorite,"favorited_at":favorited_at});
        capture_legacy_plays(connection, song)?;
        let stats_payload = sync_projection::confirmed_play_stats_payload(connection, &uuid)?;
        for (kind, payload) in [
            (sync::KIND_SONG, song_payload),
            (sync::KIND_FAVORITE, favorite_payload),
            (sync::KIND_PLAY_STATS, stats_payload),
        ] {
            statements::enqueue_object_snapshot(
                connection,
                kind,
                &uuid,
                &payload,
                sync::OP_UPSERT,
            )?;
            count += 1;
        }
    }
    let mut statement = connection.prepare(
        "SELECT p.uuid FROM playlists p WHERE p.library_root_uuid=?1 AND NOT EXISTS(SELECT 1 FROM tombstones t WHERE t.object_type='playlist' AND t.object_uuid=p.uuid) AND NOT EXISTS(SELECT 1 FROM sync_object_versions v WHERE v.object_type='playlist' AND v.object_uuid=p.uuid AND v.is_tombstone=1) ORDER BY p.uuid",
    ).map_err(storage)?;
    let playlists = statement
        .query_map(params![root], |row| row.get::<_, String>(0))
        .map_err(storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage)?;
    for uuid in playlists {
        let playlist = parse_id::<PlaylistId>(&uuid, "PlaylistId")?;
        let payload = statements::playlist_full_snapshot_payload(connection, playlist)?;
        statements::enqueue_object_snapshot(
            connection,
            sync::KIND_PLAYLIST,
            &uuid,
            &payload,
            sync::OP_UPSERT,
        )?;
        count += 1;
    }
    Ok(count)
}

fn capture_legacy_plays(connection: &Connection, song: SongId) -> Result<(), Error> {
    let total: u64 = connection
        .query_row(
            "SELECT play_count FROM songs WHERE uuid=?1",
            params![song.to_string()],
            |row| row.get(0),
        )
        .map_err(storage)?;
    let counts = super::read_device_play_counts(connection, song)?;
    let assigned = counts.values().copied().fold(0_u64, u64::saturating_add);
    if total > assigned {
        let device: String = connection
            .query_row(
                "SELECT value FROM device_state WHERE key='device_id'",
                [],
                |row| row.get(0),
            )
            .map_err(storage)?;
        connection.execute("INSERT INTO sync_device_play_counts(song_uuid,device_id,play_count) VALUES(?1,?2,?3) ON CONFLICT(song_uuid,device_id) DO UPDATE SET play_count=play_count+excluded.play_count",params![song.to_string(),device,i64::try_from(total-assigned).map_err(storage)?]).map_err(storage)?;
    }
    Ok(())
}

fn declare_audio(
    connection: &Connection,
    uuid: &str,
    payload: &serde_json::Value,
) -> Result<(), Error> {
    let mut manifests = sync_manifests::load(connection, sync::KIND_SONG, uuid)?;
    let hash = payload["blake3_hash"]
        .as_str()
        .ok_or_else(|| Error::conflict("host baseline requires a scanned audio digest"))?;
    let length = payload["file_size"]
        .as_u64()
        .ok_or_else(|| Error::conflict("host baseline requires a scanned audio length"))?;
    let path = payload["relative_path"]
        .as_str()
        .ok_or_else(|| Error::conflict("host baseline requires a library relative audio path"))?;
    if manifests.iter().any(|manifest| {
        manifest.kind == SharedResourceKind::Audio
            && manifest.digest.to_hex() == hash
            && manifest.length == length
            && manifest.relative_path.as_str() == path
    }) {
        return Ok(());
    }
    manifests.retain(|manifest| manifest.kind != SharedResourceKind::Audio);
    let media_type = match payload["format"].as_str() {
        Some("mp3") => "audio/mpeg",
        Some("flac") => "audio/flac",
        Some("ape") => "audio/ape",
        Some("m4a") => "audio/mp4",
        Some("ogg" | "opus") => "audio/ogg",
        Some("wav") => "audio/wav",
        _ => {
            return Err(Error::conflict(
                "host baseline requires a recognized scanned audio format",
            ))
        }
    };
    let wire: sync_manifests::WireResource = serde_json::from_value(serde_json::json!({
        "kind":SharedResourceKind::Audio,"content_hash":hash,"length":length,"media_type":media_type,"relative_path":path,
    })).map_err(storage)?;
    manifests.push(wire.into_manifest()?);
    sync_manifests::replace(connection, sync::KIND_SONG, uuid, &manifests)
}
