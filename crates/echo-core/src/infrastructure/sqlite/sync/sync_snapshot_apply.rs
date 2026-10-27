//! Crash safe enrollment from a fixed host snapshot.

use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

use crate::application::ports::{
    AppliedSnapshotPage, HostSnapshotRecord, ReplicaSnapshotApplyPort, ReplicaSnapshotPage,
};
use crate::domain::sync::{DeviceId, SharedResourceKind, SharedResourceManifest, SyncBinding};
use crate::error::Error;

use super::super::{
    support::{now_ms, storage},
    SqliteDatabase,
};
use super::apply::object_identity;
use super::snapshot::MAX_PAGE_BYTES;

impl SqliteDatabase {
    /// Whether this exact binding is enrolling the selected root. The marker
    /// permits a non-empty local catalog to merge with the host snapshot.
    pub fn replica_catalog_enrollment_pending(
        &self,
        root: crate::domain::ids::LibraryRootId,
        binding: SyncBinding,
    ) -> Result<bool, Error> {
        self.with_reader(move |connection| {
            replacement_requested(connection, &root.to_string(), binding)
        })
    }

    /// Whether normal local scan/import recovery must wait for host
    /// enrollment's durable replace step to finish for this root.
    pub fn replica_enrollment_pending(
        &self,
        root: crate::domain::ids::LibraryRootId,
    ) -> Result<bool, Error> {
        self.with_reader(move |connection| {
            connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sync_state WHERE key=?1)",
                    params![format!("replica_enrollment_replace:{root}")],
                    |row| row.get(0),
                )
                .map_err(storage)
        })
    }

    /// Whether this host identity has a durable replica cursor. A zero cursor
    /// is valid for a successfully completed empty-host snapshot.
    pub fn replica_enrollment_complete(&self, host: DeviceId) -> Result<bool, Error> {
        self.with_reader(|connection| {
            connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM sync_peer_cursors WHERE peer_device_id=?1)",
                    params![host.as_uuid().to_string()],
                    |row| row.get(0),
                )
                .map_err(storage)
        })
    }

    /// Resume the exact persisted snapshot page after process restart.
    pub fn replica_snapshot_progress(
        &self,
        host: DeviceId,
    ) -> Result<Option<(String, u32)>, Error> {
        self.with_reader(|connection| {
            connection
                .query_row(
                    "SELECT token,next_page_index FROM sync_replica_snapshot_sessions WHERE peer_device_id=?1 AND completed=0",
                    params![host.as_uuid().to_string()],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(storage)
        })
    }
}

