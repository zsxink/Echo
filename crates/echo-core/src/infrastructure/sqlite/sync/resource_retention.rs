//! Resource hold reconstruction and durable immutable-copy receipts.

use super::super::{support::storage, SqliteDatabase};
use super::sync_manifests;
use crate::application::ports::{RemoteDeleteIsolation, ResourceRetentionPort, RetainedResource};
use crate::domain::ids::{LibraryRootId, OperationId, RelativeMediaPath};
use crate::domain::library::durability::DurableResource;
use crate::domain::sync::{ResourceDigest, SharedResourceManifest};
use crate::error::Error;
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};

type Hold = (String, String, ResourceDigest);

impl ResourceRetentionPort for SqliteDatabase {
    fn rebuild_resource_holds(&self, root: LibraryRootId) -> Result<(), Error> {
        self.writer.run(move |connection| {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate).map_err(storage)?;
            let holds = collect(&tx, root)?;
            tx.execute("DELETE FROM sync_resource_holds WHERE local_root_uuid=?1", params![root.to_string()]).map_err(storage)?;
            for (kind, owner, digest) in holds {
                tx.execute("INSERT OR IGNORE INTO sync_resource_holds(local_root_uuid,owner_kind,owner_id,content_hash) VALUES(?1,?2,?3,?4)", params![root.to_string(),kind,owner,digest.as_bytes().as_slice()]).map_err(storage)?;
            }
            tx.commit().map_err(storage)
        })
    }

    fn record_retained_resource(
        &self,
        root: LibraryRootId,
        resource: RetainedResource,
    ) -> Result<(), Error> {
        if resource.path.normalized() != format!("echo/media/retained/{}", resource.digest.to_hex())
        {
            return Err(Error::conflict("retained receipt has an invalid location"));
        }
        self.writer.run(move |connection| {
            let length = i64::try_from(resource.length).map_err(storage)?;
            let previous: Option<i64> = connection.query_row("SELECT byte_length FROM sync_retained_resources WHERE local_root_uuid=?1 AND content_hash=?2", params![root.to_string(),resource.digest.as_bytes().as_slice()], |row| row.get(0)).optional().map_err(storage)?;
            if previous.is_some_and(|old| old != length) { return Err(Error::conflict("retained receipt length differs")); }
            connection.execute("INSERT OR IGNORE INTO sync_retained_resources(local_root_uuid,content_hash,byte_length,local_relative_path) VALUES(?1,?2,?3,?4)",params![root.to_string(),resource.digest.as_bytes().as_slice(),length,resource.path.normalized()]).map_err(storage)?;
            Ok(())
        })
    }

    fn retained_resource(
        &self,
        root: LibraryRootId,
        digest: ResourceDigest,
    ) -> Result<Option<RetainedResource>, Error> {
        self.with_reader(|connection| {
            let row: Option<(i64,String)> = connection.query_row("SELECT byte_length,local_relative_path FROM sync_retained_resources WHERE local_root_uuid=?1 AND content_hash=?2",params![root.to_string(),digest.as_bytes().as_slice()],|row| Ok((row.get(0)?,row.get(1)?))).optional().map_err(storage)?;
            row.map(|(length,path)| Ok(RetainedResource { digest, length: u64::try_from(length).map_err(storage)?, path: RelativeMediaPath::new(&path)? })).transpose()
        })
    }
    fn remote_delete_isolations(
        &self,
        root: LibraryRootId,
    ) -> Result<Vec<RemoteDeleteIsolation>, Error> {
        self.with_reader(|connection| {
            let mut statement=connection.prepare("SELECT operation_uuid,source_relative_path,content_hash FROM sync_remote_delete_isolations WHERE local_root_uuid=?1 AND completed=0 ORDER BY object_version,object_uuid").map_err(storage)?;
            let rows=statement.query_map(params![root.to_string()],|row| Ok((row.get::<_,String>(0)?,row.get::<_,String>(1)?,row.get::<_,String>(2)?))).map_err(storage)?;
            rows.map(|row| { let (operation,path,hash)=row.map_err(storage)?; Ok(RemoteDeleteIsolation { operation: operation.parse().map_err(storage)?, source: RelativeMediaPath::new(&path)?, digest:parse_digest(&hash)? }) }).collect()
        })
    }
    fn finish_remote_delete_isolation(
        &self,
        root: LibraryRootId,
        operation: OperationId,
    ) -> Result<(), Error> {
        self.writer.run(move |connection| {
            connection.execute("UPDATE sync_remote_delete_isolations SET completed=1 WHERE local_root_uuid=?1 AND operation_uuid=?2",params![root.to_string(),operation.to_string()]).map_err(storage)?; Ok(())
        })
    }
    fn remote_publication_matches(
        &self,
        root: LibraryRootId,
        path: &RelativeMediaPath,
        digest: &str,
    ) -> Result<bool, Error> {
        let digest = parse_digest(digest)?;
        self.with_reader(|connection| connection.query_row("SELECT EXISTS(SELECT 1 FROM sync_remote_resource_locations l JOIN sync_remote_publications p ON p.operation_uuid=l.operation_uuid JOIN sync_resource_manifests m ON m.object_type=l.object_type AND m.object_uuid=l.object_uuid AND m.library_relative_path=l.library_relative_path AND m.content_hash=l.content_hash JOIN sync_object_versions v ON v.object_type=l.object_type AND v.object_uuid=l.object_uuid AND v.is_tombstone=0 WHERE l.local_root_uuid=?1 AND l.local_relative_path=?2 AND l.content_hash=?3 AND p.completed=1)",params![root.to_string(),path.normalized(),digest.as_bytes().as_slice()],|row| row.get(0)).map_err(storage))
    }
}

