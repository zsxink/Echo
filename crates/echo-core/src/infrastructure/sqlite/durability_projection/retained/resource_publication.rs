//! Recover current physical locations without restoring network trust or media readiness.
use super::{Reason, dependencies, storage};
use crate::application::ports::RemoteResourcePublication;
use crate::domain::{
    ids::{LibraryRootId, RelativeMediaPath},
    library::{
        LibraryId, PortableRecord,
        durability::{DurableStateChange, LibraryRecoveryRecords, StateNamespace, StateTarget},
    },
    sync::{ObjectKey, SharedResourceKind, SharedResourceManifest},
};
use crate::error::Error;
use rusqlite::{Connection, OptionalExtension, params};

pub(super) fn project(
    c: &Connection,
    root: LibraryRootId,
    library: Option<LibraryId>,
    state: &DurableStateChange,
    retained: &LibraryRecoveryRecords,
) -> Result<Option<Reason>, Error> {
    let Some(key) = &state.key else {
        return Ok(Some(Reason::RequiresStateRecovery));
    };
    let StateNamespace::Library { library_id } = key.namespace else {
        // Completed operation receipts remain immutable history. A verified
        // current pointer for this owner supersedes their association role.
        if state.payload["schema"] == "echo.sync.resource-publication.v2"
            && state.payload["completed"] == true
        {
            state.validate()?;
            let object = &state.payload["publication"]["object"];
            for current in &retained.state {
                if current.payload["schema"] == "echo.library.resource-location.v1"
                    && &current.payload["publication"]["object"] == object
                    && matches!(current.key.as_ref().map(|key| &key.namespace), Some(StateNamespace::Library { library_id }) if Some(*library_id)==library)
                {
                    current.validate()?;
                    let current: RemoteResourcePublication =
                        serde_json::from_value(current.payload["publication"].clone())
                            .map_err(storage)?;
                    if complete_resources(&current, retained) {
                        return Ok(None);
                    }
                }
            }
        }
        return Ok(Some(Reason::RequiresStateRecovery));
    };
    if Some(library_id) != library || state.payload["schema"] != "echo.library.resource-location.v1"
    {
        return Ok(Some(Reason::InvalidStateFact));
    }
    state.validate()?;
    let StateTarget::ResourceLocation { object } = key.target else {
        return Ok(Some(Reason::InvalidStateFact));
    };
    let mut publication: RemoteResourcePublication =
        serde_json::from_value(state.payload["publication"].clone()).map_err(storage)?;
    let dependency = match object {
        ObjectKey::Playlist(id) => {
            dependencies::playlist(c, root, id.as_uuid(), &retained.records)?
        }
        ObjectKey::Song(id)
        | ObjectKey::Favorite(id)
        | ObjectKey::Override(id)
        | ObjectKey::PlayStats(id) => dependencies::song(c, root, id.as_uuid(), &retained.records)?,
        _ => Some(Reason::RequiresStateRecovery),
    };
    if dependency.is_some() {
        return Ok(dependency);
    }
    if !complete_resources(&publication, retained) {
        return Ok(Some(Reason::MissingSyncDependency));
    }
    for item in &publication.items {
        let manifest = &item.target.manifest;
        if let ObjectKey::Song(id) = object {
            let path = RelativeMediaPath::new(&item.local_path)?;
            let collision:bool = c.query_row("SELECT EXISTS(SELECT 1 FROM songs WHERE library_root_uuid=?1 AND normalized_relative_path=?2 AND uuid<>?3)",params![root.to_string(),path.identity_key(),id.to_string()],|r|r.get(0)).map_err(storage)?;
            if collision {
                return Ok(Some(Reason::IdentityConflict));
            }
            if manifest.kind == SharedResourceKind::Audio {
                let hash: Option<String> = c
                    .query_row(
                        "SELECT blake3_hash FROM songs WHERE uuid=?1 AND library_root_uuid=?2",
                        params![id.to_string(), root.to_string()],
                        |r| r.get(0),
                    )
                    .map_err(storage)?;
                if hash.as_deref() != Some(manifest.digest.to_hex().as_str()) {
                    return Ok(Some(Reason::MissingSyncDependency));
                }
            }
        }
    }
    // A relocated library has a new local root identity; the immutable source
    // payload stays retained while the disposable location index uses this root.
    publication.root = root;
    let (kind, id) = crate::infrastructure::sqlite::sync_manifests::identity(object);
    let operation = publication.operation.to_string();
    let old:Option<(String,String,String)> = c.query_row("SELECT local_root_uuid,object_type,object_uuid FROM sync_remote_publications WHERE operation_uuid=?1",[&operation],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(storage)?;
    if old.is_some_and(|old| old != (root.to_string(), kind.to_owned(), id.clone())) {
        return Ok(Some(Reason::IdentityConflict));
    }
    let targets: Vec<_> = publication.items.iter().map(|item| &item.target).collect();
    c.execute("INSERT INTO sync_remote_publications(operation_uuid,local_root_uuid,object_type,object_uuid,declaration_json,publication_json,completed) VALUES(?1,?2,?3,?4,?5,?6,1) ON CONFLICT(operation_uuid) DO UPDATE SET publication_json=excluded.publication_json,completed=1",params![operation,root.to_string(),kind,id,serde_json::to_string(&targets).map_err(storage)?,serde_json::to_string(&publication).map_err(storage)?]).map_err(storage)?;
    c.execute("DELETE FROM sync_remote_resource_locations WHERE local_root_uuid=?1 AND object_type=?2 AND object_uuid=?3",params![root.to_string(),kind,id]).map_err(storage)?;
    for item in &publication.items {
        let resource = &item.target.manifest;
        c.execute("INSERT INTO sync_remote_resource_locations(local_root_uuid,object_type,object_uuid,content_hash,library_relative_path,local_relative_path,operation_uuid) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![root.to_string(),kind,id,resource.digest.as_bytes().as_slice(),resource.relative_path.as_str(),item.local_path,operation]).map_err(storage)?;
        if let (ObjectKey::Song(song), SharedResourceKind::Audio) = (object, resource.kind) {
            // This pointer only says where an immutable published version was
            // placed. A later committed after-image (a same-content move, for
            // example) owns the song's current location, so it must not be
            // undone by an older association whose logical path no longer
            // describes the retained record.
            let cur = location_is_current(retained, song.as_uuid(), resource);
            if cur {
                let path = RelativeMediaPath::new(&item.local_path)?;
                c.execute("UPDATE songs SET relative_path=?2,normalized_relative_path=?3,file_size=?4 WHERE uuid=?1 AND library_root_uuid=?5",params![song.to_string(),path.normalized(),path.identity_key(),resource.length,root.to_string()]).map_err(storage)?;
            }
        }
    }
    Ok(None)
}

/// Whether a retained location still describes the song's committed value.
///
/// The pointer is evidence about one published version's physical placement,
/// not about the song's current logical identity. Once a later after-image
/// commits a different logical path (a same-content move, for example), the
/// older association no longer describes where that value lives and must not
/// overwrite the newer committed fact.
fn location_is_current(
    retained: &LibraryRecoveryRecords,
    song: uuid::Uuid,
    resource: &SharedResourceManifest,
) -> bool {
    retained.records.iter().any(|envelope| {
        matches!(envelope.record(), PortableRecord::Song(record)
            if record.song_uuid == song
                && record.media_path.as_str() == resource.relative_path.as_str()
                && record.content_hash == resource.digest.to_hex())
    })
}

fn complete_resources(
    publication: &RemoteResourcePublication,
    retained: &LibraryRecoveryRecords,
) -> bool {
    publication.items.iter().all(|item| {
        retained.resources.iter().any(|resource| {
            resource.relative_path == item.local_path
                && resource.content_hash == item.target.manifest.digest.to_hex()
                && resource.byte_length == item.target.manifest.length
        })
    })
}