impl ReplicaSnapshotApplyPort for SqliteDatabase {
    fn apply_snapshot_page(
        &self,
        peer_device_id: DeviceId,
        page: &ReplicaSnapshotPage,
    ) -> Result<AppliedSnapshotPage, Error> {
        let body = validate_page(page)?;
        let page = page.clone();
        let control = self.library_control();
        self.writer.run(move |connection| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(storage)?;
            let root = replica_root(&tx, page.checkpoint.binding)?;
            let peer = peer_device_id.as_uuid().to_string();
            let session = load_session(&tx, &peer)?;
            if let Some(session) = session {
                if session.token != page.snapshot_token || session.binding != page.checkpoint.binding
                    || session.sequence != page.checkpoint.sequence.as_u64() || session.digest != page.checkpoint.digest
                {
                    return Err(Error::conflict("snapshot session token or frozen checkpoint changed"));
                }
                let saved: Option<(Vec<u8>, String)> = tx.query_row(
                    "SELECT body_digest, body_json FROM sync_replica_snapshot_pages WHERE token=?1 AND page_index=?2",
                    params![page.snapshot_token, page.page_index], |row| Ok((row.get(0)?, row.get(1)?)),
                ).optional().map_err(storage)?;
                if let Some((digest, saved_body)) = saved {
                    if saved_body != body || digest.as_slice() != blake3::hash(body.as_bytes()).as_bytes() {
                        return Err(Error::conflict("replayed snapshot page differs from durable content"));
                    }
                    tx.commit().map_err(storage)?;
                    return Ok(AppliedSnapshotPage {
                        next_page_index: page.next_page_index,
                        checkpoint: session.completed.then_some(page.checkpoint), applied: 0,
                    });
                }
                if session.completed || session.next_page != page.page_index {
                    return Err(Error::conflict("snapshot page is out of sequence"));
                }
            } else {
                if page.page_index != 0 {
                    return Err(Error::conflict("snapshot enrollment must start at page zero"));
                }
                ensure_enrollment_target(&tx, &root, page.checkpoint.binding)?;
                let enrolling: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM sync_replica_snapshot_sessions)", [], |row| row.get(0)).map_err(storage)?;
                if enrolling {
                    return Err(Error::conflict("replica already has a snapshot enrollment"));
                }
                tx.execute(
                    "INSERT INTO sync_replica_snapshot_sessions(peer_device_id,token,library_id,authority_id,epoch,commit_seq,commit_digest,next_page_index) VALUES(?1,?2,?3,?4,?5,?6,?7,0)",
                    params![peer,page.snapshot_token,page.checkpoint.binding.library_id.as_uuid().to_string(),page.checkpoint.binding.authority_id.as_uuid().to_string(),i64::try_from(page.checkpoint.binding.epoch.as_u64()).map_err(storage)?,i64::try_from(page.checkpoint.sequence.as_u64()).map_err(storage)?,page.checkpoint.digest.as_slice()],
                ).map_err(storage)?;
            }
            // Enrollment is local-first: the snapshot projects into the existing
            // catalog after its current state has been queued for the host.
            ensure_enrollment_target(&tx, &root, page.checkpoint.binding)?;
            tx.execute(
                "INSERT INTO sync_replica_snapshot_pages(token,page_index,body_digest,body_json) VALUES(?1,?2,?3,?4)",
                params![page.snapshot_token,page.page_index,blake3::hash(body.as_bytes()).as_bytes().as_slice(),body],
            ).map_err(storage)?;
            for record in &page.records {
                let identity = record_identity(record);
                let duplicate: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM sync_replica_snapshot_records WHERE token=?1 AND identity=?2)", params![page.snapshot_token,identity], |row| row.get(0)).map_err(storage)?;
                if duplicate {
                    return Err(Error::conflict("snapshot repeats a typed object or alias identity"));
                }
                tx.execute("INSERT INTO sync_replica_snapshot_records(token,identity,record_json) VALUES(?1,?2,?3)", params![page.snapshot_token,identity,serde_json::to_string(record).map_err(storage)?]).map_err(storage)?;
            }
            if page.next_page_index.is_none() {
                ensure_snapshot_root_ownership(&tx, &page.snapshot_token, &root)?;
                finish_snapshot(&tx, &page, &root)?;
                tx.execute(
                    "INSERT INTO sync_peer_cursors(peer_device_id,applied_commit_seq,applied_digest,resource_bytes_complete,updated_at) VALUES(?1,?2,?3,0,?4)",
                    params![peer,i64::try_from(page.checkpoint.sequence.as_u64()).map_err(storage)?,page.checkpoint.digest.as_slice(),now_ms()],
                ).map_err(storage)?;
                tx.execute("UPDATE sync_replica_snapshot_sessions SET completed=1 WHERE token=?1",params![page.snapshot_token]).map_err(storage)?;
                tx.execute("DELETE FROM sync_replica_snapshot_records WHERE token=?1",params![page.snapshot_token]).map_err(storage)?;
                tx.execute("DELETE FROM sync_state WHERE key=?1", params![replacement_key(&root)]).map_err(storage)?;
            } else {
                tx.execute("UPDATE sync_replica_snapshot_sessions SET next_page_index=?2 WHERE token=?1",params![page.snapshot_token,page.next_page_index]).map_err(storage)?;
            }
            if page.next_page_index.is_none() {
                let root_id = crate::domain::ids::LibraryRootId::from_uuid(uuid::Uuid::parse_str(&root).map_err(storage)?);
                let operation = super::durability::identity("snapshot", &format!("{}:{}",page.checkpoint.binding.library_id.as_uuid(),page.snapshot_token));
                super::durability::commit_projection(control,tx,root_id,crate::domain::library::durability::ChangeOrigin::Remote,operation)?;
            } else {
                // Unverified pages remain private staging; no business object or
                // applied cursor is published until the complete root commits.
                tx.commit().map_err(storage)?;
            }
            Ok(AppliedSnapshotPage {
                next_page_index: page.next_page_index,
                checkpoint: page.next_page_index.is_none().then_some(page.checkpoint),
                applied: page.records.len(),
            })
        })
    }

    fn discard_snapshot(
        &self,
        peer_device_id: DeviceId,
        snapshot_token: &str,
    ) -> Result<(), Error> {
        let token = snapshot_token.to_owned();
        self.writer.run(move |connection| {
            let tx = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;
            if let Some(session) = load_session(&tx, &peer_device_id.as_uuid().to_string())? {
                if session.token != token || session.completed {
                    return Err(Error::conflict(
                        "cannot discard a different or completed snapshot",
                    ));
                }
                tx.execute(
                    "DELETE FROM sync_replica_snapshot_sessions WHERE token=?1",
                    params![token],
                )
                .map_err(storage)?;
            }
            tx.commit().map_err(storage)
        })
    }
}

