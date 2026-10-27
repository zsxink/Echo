//! Durable immutable declarations shared by local queue, ledger and replica apply.

#![allow(clippy::missing_errors_doc, clippy::redundant_pub_crate)]

use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

use crate::domain::sync::{ObjectKey, ResourceDigest, SharedResourceKind, SharedResourceManifest};
use crate::error::{Error, Subject};

use super::super::SqliteDatabase;
use super::super::support::storage;

/// Canonical resource wire shape, kept transport independent inside Core.
#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct WireResource {
    kind: SharedResourceKind,
    content_hash: String,
    length: u64,
    media_type: String,
    relative_path: String,
}

impl From<&SharedResourceManifest> for WireResource {
    fn from(value: &SharedResourceManifest) -> Self {
        Self {
            kind: value.kind,
            content_hash: value.digest.to_hex(),
            length: value.length,
            media_type: value.media_type.clone(),
            relative_path: value.relative_path.as_str().to_owned(),
        }
    }
}

impl WireResource {
    pub(crate) fn into_manifest(self) -> Result<SharedResourceManifest, Error> {
        if self.content_hash.len() != 64
            || !self
                .content_hash
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(Error::validation(
                Subject::Other,
                "resource digest",
                "invalid lowercase BLAKE3 digest",
            ));
        }
        let mut digest = [0; 32];
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

pub(crate) fn identity(object: ObjectKey) -> (&'static str, String) {
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

pub(crate) fn validate(resources: &[SharedResourceManifest]) -> Result<(), Error> {
    if resources.len() > 32 {
        return Err(Error::validation(
            Subject::Other,
            "resource declarations",
            "at most 32 resources are allowed per object",
        ));
    }
    let wire = resources.iter().map(WireResource::from).collect::<Vec<_>>();
    if serde_json::to_vec(&wire).map_err(storage)?.len() > 32 * 1024 {
        return Err(Error::validation(
            Subject::Other,
            "resource declarations",
            "encoded resource declarations exceed 32 KiB",
        ));
    }
    let mut paths = std::collections::HashSet::new();
    for resource in resources {
        SharedResourceManifest::new(
            resource.kind,
            resource.digest,
            resource.length,
            resource.media_type.clone(),
            resource.relative_path.as_str(),
        )?;
        i64::try_from(resource.length).map_err(storage)?;
        if !paths.insert(resource.relative_path.as_str()) {
            return Err(Error::conflict(
                "resource declarations contain duplicate destination paths",
            ));
        }
    }
    Ok(())
}

impl SqliteDatabase {
    /// Replace the local object's resource declarations before its next write.
    /// The caller must hash and validate the files through the library file port.
    /// Queue entries already persisted retain their original declarations.
    pub fn set_local_sync_resources(
        &self,
        object: ObjectKey,
        resources: Vec<SharedResourceManifest>,
    ) -> Result<(), Error> {
        validate(&resources)?;
        self.writer.run(move |connection| {
            let tx = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(storage)?;
            let (kind, id) = identity(object);
            replace(&tx, kind, &id, &resources)?;
            tx.commit().map_err(storage)
        })
    }
}

pub(crate) fn load(
    connection: &Connection,
    kind: &str,
    id: &str,
) -> Result<Vec<SharedResourceManifest>, Error> {
    let mut statement = connection.prepare("SELECT ref_kind,content_hash,byte_length,media_type,library_relative_path FROM sync_resource_manifests WHERE object_type=?1 AND object_uuid=?2 ORDER BY library_relative_path,ref_kind,content_hash").map_err(storage)?;
    let rows = statement
        .query_map(params![kind, id], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Vec<u8>>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
            ))
        })
        .map_err(storage)?;
    rows.map(|row| {
        let (kind, digest, length, media_type, path) = row.map_err(storage)?;
        let kind = match kind.as_str() {
            "audio" => SharedResourceKind::Audio,
            "lyrics_sidecar" => SharedResourceKind::LyricsSidecar,
            "user_cover" => SharedResourceKind::UserCover,
            "playlist_cover" => SharedResourceKind::PlaylistCover,
            _ => {
                return Err(Error::InvariantViolation {
                    why: "invalid resource kind in resource manifest".to_owned(),
                });
            }
        };
        SharedResourceManifest::new(
            kind,
            ResourceDigest::from_bytes(digest.try_into().map_err(|_| {
                Error::InvariantViolation {
                    why: "invalid resource digest length".to_owned(),
                }
            })?),
            u64::try_from(length).map_err(storage)?,
            media_type,
            &path,
        )
    })
    .collect()
}

/// Only explicitly associated current song resources may enter the scan from
/// Echo's control tree. The location is durable even when an external editor
/// has changed its bytes since publication.
pub(crate) fn published_file_paths(
    connection: &Connection,
    root: crate::domain::ids::LibraryRootId,
) -> Result<
    Vec<(
        crate::domain::ids::RelativeMediaPath,
        crate::domain::ids::RelativeMediaPath,
    )>,
    Error,