fn collect_library_resources(
    connection: &Connection,
    root: LibraryRootId,
) -> Result<Vec<Hold>, Error> {
    // Directory replay retains resource declarations even while their frozen
    // candidate, conflict or publication is intentionally not installed into a
    // live session. Those declarations must protect bytes independently of the
    // selected binding, current catalog and protocol-table lifecycle.
    let mut holds = Vec::new();
    let mut statement = connection.prepare("SELECT object_key,envelope_json FROM library_recovery_records WHERE library_root_uuid=?1 AND row_kind='resource'").map_err(storage)?;
    let rows = statement
        .query_map(params![root.to_string()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(storage)?;
    for row in rows {
        let (owner, raw) = row.map_err(storage)?;
        let resource: DurableResource = serde_json::from_str(&raw).map_err(storage)?;
        let path = RelativeMediaPath::new(&resource.relative_path)?;
        if path.normalized() != resource.relative_path
            || owner != format!("resource:{}", resource.relative_path)
        {
            return Err(Error::conflict("retained resource identity differs"));
        }
        holds.push((
            "library".into(),
            owner,
            parse_digest(&resource.content_hash)?,
        ));
    }
    Ok(holds)
}

fn collect(connection: &Connection, root: LibraryRootId) -> Result<Vec<Hold>, Error> {
    let mut holds = collect_library_resources(connection, root)?;
    // Terminal candidates remain reconciliation evidence while their rows are
    // retained. Explicit epoch recovery archives them outside the root-switch
    // archive tables; both live and archived evidence continue to own bytes.
    let mut statement = connection.prepare("SELECT mutation_id,resource_refs_json FROM sync_mutations WHERE local_root_uuid=?1 UNION ALL SELECT m.recovery_id||':'||m.mutation_id,m.resource_refs_json FROM recovery_sync_mutations m JOIN sync_recovery_archives a ON a.recovery_id=m.recovery_id WHERE a.local_root_uuid=?1").map_err(storage)?;
    let rows = statement
        .query_map(params![root.to_string()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(storage)?;
    for row in rows {
        let (owner, raw) = row.map_err(storage)?;
        let resources: Vec<SharedResourceManifest> = serde_json::from_str(&raw).map_err(storage)?;
        sync_manifests::validate(&resources)?;
        holds.extend(
            resources
                .into_iter()
                .map(|resource| ("mutation".into(), owner.clone(), resource.digest)),
        );
    }
    drop(statement);
    // Retained logs include every losing resource version. Conflicts refer to
    // those versions; neither logs nor unresolved conflicts are GC'ed here.
    let mut statement = connection.prepare("SELECT 'commit',CAST(commit_seq AS TEXT),canonical_json FROM sync_commits WHERE (SELECT local_root_uuid FROM sync_active_context WHERE singleton=1)=?1 UNION ALL SELECT 'commit',CAST(commit_seq AS TEXT),canonical_json FROM archived_sync_commits WHERE context_root=?1 UNION ALL SELECT 'recovery_commit',c.recovery_id||':'||c.commit_seq,c.canonical_json FROM recovery_sync_commits c JOIN sync_recovery_archives a ON a.recovery_id=c.recovery_id WHERE a.local_root_uuid=?1").map_err(storage)?;
    let rows = statement
        .query_map(params![root.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(storage)?;
    for row in rows {
        let (kind, owner, raw) = row.map_err(storage)?;
        for resource in sync_manifests::from_commit_json(&raw)? {
            holds.push((kind.clone(), owner.clone(), resource.digest));
        }
    }
    drop(statement);
    let mut conflicts = connection.prepare("SELECT conflict_id,replaced_version,accepted_version FROM sync_conflicts WHERE resolved_at IS NULL AND (SELECT local_root_uuid FROM sync_active_context WHERE singleton=1)=?1 UNION ALL SELECT conflict_id,replaced_version,accepted_version FROM archived_sync_conflicts WHERE resolved_at IS NULL AND context_root=?1").map_err(storage)?;
    let rows = conflicts
        .query_map(params![root.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(storage)?;
    let mut conflict_holds = Vec::new();
    for row in rows {
        let (owner, replaced, accepted) = row.map_err(storage)?;
        for (_, version, digest) in &holds {
            if version == &replaced.to_string() || version == &accepted.to_string() {
                conflict_holds.push(("conflict".into(), owner.clone(), *digest));
            }
        }
    }
    holds.extend(conflict_holds);
    drop(conflicts);
    let mut statement = connection.prepare("SELECT 'snapshot',token,content_hash FROM sync_snapshot_resource_refs WHERE (SELECT local_root_uuid FROM sync_active_context WHERE singleton=1)=?1 UNION ALL SELECT 'snapshot',token,content_hash FROM archived_sync_snapshot_resource_refs WHERE context_root=?1 UNION ALL SELECT 'snapshot',r.recovery_id||':'||r.token,r.content_hash FROM recovery_sync_snapshot_resource_refs r JOIN sync_recovery_archives a ON a.recovery_id=r.recovery_id WHERE a.local_root_uuid=?1 UNION ALL SELECT 'current',r.object_type||':'||r.object_uuid,r.content_hash FROM sync_resource_refs r WHERE EXISTS(SELECT 1 FROM songs s WHERE s.uuid=r.object_uuid AND s.library_root_uuid=?1) OR EXISTS(SELECT 1 FROM playlists p WHERE p.uuid=r.object_uuid AND p.library_root_uuid=?1)").map_err(storage)?;
    let rows = statement
        .query_map(params![root.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Vec<u8>>(2)?,
            ))
        })
        .map_err(storage)?;
    for row in rows {
        let (kind, owner, bytes) = row.map_err(storage)?;
        let digest = ResourceDigest::from_bytes(
            bytes
                .try_into()
                .map_err(|_| Error::conflict("invalid retained owner digest"))?,
        );
        holds.push((kind, owner, digest));
    }
    drop(statement);
    let mut statement = connection.prepare("SELECT j.operation_uuid,i.expected_hash FROM operation_items i JOIN operation_journal j ON j.operation_uuid=i.operation_uuid WHERE j.library_root_uuid=?1 AND i.state NOT IN ('rolled_back','database_finalized','completed')").map_err(storage)?;
    let rows = statement
        .query_map(params![root.to_string()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(storage)?;
    for row in rows {
        let (owner, hash) = row.map_err(storage)?;
        if hash.is_empty() {
            continue;
        }
        let digest = parse_digest(&hash)?;
        holds.push(("operation".into(), owner, digest));
    }
    Ok(holds)
}

fn parse_digest(value: &str) -> Result<ResourceDigest, Error> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(Error::conflict("invalid operation resource digest"));
    }
    let mut digest = [0; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).map_err(storage)?;
    }
    Ok(ResourceDigest::from_bytes(digest))
}

/// Persist before hiding the row; replay reuses the same isolation identity.
pub(super) fn plan_remote_delete(connection: &Connection, song: &str) -> Result<(), Error> {
    let row: Option<(String,String,String,i64)> = connection.query_row("SELECT s.library_root_uuid,s.relative_path,s.blake3_hash,v.object_version FROM songs s JOIN sync_object_versions v ON v.object_type='song' AND v.object_uuid=s.uuid WHERE s.uuid=?1 AND s.blake3_hash IS NOT NULL AND s.availability='available'",params![song],|row| Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).optional().map_err(storage)?;
    if let Some((root, path, hash, version)) = row {
        let key = format!("remote-delete:{root}:{song}:{version}");
        let bytes = blake3::hash(key.as_bytes());
        let mut id = [0; 16];
        id.copy_from_slice(&bytes.as_bytes()[..16]);
        let operation = OperationId::from_uuid(uuid::Uuid::from_bytes(id));
        connection.execute("INSERT OR IGNORE INTO sync_remote_delete_isolations(local_root_uuid,object_uuid,object_version,operation_uuid,source_relative_path,content_hash) VALUES(?1,?2,?3,?4,?5,?6)",params![root,song,version,operation.to_string(),path,hash]).map_err(storage)?;
    }
    Ok(())
}