fn validate_page(page: &ReplicaSnapshotPage) -> Result<String, Error> {
    let valid_token = uuid::Uuid::parse_str(&page.snapshot_token)
        .is_ok_and(|token| token.simple().to_string() == page.snapshot_token);
    if !valid_token
        || page.records.len() > 512
        || page
            .next_page_index
            .is_some_and(|next| page.page_index.checked_add(1) != Some(next))
        || (page.next_page_index.is_some() && page.records.is_empty())
        || (page.checkpoint.sequence.as_u64() == 0 && page.checkpoint.digest != [0; 32])
        || (page.checkpoint.sequence.as_u64() == 0 && !page.records.is_empty())
    {
        return Err(Error::conflict(
            "snapshot page limits or checkpoint are invalid",
        ));
    }
    let mut resources = 0usize;
    for record in &page.records {
        match record {
            HostSnapshotRecord::Object {
                candidate,
                version,
                resources: refs,
            } => {
                let sequence = version.commit_seq().as_u64();
                if sequence == 0
                    || sequence > page.checkpoint.sequence.as_u64()
                    || candidate.origin_seq == 0
                {
                    return Err(Error::conflict(
                        "snapshot object version is outside the frozen checkpoint",
                    ));
                }
                resources = resources
                    .checked_add(refs.len())
                    .ok_or_else(|| Error::conflict("too many snapshot resource references"))?;
                for resource in refs {
                    SharedResourceManifest::new(
                        resource.kind,
                        resource.digest,
                        resource.length,
                        resource.media_type.clone(),
                        resource.relative_path.as_str(),
                    )?;
                    i64::try_from(resource.length).map_err(storage)?;
                }
            }
            HostSnapshotRecord::SongAlias { alias, canonical } if alias == canonical => {
                return Err(Error::conflict("snapshot contains a song self alias"));
            }
            HostSnapshotRecord::PlaylistMemberAlias(alias)
                if alias.alias_member_id == alias.canonical_member_id =>
            {
                return Err(Error::conflict("snapshot contains a member self alias"));
            }
            _ => {}
        }
    }
    if resources + page.records.len() > 512 {
        return Err(Error::conflict(
            "snapshot resource reference limit exceeded",
        ));
    }
    let body = serde_json::to_string(page).map_err(storage)?;
    if body.len() > MAX_PAGE_BYTES {
        return Err(Error::conflict("snapshot page exceeds 16 MiB"));
    }
    Ok(body)
}

