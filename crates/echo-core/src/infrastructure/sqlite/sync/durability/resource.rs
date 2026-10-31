//! Focused recovery evidence for one resource-publication transition.
use super::{parse_uuid, storage};
use crate::application::ports::RemoteResourcePublication;
use crate::domain::ids::{LibraryRootId, OperationId};
use crate::domain::library::durability::{
    DurableStateChange, LibraryReplay, StateKey, StateNamespace, StateTarget,
};
use crate::domain::library::PortableRecord;
use crate::domain::sync::{ObjectKey, SharedResourceKind};
use crate::error::Error;
use rusqlite::{params, Connection};
use serde_json::Value;

pub(super) const PAYLOAD: &str = "json_object('schema','echo.sync.resource-publication.v1','operation',operation_uuid,'publication',json(publication_json),'original_body',publication_json,'completed',json(CASE WHEN completed=1 THEN 'true' ELSE 'false' END))";

/// The publication changes physical associations, not its owner's logical value.
/// Only skip the full compatibility capture when that value is already in the
/// authenticated journal protected by the caller's current write session.
pub(super) fn owner_is_covered(
    replay: &LibraryReplay,
    publication: &RemoteResourcePublication,
) -> Result<bool, Error> {
    let owner = match publication.object {
        ObjectKey::Song(id)
        | ObjectKey::Favorite(id)
        | ObjectKey::Override(id)
        | ObjectKey::PlayStats(id) => ObjectKey::Song(id),
        ObjectKey::Playlist(id) => ObjectKey::Playlist(id),
        _ => return Ok(false),
    };
    let mut latest = None;
    for envelope in replay
        .commits
        .iter()
        .rev()
        .flat_map(|commit| commit.batch.records.iter().rev())
        .chain(
            replay
                .checkpoint
                .iter()
                .flat_map(|checkpoint| checkpoint.records.iter().rev()),
        )
    {
        if envelope.record().object_key()? == owner {
            latest = Some(envelope.record());
            break;
        }
    }
    Ok(match latest {
        Some(PortableRecord::Song(song)) => publication.items.iter().all(|item| {
            item.target.manifest.kind != SharedResourceKind::Audio
                || (song.content_hash == item.target.manifest.digest.to_hex()
                    && song.media_path == item.target.manifest.relative_path)
        }),
        Some(PortableRecord::Playlist(_)) => true,
        // A tombstone is evidence that the owner is absent, not coverage for a
        // newly planned association. The ordinary capture keeps its old checks.
        _ => false,
    })
}

pub(super) fn capture(
    connection: &Connection,
    root: LibraryRootId,
    operation: OperationId,
) -> Result<DurableStateChange, Error> {
    let raw: String = connection
        .query_row(
            &format!("SELECT {PAYLOAD} FROM sync_remote_publications WHERE local_root_uuid=?1 AND operation_uuid=?2"),
            params![root.to_string(), operation.to_string()],
            |row| row.get(0),
        )
        .map_err(storage)?;
    from_payload(root, serde_json::from_str(&raw).map_err(storage)?)
}

pub(super) fn from_payload(
    root: LibraryRootId,
    mut payload: Value,
) -> Result<DurableStateChange, Error> {
    // The historical table did not capture a binding. Keep the exact operation
    // and source root without inventing authority from the current session.
    let key = StateKey {
        namespace: StateNamespace::LegacyEvidence { source_root: root },
        target: StateTarget::Publication {
            operation_id: parse_uuid(&payload, "operation")?,
        },
    };
    payload["schema"] = Value::from("echo.sync.resource-publication.v2");
    if let Ok(complete) = DurableStateChange::scoped(key.clone(), payload.clone()) {
        return Ok(complete);
    }
    // Existing incomplete/old declarations remain unproven original evidence.
    payload["schema"] = Value::from("echo.sync.resource-publication.v1");
    payload
        .as_object_mut()
        .ok_or_else(|| Error::conflict("invalid publication evidence"))?
        .remove("original_body");
    DurableStateChange::scoped(key, payload)
}