> {
    let mut statement = connection.prepare(
        "SELECT l.local_relative_path,l.library_relative_path
         FROM sync_remote_resource_locations l
         JOIN sync_remote_publications p ON p.operation_uuid=l.operation_uuid
         JOIN songs s ON s.uuid=l.object_uuid AND s.library_root_uuid=l.local_root_uuid
         JOIN json_each(p.publication_json,'$.items') item
           ON json_extract(item.value,'$.local_path')=l.local_relative_path
           AND json_extract(item.value,'$.target.manifest.relative_path.normalized')=l.library_relative_path
         WHERE l.local_root_uuid=?1 AND l.object_type='song' AND p.completed=1
           AND json_extract(item.value,'$.target.manifest.kind')='audio'
           AND l.local_relative_path=s.relative_path
         ORDER BY l.local_relative_path",
    ).map_err(storage)?;
    let rows = statement
        .query_map(params![root.to_string()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(storage)?;
    rows.map(|row| {
        let (local, logical) = row.map_err(storage)?;
        Ok((
            crate::domain::ids::RelativeMediaPath::new(&local)?,
            crate::domain::ids::RelativeMediaPath::new(&logical)?,
        ))
    })
    .collect()
}

pub(crate) fn published_logical_path(
    connection: &Connection,
    root: crate::domain::ids::LibraryRootId,
    local: &crate::domain::ids::RelativeMediaPath,
) -> Result<Option<crate::domain::ids::RelativeMediaPath>, Error> {
    let logical: Option<String> = connection.query_row(
        "SELECT l.library_relative_path FROM songs s
         JOIN sync_remote_resource_locations l ON l.local_root_uuid=s.library_root_uuid
           AND l.object_type='song' AND l.object_uuid=s.uuid AND l.local_relative_path=s.relative_path
         JOIN sync_remote_publications p ON p.operation_uuid=l.operation_uuid
         WHERE s.library_root_uuid=?1 AND s.normalized_relative_path=?2 AND p.completed=1",
        params![root.to_string(),local.identity_key()], |row| row.get(0),
    ).optional().map_err(storage)?;
    logical
        .map(|path| crate::domain::ids::RelativeMediaPath::new(&path))
        .transpose()
}

/// The file declarations and the metadata snapshot are committed together.
/// Existing user-authored covers belong to their own editing flow.
pub(crate) fn replace_song_files(
    connection: &Connection,
    song: &crate::domain::entities::Song,
    resources: &[SharedResourceManifest],
) -> Result<(), Error> {
    validate(resources)?;
    let audio = resources
        .iter()
        .filter(|resource| resource.kind == SharedResourceKind::Audio)
        .collect::<Vec<_>>();
    let mapped_path: Option<String> = connection
        .query_row(
            "SELECT l.library_relative_path FROM sync_remote_resource_locations l
         JOIN sync_remote_publications p ON p.operation_uuid=l.operation_uuid
         WHERE l.local_root_uuid=?1 AND l.object_type='song' AND l.object_uuid=?2
           AND l.local_relative_path=?3 AND p.completed=1",
            params![
                song.root().to_string(),
                song.id().to_string(),
                song.path().normalized()
            ],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage)?;
    if audio.len() != 1
        || audio[0].relative_path.as_str()
            != mapped_path
                .as_deref()
                .unwrap_or_else(|| song.path().normalized())
        || Some(audio[0].digest.to_hex().as_str()) != song.blake3_hash()
        || Some(audio[0].length) != song.file_size()
        || resources.iter().any(|resource| {
            !matches!(
                resource.kind,
                SharedResourceKind::Audio | SharedResourceKind::LyricsSidecar
            )
        })
    {
        return Err(Error::conflict(
            "verified song resources differ from the song scan facts",
        ));
    }
    let id = song.id().to_string();
    let mut manifests = load(connection, "song", &id)?;
    manifests.retain(|resource| {
        !matches!(
            resource.kind,
            SharedResourceKind::Audio | SharedResourceKind::LyricsSidecar
        )
    });
    manifests.extend_from_slice(resources);
    replace(connection, "song", &id, &manifests)
}

pub(crate) fn replace(
    connection: &Connection,
    kind: &str,
    id: &str,
    resources: &[SharedResourceManifest],
) -> Result<(), Error> {
    validate(resources)?;
    connection
        .execute(
            "DELETE FROM sync_resource_refs WHERE object_type=?1 AND object_uuid=?2",
            params![kind, id],
        )
        .map_err(storage)?;
    connection
        .execute(
            "DELETE FROM sync_resource_manifests WHERE object_type=?1 AND object_uuid=?2",
            params![kind, id],
        )
        .map_err(storage)?;
    for resource in resources {
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
                    "conflicting metadata for one resource digest",
                ));
            }
        } else {
            connection.execute("INSERT INTO sync_resources(content_hash,resource_kind,byte_length,media_type,library_relative_path,local_state) VALUES(?1,?2,?3,?4,?5,'missing')", params![digest.as_slice(), resource_kind, i64::try_from(resource.length).map_err(storage)?, resource.media_type, resource.relative_path.as_str()]).map_err(storage)?;
        }
        connection.execute("INSERT OR IGNORE INTO sync_resource_refs(object_type,object_uuid,content_hash,ref_kind) VALUES(?1,?2,?3,?4)", params![kind,id,digest.as_slice(),resource_kind]).map_err(storage)?;
        connection.execute("INSERT INTO sync_resource_manifests(object_type,object_uuid,content_hash,ref_kind,library_relative_path,byte_length,media_type) VALUES(?1,?2,?3,?4,?5,?6,?7)", params![kind,id,digest.as_slice(),resource_kind,resource.relative_path.as_str(),i64::try_from(resource.length).map_err(storage)?,resource.media_type]).map_err(storage)?;
    }
    Ok(())
}

pub(crate) fn from_commit_json(canonical: &str) -> Result<Vec<SharedResourceManifest>, Error> {
    #[derive(Deserialize)]
    struct Resources {
        #[serde(default)]
        resource_refs: Vec<WireResource>,
    }
    let refs: Resources = serde_json::from_str(canonical).map_err(storage)?;
    let resources = refs
        .resource_refs
        .into_iter()
        .map(WireResource::into_manifest)
        .collect::<Result<Vec<_>, _>>()?;
    validate(&resources)?;
    Ok(resources)
}