struct Session {
    token: String,
    binding: SyncBinding,
    sequence: u64,
    digest: [u8; 32],
    next_page: u32,
    completed: bool,
}

fn load_session(connection: &Connection, peer: &str) -> Result<Option<Session>, Error> {
    let row = connection.query_row(
        "SELECT token,library_id,authority_id,epoch,commit_seq,commit_digest,next_page_index,completed FROM sync_replica_snapshot_sessions WHERE peer_device_id=?1",
        params![peer], |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?,row.get::<_,u64>(3)?,row.get::<_,u64>(4)?,row.get::<_,Vec<u8>>(5)?,row.get::<_,u32>(6)?,row.get::<_,bool>(7)?)),
    ).optional().map_err(storage)?;
    row.map(|row| {
        Ok(Session {
            token: row.0,
            binding: SyncBinding {
                library_id: crate::domain::sync::LibraryId::from_uuid(
                    uuid::Uuid::parse_str(&row.1).map_err(storage)?,
                ),
                authority_id: crate::domain::sync::AuthorityId::from_uuid(
                    uuid::Uuid::parse_str(&row.2).map_err(storage)?,
                ),
                epoch: crate::domain::sync::Epoch::from_u64(row.3),
            },
            sequence: row.4,
            digest: row.5.as_slice().try_into().map_err(storage)?,
            next_page: row.6,
            completed: row.7,
        })
    })
    .transpose()
}

fn replica_root(connection: &Connection, binding: SyncBinding) -> Result<String, Error> {
    connection.query_row(
        "SELECT local_root_uuid FROM sync_binding WHERE singleton=1 AND role='replica' AND local_root_uuid IS NOT NULL AND library_id=?1 AND authority_id=?2 AND epoch=?3",
        params![binding.library_id.as_uuid().to_string(),binding.authority_id.as_uuid().to_string(),i64::try_from(binding.epoch.as_u64()).map_err(storage)?],
        |row| row.get(0),
    ).optional().map_err(storage)?.ok_or_else(|| Error::conflict("snapshot apply requires the matching replica binding"))
}

fn ensure_empty_replica(connection: &Connection, root: &str) -> Result<(), Error> {
    let nonempty: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_mutations WHERE status='pending' AND local_root_uuid=?1) OR EXISTS(SELECT 1 FROM sync_outbox WHERE pushed_at IS NULL AND local_root_uuid=?1) OR EXISTS(SELECT 1 FROM sync_object_versions) OR EXISTS(SELECT 1 FROM sync_commits) OR EXISTS(SELECT 1 FROM sync_peer_cursors) OR EXISTS(SELECT 1 FROM songs WHERE library_root_uuid=?1) OR EXISTS(SELECT 1 FROM playlists WHERE library_root_uuid=?1) OR EXISTS(SELECT 1 FROM sync_local_delete_intents i JOIN songs s ON s.uuid=i.object_uuid WHERE i.state='open' AND s.library_root_uuid=?1) OR EXISTS(SELECT 1 FROM sync_song_aliases) OR EXISTS(SELECT 1 FROM sync_member_aliases) OR EXISTS(SELECT 1 FROM sync_resource_refs r WHERE EXISTS(SELECT 1 FROM songs s WHERE s.uuid=r.object_uuid AND s.library_root_uuid=?1) OR EXISTS(SELECT 1 FROM playlists p WHERE p.uuid=r.object_uuid AND p.library_root_uuid=?1)) OR EXISTS(SELECT 1 FROM sync_resource_manifests r WHERE EXISTS(SELECT 1 FROM songs s WHERE s.uuid=r.object_uuid AND s.library_root_uuid=?1) OR EXISTS(SELECT 1 FROM playlists p WHERE p.uuid=r.object_uuid AND p.library_root_uuid=?1))",
        params![root], |row| row.get(0),
    ).map_err(storage)?;
    if nonempty {
        Err(Error::conflict(
            "snapshot enrollment requires an empty replica without local pending edits",
        ))
    } else {
        Ok(())
    }
}

