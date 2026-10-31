//! Freeze artwork references using the same writer planning and catch-up boundary.
use super::{
    planned_batch, planned_playlist, statements, storage, ControlPlanePort, DeviceId, Error,
    LibraryRootId, ObjectKey, PlaylistId, PortableRecord, RecordKind, SqliteDatabase,
};
use crate::application::ports::PlaylistCoverEditPort;
use crate::domain::library::{durability::DurableResource, PortableAssetReference};
use rusqlite::params;

impl PlaylistCoverEditPort for SqliteDatabase {
    fn set_library_playlist_cover(
        &self,
        control: &dyn ControlPlanePort,
        root: LibraryRootId,
        id: PlaylistId,
        asset: Option<PortableAssetReference>,
        device: DeviceId,
    ) -> Result<(), Error> {
        self.plan_library_edit(
            control,
            root,
            device,
            false,
            move |c, authority, library, sequence, _| {
                let belongs: bool = c.query_row(
                "SELECT EXISTS(SELECT 1 FROM playlists WHERE uuid=?1 AND library_root_uuid=?2)",
                params![id.to_string(), root.to_string()], |row| row.get(0),
            ).map_err(storage)?;
                if !belongs {
                    return Err(Error::unavailable("playlist", "not in active library"));
                }
                let saved = authority.read_record(root, RecordKind::Playlist, &id.to_string())?;
                let mut payload = statements::playlist_full_snapshot_payload(c, id)?;
                payload["cover_asset"] = serde_json::to_value(&asset).map_err(storage)?;
                super::super::sync::durability::allocate_local_playlist_snapshot(
                    c, root, id, &payload,
                )?;
                statements::advance_object_hlc(
                    c,
                    "playlist",
                    &id.to_string(),
                    u64::try_from(super::super::now_ms().max(0)).unwrap_or(0) / 1000,
                )?;
                let PortableRecord::Playlist(mut record) =
                    planned_playlist(c, id, device, saved.as_ref())?
                else {
                    return Err(Error::conflict("playlist after-image expected"));
                };
                record.cover_asset.clone_from(&asset);
                let mut batch = planned_batch(
                    c,
                    root,
                    library,
                    device,
                    PortableRecord::Playlist(record),
                    ObjectKey::Playlist(id),
                    sequence,
                )?;
                if let Some(asset) = asset {
                    batch.resources.push(DurableResource {
                        relative_path: format!("echo/assets/{}", asset.content_hash),
                        content_hash: asset.content_hash,
                        byte_length: asset.byte_len,
                    });
                }
                Ok(batch)
            },
        )
    }
}
