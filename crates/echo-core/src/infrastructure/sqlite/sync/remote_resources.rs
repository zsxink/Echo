//! Resource publication journal; no outbox or local revision effects.

#![allow(clippy::missing_errors_doc)]

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};

use crate::application::ports::{
    RemoteResourceJournalPort, RemoteResourcePublication, RemoteResourceStage, RemoteResourceTarget,
};
use crate::domain::ids::{LibraryRootId, OperationId, RelativeMediaPath, SongId};
use crate::domain::sync::{ObjectKey, ResourceDigest, SharedResourceKind, SharedResourceManifest};
use crate::error::Error;

use super::super::{SqliteDatabase, support::storage};
use super::sync_manifests;

fn validate_current(
    connection: &Connection,
    publication: &RemoteResourcePublication,
) -> Result<(), Error> {
    let bound: Option<String> = connection
        .query_row(
            "SELECT local_root_uuid FROM sync_binding WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage)?
        .flatten();
    if bound.as_deref() != Some(publication.root.to_string().as_str()) {
        return Err(Error::conflict(
            "resource publication root is not the bound library",
        ));
    }
    let (kind, id) = sync_manifests::identity(publication.object);
    if let ObjectKey::Song(song) = publication.object {
        let alias: bool = connection
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_song_aliases WHERE alias_song_uuid=?1)",
                params![song.to_string()],
                |row| row.get(0),
            )
            .map_err(storage)?;
        let tombstoned: bool = connection
            .query_row(
                "SELECT COALESCE((SELECT is_tombstone FROM sync_object_versions WHERE object_type='song' AND object_uuid=?1 ORDER BY commit_seq DESC LIMIT 1),0)",
                params![song.to_string()],
                |row| row.get(0),
            )
            .map_err(storage)?;
        if alias || tombstoned {
            return Err(Error::conflict(
                "resource publication object is an alias or final tombstone",
            ));
        }
    }
    let current = sync_manifests::load(connection, kind, &id)?;
    let expected = publication
        .items
        .iter()
        .map(|item| item.target.manifest.clone())
        .collect::<Vec<_>>();
    sync_manifests::validate(&expected)?;
    if current.len() != expected.len()
        || expected.iter().any(|resource| !current.contains(resource))
    {
        return Err(Error::conflict(
            "resource publication declaration is stale or unauthorized",
        ));
    }
    for item in &publication.items {
        let path = RelativeMediaPath::new(&item.local_path)?;
        if let ObjectKey::Song(song) = publication.object {
            let owner: Option<String> = connection.query_row("SELECT uuid FROM songs WHERE library_root_uuid=?1 AND normalized_relative_path=?2 AND uuid<>?3", params![publication.root.to_string(),path.identity_key(),song.to_string()],|row| row.get(0)).optional().map_err(storage)?;
            if owner.is_some() {
                return Err(Error::conflict("resource path belongs to another song"));
            }
        }
    }
    Ok(())
}

fn immutable_declaration(publication: &RemoteResourcePublication) -> Result<String, Error> {
    serde_json::to_string(
        &publication
            .items
            .iter()
            .map(|item| &item.target)
            .collect::<Vec<_>>(),
    )
    .map_err(storage)
}

fn load(
    connection: &Connection,
    root: LibraryRootId,
    operation: OperationId,
) -> Result<RemoteResourcePublication, Error> {
    let json: Option<String> = connection.query_row("SELECT publication_json FROM sync_remote_publications WHERE local_root_uuid=?1 AND operation_uuid=?2",params![root.to_string(),operation.to_string()],|row| row.get(0)).optional().map_err(storage)?;
    serde_json::from_str(
        &json.ok_or_else(|| Error::conflict("unknown resource publication operation"))?,
    )
    .map_err(storage)
}