fn replacement_key(root: &str) -> String {
    format!("replica_enrollment_replace:{root}")
}

fn replacement_requested(
    connection: &Connection,
    root: &str,
    binding: SyncBinding,
) -> Result<bool, Error> {
    let saved: Option<String> = connection
        .query_row(
            "SELECT value FROM sync_state WHERE key=?1",
            params![replacement_key(root)],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage)?;
    saved
        .map(|value| {
            serde_json::from_str::<SyncBinding>(&value)
                .map(|saved| saved == binding)
                .map_err(storage)
        })
        .transpose()
        .map(|requested| requested.unwrap_or(false))
}

fn ensure_enrollment_target(
    connection: &Connection,
    root: &str,
    binding: SyncBinding,
) -> Result<(), Error> {
    if replacement_requested(connection, root, binding)? {
        Ok(())
    } else {
        ensure_empty_replica(connection, root)
    }
}

fn record_identity(record: &HostSnapshotRecord) -> String {
    match record {
        HostSnapshotRecord::Object { candidate, .. } => {
            let (kind, id) = object_identity(candidate.object);
            format!("object:{kind}:{id}")
        }
        HostSnapshotRecord::SongAlias { alias, .. } => format!("song-alias:{alias}"),
        HostSnapshotRecord::PlaylistMemberAlias(alias) => format!(
            "member-alias:{}:{}",
            alias.playlist_id, alias.alias_member_id
        ),
    }
}

fn ensure_snapshot_root_ownership(
    connection: &Connection,
    token: &str,
    root: &str,
) -> Result<(), Error> {
    visit_records(connection, token, |record| {
        let objects = match record {
            HostSnapshotRecord::Object { candidate, .. } => {
                vec![object_identity(candidate.object)]
            }
            HostSnapshotRecord::SongAlias { alias, canonical } => {
                vec![("song", alias.to_string()), ("song", canonical.to_string())]
            }
            HostSnapshotRecord::PlaylistMemberAlias(alias) => vec![
                ("playlist", alias.playlist_id.to_string()),
                ("playlist-item", alias.alias_member_id.to_string()),
                ("playlist-item", alias.canonical_member_id.to_string()),
            ],
        };
        for (kind, id) in objects {
            let query = match kind {
                "song" | "favorite" | "override" | "play-stats" | "tombstone" => "SELECT EXISTS(SELECT 1 FROM songs WHERE uuid=?1 AND library_root_uuid<>?2)",
                "playlist" => "SELECT EXISTS(SELECT 1 FROM playlists WHERE uuid=?1 AND library_root_uuid<>?2)",
                "playlist-item" => "SELECT EXISTS(SELECT 1 FROM playlist_songs m JOIN playlists p ON p.uuid=m.playlist_uuid WHERE m.member_uuid=?1 AND p.library_root_uuid<>?2)",
                _ => continue,
            };
            let other_root: bool = connection
                .query_row(query, params![id, root], |row| row.get(0))
                .map_err(storage)?;
            if other_root {
                return Err(Error::conflict(
                    "snapshot object identity belongs to another local root",
                ));
            }
        }
        Ok(())
    })
}

fn visit_records(
    connection: &Connection,
    token: &str,
    mut visit: impl FnMut(HostSnapshotRecord) -> Result<(), Error>,
) -> Result<(), Error> {
    let mut statement = connection.prepare("SELECT record_json FROM sync_replica_snapshot_records WHERE token=?1 ORDER BY identity").map_err(storage)?;
    let rows = statement
        .query_map(params![token], |row| row.get::<_, String>(0))
        .map_err(storage)?;
    for row in rows {
        visit(serde_json::from_str(&row.map_err(storage)?).map_err(storage)?)?;
    }
    Ok(())
}

fn finish_snapshot(
    connection: &Connection,
    page: &ReplicaSnapshotPage,
    root: &str,
) -> Result<(), Error> {
    // Persist every confirmed base first. Projection can then resolve cross-page
    // dependencies and tombstones regardless of the transmitted record order.
    visit_records(connection, &page.snapshot_token, |record| {
        match record {
            HostSnapshotRecord::Object {
                candidate,
                version,
                resources,
            } => {
                let (kind, id) = object_identity(candidate.object);
                let deleted = candidate.payload == serde_json::json!({"tombstone":true});
                let payload = (!deleted).then(|| candidate.payload.to_string());
                let sequence = i64::try_from(version.commit_seq().as_u64()).map_err(storage)?;
                connection.execute("INSERT INTO sync_object_versions(object_type,object_uuid,object_version,commit_seq,payload_json,is_tombstone,changed_at_ms) VALUES(?1,?2,?3,?3,?4,?5,0) ON CONFLICT(object_type,object_uuid) DO UPDATE SET object_version=excluded.object_version,commit_seq=excluded.commit_seq,payload_json=excluded.payload_json,is_tombstone=excluded.is_tombstone",params![kind,id,sequence,payload,i64::from(deleted)]).map_err(storage)?;
                for resource in resources {
                    persist_resource(connection, kind, &id, &resource)?;
                }
            }
            HostSnapshotRecord::SongAlias { alias, canonical } => {
                connection.execute("INSERT INTO sync_song_aliases(alias_song_uuid,canonical_song_uuid,commit_seq) VALUES(?1,?2,?3)",params![alias.to_string(),canonical.to_string(),i64::try_from(page.checkpoint.sequence.as_u64()).map_err(storage)?]).map_err(storage)?;
            }
            HostSnapshotRecord::PlaylistMemberAlias(alias) => {
                connection.execute("INSERT INTO sync_member_aliases(playlist_uuid,alias_member_uuid,canonical_member_uuid,commit_seq) VALUES(?1,?2,?3,?4)",params![alias.playlist_id.to_string(),alias.alias_member_id.to_string(),alias.canonical_member_id.to_string(),i64::try_from(page.checkpoint.sequence.as_u64()).map_err(storage)?]).map_err(storage)?;
            }
        }
        Ok(())
    })?;
    validate_aliases(connection)?;
    // Resolve existing local aliases before song projection, so deduplication
    // can retain their playable media and release any colliding local path.
    visit_records(connection, &page.snapshot_token, |record| {
        if let HostSnapshotRecord::SongAlias { alias, canonical } = record {
            super::song_aliases::persist(
                connection,
                &alias.to_string(),
                &canonical.to_string(),
                i64::try_from(page.checkpoint.sequence.as_u64()).map_err(storage)?,
            )?;
        }
        Ok(())
    })?;
    // Song projection replays waiting playlist/favorite/stat bases. Because all
    // versions already exist, deleted songs/members are excluded consistently.
    visit_records(connection, &page.snapshot_token, |record| {
        if let HostSnapshotRecord::Object { candidate, .. } = record {
            let (kind, id) = object_identity(candidate.object);
            if candidate.payload == serde_json::json!({"tombstone":true}) {
                // A first snapshot is a merge with the replica's existing
                // catalog. A host tombstone may refer to an object that was
                // created independently on this device (or retained locally
                // after a previous pairing); do not hide that local object
                // before its queued local baseline has had a chance to reach
                // the host. Tombstones for objects absent locally still
                // advance the confirmed ledger without creating a local row.
                if local_object_exists(connection, kind, &id, root)? {
                    return Ok(());
                }
                super::projection::project_confirmed_tombstone(connection, kind, &id)?;
            } else {
                super::projection::project_confirmed_song_state(
                    connection,
                    kind,
                    &id,
                    &candidate.payload,
                    Some(root),
                )?;
            }
        }
        Ok(())
    })?;
    visit_records(connection, &page.snapshot_token, |record| {
        if let HostSnapshotRecord::SongAlias { alias, canonical } = record {
            super::song_aliases::project(connection, &alias.to_string(), &canonical.to_string())?;
        }
        Ok(())
    })
}