impl RemoteResourceJournalPort for SqliteDatabase {
    fn prepare(&self, publication: &RemoteResourcePublication) -> Result<(), Error> {
        let mut publication = publication.clone();
        let control = self.library_control();
        self.writer.run(move |connection| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(storage)?;
            validate_current(&tx, &publication)?;
            let declaration = immutable_declaration(&publication)?;
            let existing: Option<(String,String)> = tx.query_row("SELECT declaration_json,local_root_uuid FROM sync_remote_publications WHERE operation_uuid=?1",params![publication.operation.to_string()],|row| Ok((row.get(0)?,row.get(1)?))).optional().map_err(storage)?;
            if let Some((saved, root)) = existing {
                if saved != declaration || root != publication.root.to_string() || load(&tx,publication.root,publication.operation)?.object != publication.object {
                    return Err(Error::conflict("resource operation identity reused with different declaration"));
                }
                return tx.commit().map_err(storage);
            }
            let (kind,id) = sync_manifests::identity(publication.object);
            for item in &mut publication.items {
                let previous: Option<(Vec<u8>,String)> = tx.query_row("SELECT content_hash,local_relative_path FROM sync_remote_resource_locations WHERE local_root_uuid=?1 AND object_type=?2 AND object_uuid=?3 AND library_relative_path=?4",params![publication.root.to_string(),kind,id,item.target.manifest.relative_path.as_str()],|row| Ok((row.get(0)?,row.get(1)?))).optional().map_err(storage)?;
                match (previous,item.target.expected_old_digest) {
                    (Some((digest,path)),Some(expected)) if digest.as_slice()==expected.as_bytes().as_slice() => item.old_path=Some(path),
                    (None,None) => {},
                    _ => return Err(Error::conflict("resource publication expected old hash differs from durable association")),
                }
            }
            tx.execute("INSERT INTO sync_remote_publications(operation_uuid,local_root_uuid,object_type,object_uuid,declaration_json,publication_json) VALUES(?1,?2,?3,?4,?5,?6)",params![publication.operation.to_string(),publication.root.to_string(),kind,id,declaration,serde_json::to_string(&publication).map_err(storage)?]).map_err(storage)?;
            for item in &publication.items {
                let path = RelativeMediaPath::new(&item.local_path)?;
                let existing_claim: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM operation_items i JOIN operation_journal j ON j.operation_uuid=i.operation_uuid WHERE j.library_root_uuid=?1 AND i.normalized_target_path=?2 AND i.claim_active=1)",params![publication.root.to_string(),path.identity_key()],|row| row.get(0)).map_err(storage)?;
                if existing_claim { return Err(Error::conflict("resource target is claimed by a local file operation")); }
                tx.execute("INSERT INTO sync_remote_publication_claims(operation_uuid,local_root_uuid,normalized_relative_path) VALUES(?1,?2,?3)",params![publication.operation.to_string(),publication.root.to_string(),path.identity_key()]).map_err(storage)?;
            }
            let operation = transition_operation(&publication, "prepare")?;
            super::durability::commit_resource_projection(control, tx, publication.root, operation, None)
        })
    }

    fn load(
        &self,
        root: LibraryRootId,
        operation: OperationId,
    ) -> Result<RemoteResourcePublication, Error> {
        self.with_reader(|connection| {
            let publication = load(connection, root, operation)?;
            if publication.completed {
                validate_current(connection, &publication)?;
            }
            Ok(publication)
        })
    }

    fn save(&self, publication: &RemoteResourcePublication) -> Result<(), Error> {
        let publication = publication.clone();
        let control = self.library_control();
        self.writer.run(move |connection| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(storage)?;
            let existing = load(&tx,publication.root,publication.operation)?;
            if existing.completed || existing.object != publication.object || immutable_declaration(&existing)? != immutable_declaration(&publication)? || existing.items.iter().zip(&publication.items).any(|(old,new)| old.local_path!=new.local_path || old.staging_path!=new.staging_path || old.old_path!=new.old_path) {
                return Err(Error::conflict("resource journal update differs from its immutable intent"));
            }
            tx.execute("UPDATE sync_remote_publications SET publication_json=?2 WHERE operation_uuid=?1 AND completed=0",params![publication.operation.to_string(),serde_json::to_string(&publication).map_err(storage)?]).map_err(storage)?;
            let operation = transition_operation(&publication, "save")?;
            super::durability::commit_resource_projection(control, tx, publication.root, operation, None)
        })
    }

    fn finish(&self, publication: &RemoteResourcePublication) -> Result<(), Error> {
        let mut publication = publication.clone();
        let control = self.library_control();
        self.writer.run(move |connection| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(storage)?;
            validate_current(&tx,&publication)?;
            let saved = load(&tx,publication.root,publication.operation)?;
            if saved.completed { return tx.commit().map_err(storage); }
            if saved.object != publication.object || immutable_declaration(&saved)? != immutable_declaration(&publication)? || publication.items.iter().any(|item| item.stage != RemoteResourceStage::Published) || saved.items != publication.items {
                return Err(Error::conflict("resource publication has not durably completed every file"));
            }
            let (kind,id) = sync_manifests::identity(publication.object);
            tx.execute("DELETE FROM sync_remote_resource_locations WHERE local_root_uuid=?1 AND object_type=?2 AND object_uuid=?3",params![publication.root.to_string(),kind,id]).map_err(storage)?;
            for item in &publication.items {
                let resource = &item.target.manifest;
                tx.execute("INSERT INTO sync_remote_resource_locations(local_root_uuid,object_type,object_uuid,content_hash,library_relative_path,local_relative_path,operation_uuid) VALUES(?1,?2,?3,?4,?5,?6,?7) ON CONFLICT(local_root_uuid,object_type,object_uuid,library_relative_path) DO UPDATE SET content_hash=excluded.content_hash,local_relative_path=excluded.local_relative_path,operation_uuid=excluded.operation_uuid",params![publication.root.to_string(),kind,id,resource.digest.as_bytes().as_slice(),resource.relative_path.as_str(),item.local_path,publication.operation.to_string()]).map_err(storage)?;
                tx.execute("UPDATE sync_resources SET local_state='available' WHERE content_hash=?1",params![resource.digest.as_bytes().as_slice()]).map_err(storage)?;
                if let (ObjectKey::Song(song),SharedResourceKind::Audio) = (publication.object,resource.kind) {
                    let path = RelativeMediaPath::new(&item.local_path)?;
                    tx.execute("UPDATE songs SET relative_path=?2,normalized_relative_path=?3,blake3_hash=?4,file_size=?5,availability=CASE WHEN availability='pending_delete' THEN availability ELSE 'available' END WHERE uuid=?1 AND library_root_uuid=?6",params![song.to_string(),path.normalized(),path.identity_key(),resource.digest.to_hex(),i64::try_from(resource.length).map_err(storage)?,publication.root.to_string()]).map_err(storage)?;
                }
            }
            if let ObjectKey::Song(song) = publication.object {
                // A complete file association is not a parsed local index.
                // Clearing the scan stamp durably queues embedded artwork and
                // lyrics extraction, also for sidecar-only replacements.
                tx.execute("UPDATE songs SET file_mtime_ns=NULL WHERE uuid=?1 AND library_root_uuid=?2",params![song.to_string(),publication.root.to_string()]).map_err(storage)?;
            }
            for item in &mut publication.items { item.stage=RemoteResourceStage::Completed; }
            publication.completed=true;
            tx.execute("UPDATE sync_remote_publications SET publication_json=?2,completed=1 WHERE operation_uuid=?1",params![publication.operation.to_string(),serde_json::to_string(&publication).map_err(storage)?]).map_err(storage)?;
            tx.execute("UPDATE sync_remote_publication_claims SET released=1 WHERE operation_uuid=?1",params![publication.operation.to_string()]).map_err(storage)?;
            let operation = transition_operation(&publication, "finish")?;
            super::durability::commit_resource_projection(control, tx, publication.root, operation, Some(publication))
        })
    }

    fn incomplete(&self, root: LibraryRootId) -> Result<Vec<OperationId>, Error> {
        self.with_reader(|connection| {
            let mut statement=connection.prepare("SELECT operation_uuid FROM sync_remote_publications WHERE local_root_uuid=?1 AND completed=0 ORDER BY operation_uuid").map_err(storage)?;
            let rows=statement.query_map(params![root.to_string()],|row| row.get::<_,String>(0)).map_err(storage)?;
            rows.map(|row| row.map_err(storage)?.parse().map_err(storage)).collect()
        })
    }
}

fn transition_operation(
    publication: &RemoteResourcePublication,
    phase: &str,
) -> Result<uuid::Uuid, Error> {
    let body = serde_json::to_vec(publication).map_err(storage)?;
    Ok(super::durability::identity(
        "resource-publication-transition",
        &format!(
            "{phase}:{}:{}",
            publication.operation,
            blake3::hash(&body).to_hex()
        ),
    ))
}

impl SqliteDatabase {
    /// Repair shared tags skipped by older projectors, including songs whose
    /// local asset index already completed. File facts and local overlays stay
    /// under their existing publication and mutation guards.
    pub fn repair_published_song_projections(&self, root: LibraryRootId) -> Result<usize, Error> {
        self.writer.run(move |connection| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(storage)?;
            let root = root.to_string();
            let rows = {
                let mut statement = tx.prepare(
                    "SELECT s.uuid,v.payload_json FROM songs s
                     JOIN sync_object_versions v ON v.object_type='song' AND v.object_uuid=s.uuid
                     JOIN sync_binding b ON b.local_root_uuid=s.library_root_uuid AND b.singleton=1
                     WHERE s.library_root_uuid=?1 AND s.availability='available' AND v.is_tombstone=0
                       AND v.payload_json IS NOT NULL
                       AND NOT EXISTS(SELECT 1 FROM sync_song_aliases a WHERE a.alias_song_uuid=s.uuid)
                       AND NOT EXISTS(SELECT 1 FROM sync_mutations m WHERE m.object_type='song'
                         AND (m.object_uuid=s.uuid OR m.object_uuid IN
                           (SELECT a.alias_song_uuid FROM sync_song_aliases a WHERE a.canonical_song_uuid=s.uuid))
                         AND m.status='pending')
                       AND NOT EXISTS(SELECT 1 FROM sync_local_delete_intents d
                         WHERE d.object_type='song' AND d.object_uuid=s.uuid AND d.state='open')
                       AND EXISTS(SELECT 1 FROM sync_remote_resource_locations l
                         JOIN sync_remote_publications p ON p.operation_uuid=l.operation_uuid
                         WHERE l.local_root_uuid=s.library_root_uuid AND l.object_type='song'
                           AND l.object_uuid=s.uuid AND p.completed=1)
                       AND (s.title IS NOT json_extract(v.payload_json,'$.title')
                         OR s.artist IS NOT json_extract(v.payload_json,'$.artist')
                         OR s.album IS NOT json_extract(v.payload_json,'$.album')
                         OR s.duration_ms IS NOT CAST(json_extract(v.payload_json,'$.duration_ms') AS INTEGER)
                         OR s.added_at IS NOT CAST(json_extract(v.payload_json,'$.added_at') AS INTEGER))",
                ).map_err(storage)?;
                let rows = statement.query_map(params![root], |row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?)))
                    .map_err(storage)?.collect::<Result<Vec<_>,_>>().map_err(storage)?;
                rows
            };
            for (song,payload) in &rows {
                super::projection::repair_confirmed_song_tags(
                    &tx, song, &serde_json::from_str(payload).map_err(storage)?, &root,
                )?;
            }
            tx.commit().map_err(storage)?;
            Ok(rows.len())
        })
    }

    /// Published audio still missing its local tag/asset index. The scan stamp
    /// is committed with publication, survives restart and also repairs songs
    /// downloaded by versions which never scheduled metadata extraction. Missing
    /// rows with a current completed association are also checked: older scans
    /// omitted Echo version paths and could incorrectly mark them missing.
    pub fn pending_remote_song_metadata(&self, root: LibraryRootId) -> Result<Vec<SongId>, Error> {
        self.with_reader(|connection| {
            let mut statement = connection
                .prepare(
                    "SELECT s.uuid FROM songs s
                 WHERE s.library_root_uuid=?1
                   AND ((s.availability='available' AND s.file_mtime_ns IS NULL)
                     OR s.availability='missing')
                   AND EXISTS(SELECT 1 FROM sync_binding b WHERE b.singleton=1
                     AND b.local_root_uuid=s.library_root_uuid)
                   AND NOT EXISTS(SELECT 1 FROM sync_song_aliases a WHERE a.alias_song_uuid=s.uuid)
                   AND NOT EXISTS(SELECT 1 FROM sync_object_versions v
                     WHERE v.object_type='song' AND v.object_uuid=s.uuid AND v.is_tombstone=1)
                   AND NOT EXISTS(SELECT 1 FROM sync_local_delete_intents d
                     WHERE d.object_type='song' AND d.object_uuid=s.uuid AND d.state='open')
                   AND NOT EXISTS(SELECT 1 FROM sync_mutations m
                     WHERE m.object_type='song' AND m.object_uuid=s.uuid AND m.status='pending')
                   AND EXISTS (SELECT 1 FROM sync_remote_resource_locations l
                     JOIN sync_remote_publications p ON p.operation_uuid=l.operation_uuid
                     JOIN sync_resource_manifests m ON m.object_type=l.object_type
                       AND m.object_uuid=l.object_uuid AND m.library_relative_path=l.library_relative_path
                       AND m.content_hash=l.content_hash AND m.ref_kind='audio'
                     WHERE l.local_root_uuid=s.library_root_uuid AND l.object_type='song'
                       AND l.object_uuid=s.uuid AND lower(hex(l.content_hash))=s.blake3_hash
                       AND l.local_relative_path=s.relative_path AND p.completed=1)
                 ORDER BY s.uuid",
                )
                .map_err(storage)?;
            let rows = statement
                .query_map(params![root.to_string()], |row| row.get::<_, String>(0))
                .map_err(storage)?;
            rows.map(|row| row.map_err(storage)?.parse().map_err(storage))
                .collect()
        })
    }

    /// True only if every current declaration has a completed durable local
    /// association. The caller should still handle files removed externally.
    pub fn remote_resource_media_ready(
        &self,
        root: LibraryRootId,
        object: ObjectKey,
    ) -> Result<bool, Error> {
        self.with_reader(|connection| {
            let (kind,id)=sync_manifests::identity(object);
            let current=sync_manifests::load(connection,kind,&id)?;
            if current.is_empty() { return Ok(false); }
            for resource in current {
                let available: bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM sync_remote_resource_locations l JOIN sync_remote_publications p ON p.operation_uuid=l.operation_uuid WHERE l.local_root_uuid=?1 AND l.object_type=?2 AND l.object_uuid=?3 AND l.library_relative_path=?4 AND l.content_hash=?5 AND p.completed=1)",params![root.to_string(),kind,id,resource.relative_path.as_str(),resource.digest.as_bytes().as_slice()],|row| row.get(0)).map_err(storage)?;
                if !available { return Ok(false); }
            }
            Ok(true)
        })
    }
}