fn local_object_exists(
    connection: &Connection,
    kind: &str,
    id: &str,
    root: &str,
) -> Result<bool, Error> {
    let query = match kind {
        "song" | "favorite" | "override" | "play-stats" => {
            "SELECT EXISTS(SELECT 1 FROM songs WHERE uuid=?1 AND library_root_uuid=?2)"
        }
        "playlist" => {
            "SELECT EXISTS(SELECT 1 FROM playlists WHERE uuid=?1 AND library_root_uuid=?2)"
        }
        "playlist-item" => {
            "SELECT EXISTS(SELECT 1 FROM playlist_songs m JOIN playlists p ON p.uuid=m.playlist_uuid WHERE m.member_uuid=?1 AND p.library_root_uuid=?2)"
        }
        _ => return Ok(false),
    };
    connection
        .query_row(query, params![id, root], |row| row.get(0))
        .map_err(storage)
}

fn persist_resource(
    connection: &Connection,
    kind: &str,
    id: &str,
    resource: &SharedResourceManifest,
) -> Result<(), Error> {
    let resource_kind = match resource.kind {
        SharedResourceKind::Audio => "audio",
        SharedResourceKind::LyricsSidecar => "lyrics_sidecar",
        SharedResourceKind::UserCover => "user_cover",
        SharedResourceKind::PlaylistCover => "playlist_cover",
    };
    let digest = resource.digest.as_bytes();
    let saved: Option<(i64, String)> = connection
        .query_row(
            "SELECT byte_length,media_type FROM sync_resources WHERE content_hash=?1",
            params![digest.as_slice()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(storage)?;
    if let Some((length, media_type)) = saved {
        if u64::try_from(length).map_err(storage)? != resource.length
            || media_type != resource.media_type
        {
            return Err(Error::conflict(
                "snapshot has conflicting metadata for one resource digest",
            ));
        }
    } else {
        connection.execute("INSERT INTO sync_resources(content_hash,resource_kind,byte_length,media_type,library_relative_path,local_state) VALUES(?1,?2,?3,?4,?5,'missing')",params![digest.as_slice(),resource_kind,i64::try_from(resource.length).map_err(storage)?,resource.media_type,resource.relative_path.as_str()]).map_err(storage)?;
    }
    connection.execute("INSERT OR IGNORE INTO sync_resource_refs(object_type,object_uuid,content_hash,ref_kind) VALUES(?1,?2,?3,?4)",params![kind,id,digest.as_slice(),resource_kind]).map_err(storage)?;
    connection.execute("INSERT OR IGNORE INTO sync_resource_manifests(object_type,object_uuid,content_hash,ref_kind,library_relative_path,byte_length,media_type) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![kind,id,digest.as_slice(),resource_kind,resource.relative_path.as_str(),i64::try_from(resource.length).map_err(storage)?,resource.media_type]).map_err(storage)?;
    Ok(())
}

fn validate_aliases(connection: &Connection) -> Result<(), Error> {
    // Domain aliases resolve directly to canonical identities. An alias target
    // that is itself an alias would allow cycles and change historical meaning.
    let chained: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_song_aliases a JOIN sync_song_aliases b ON a.canonical_song_uuid=b.alias_song_uuid) OR EXISTS(SELECT 1 FROM sync_member_aliases a JOIN sync_member_aliases b ON a.playlist_uuid=b.playlist_uuid AND a.canonical_member_uuid=b.alias_member_uuid)",
        [], |row| row.get(0),
    ).map_err(storage)?;
    if chained {
        Err(Error::conflict(
            "snapshot aliases must resolve directly to canonical identities",
        ))
    } else {
        Ok(())
    }
}