/// Resource sets awaiting local media publication, grouped by typed object.
pub type PendingRemoteResourceSets = Vec<(ObjectKey, Vec<RemoteResourceTarget>)>;

impl SqliteDatabase {
    pub fn pending_remote_resource_sets(
        &self,
        root: LibraryRootId,
    ) -> Result<PendingRemoteResourceSets, Error> {
        self.pending_resource_sets(root, false)
    }

    /// Peer-authored resources and previously published resources edited
    /// locally. A host advances existing version associations through the same
    /// journal without treating newly imported local media as remote input.
    pub fn pending_peer_resource_sets(
        &self,
        root: LibraryRootId,
    ) -> Result<PendingRemoteResourceSets, Error> {
        self.pending_resource_sets(root, true)
    }

    fn pending_resource_sets(
        &self,
        root: LibraryRootId,
        peer_authored_only: bool,
    ) -> Result<PendingRemoteResourceSets, Error> {
        self.with_reader(|connection| {
            let bound: Option<String> = connection.query_row("SELECT local_root_uuid FROM sync_binding WHERE singleton=1",[],|row| row.get(0)).optional().map_err(storage)?.flatten();
            if bound.as_deref()!=Some(root.to_string().as_str()) { return Err(Error::conflict("remote resources root is not bound")); }
            let query = if peer_authored_only {
                "SELECT DISTINCT m.object_type,m.object_uuid FROM sync_resource_manifests m
                 WHERE NOT (m.object_type='song' AND EXISTS (
                   SELECT 1 FROM sync_song_aliases a WHERE a.alias_song_uuid=m.object_uuid))
                   AND EXISTS (SELECT 1 FROM sync_object_versions v
                   JOIN sync_commits c ON c.commit_seq=v.commit_seq
                   WHERE v.object_type=m.object_type AND v.object_uuid=m.object_uuid
                     AND v.is_tombstone=0 AND (
                       c.origin_device_id <> (SELECT value FROM device_state WHERE key='device_id')
                       OR EXISTS(SELECT 1 FROM sync_remote_resource_locations l
                         JOIN sync_remote_publications p ON p.operation_uuid=l.operation_uuid
                         WHERE l.local_root_uuid=?1 AND l.object_type=m.object_type
                           AND l.object_uuid=m.object_uuid AND p.completed=1)))
                   AND NOT EXISTS(SELECT 1 FROM sync_mutations local
                     WHERE local.object_type=m.object_type AND local.object_uuid=m.object_uuid
                       AND local.status='pending')
                 ORDER BY m.object_type,m.object_uuid"
            } else {
                "SELECT DISTINCT m.object_type,m.object_uuid FROM sync_resource_manifests m
                 WHERE NOT (m.object_type='song' AND EXISTS (
                   SELECT 1 FROM sync_song_aliases a WHERE a.alias_song_uuid=m.object_uuid))
                   AND EXISTS (SELECT 1 FROM sync_object_versions v
                     WHERE v.object_type=m.object_type AND v.object_uuid=m.object_uuid
                       AND v.is_tombstone=0)
                 ORDER BY m.object_type,m.object_uuid"
            };
            let mut statement=connection.prepare(query).map_err(storage)?;
            let mut rows = if peer_authored_only {
                statement.query(params![root.to_string()])
            } else {
                statement.query([])
            }.map_err(storage)?;
            let mut pending=Vec::new();
            while let Some(row) = rows.next().map_err(storage)? {
                let kind: String = row.get(0).map_err(storage)?;
                let id: String = row.get(1).map_err(storage)?;
                let object=parse_object(&kind,&id)?;
                let resources=sync_manifests::load(connection,&kind,&id)?;
                let mut ready=true;
                let mut targets=Vec::new();
                for manifest in resources {
                    let previous: Option<(Vec<u8>,bool)>=connection.query_row("SELECT l.content_hash,p.completed FROM sync_remote_resource_locations l JOIN sync_remote_publications p ON p.operation_uuid=l.operation_uuid WHERE l.local_root_uuid=?1 AND l.object_type=?2 AND l.object_uuid=?3 AND l.library_relative_path=?4",params![root.to_string(),kind,id,manifest.relative_path.as_str()],|row| Ok((row.get(0)?,row.get(1)?))).optional().map_err(storage)?;
                    let expected_old_digest=previous.as_ref().map(|(digest,_)| digest_from_bytes(digest)).transpose()?;
                    ready &= previous.is_some_and(|(digest,complete)| complete && digest.as_slice()==manifest.digest.as_bytes().as_slice());
                    targets.push(RemoteResourceTarget { manifest,expected_old_digest });
                }
                if !ready && !targets.is_empty() { pending.push((object,targets)); }
            }
            Ok(pending)
        })
    }

    /// Resolve one exact immutable manifest to its completed local path.
    pub fn local_remote_resource_path(
        &self,
        root: LibraryRootId,
        object: ObjectKey,
        manifest: &SharedResourceManifest,
    ) -> Result<Option<RelativeMediaPath>, Error> {
        self.with_reader(|connection| {
            let (kind,id)=sync_manifests::identity(object);
            let path: Option<String>=connection.query_row("SELECT l.local_relative_path FROM sync_remote_resource_locations l JOIN sync_remote_publications p ON p.operation_uuid=l.operation_uuid WHERE l.local_root_uuid=?1 AND l.object_type=?2 AND l.object_uuid=?3 AND l.library_relative_path=?4 AND l.content_hash=?5 AND p.completed=1",params![root.to_string(),kind,id,manifest.relative_path.as_str(),manifest.digest.as_bytes().as_slice()],|row| row.get(0)).optional().map_err(storage)?;
            path.map(|path| RelativeMediaPath::new(&path)).transpose()
        })
    }
}

fn digest_from_bytes(bytes: &[u8]) -> Result<ResourceDigest, Error> {
    Ok(ResourceDigest::from_bytes(bytes.try_into().map_err(
        |_| Error::InvariantViolation {
            why: "invalid local resource digest".to_owned(),
        },
    )?))
}

fn parse_object(kind: &str, id: &str) -> Result<ObjectKey, Error> {
    match kind {
        "song" => Ok(ObjectKey::Song(id.parse().map_err(storage)?)),
        "playlist" => Ok(ObjectKey::Playlist(id.parse().map_err(storage)?)),
        "playlist-item" => Ok(ObjectKey::PlaylistItem(id.parse().map_err(storage)?)),
        "library-root" => Ok(ObjectKey::LibraryRoot(id.parse().map_err(storage)?)),
        "favorite" => Ok(ObjectKey::Favorite(id.parse().map_err(storage)?)),
        "override" => Ok(ObjectKey::Override(id.parse().map_err(storage)?)),
        "play-stats" => Ok(ObjectKey::PlayStats(id.parse().map_err(storage)?)),
        _ => Err(Error::InvariantViolation {
            why: "unknown resource object kind".to_owned(),
        }),
    }
}
