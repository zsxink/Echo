//! Original artwork references use the fake's serialized directory-first edit path.
use super::{
    candidate, commit_then_project, project_records, ChangeOrigin, ControlPlanePort, DeviceId,
    Error, LibraryBatch, LibraryRootId, MemoryDatabase, PlaylistId, PortableRecord,
    PortableRecordEnvelope, RecordKind, Revision,
};
use crate::application::ports::PlaylistCoverEditPort;
use crate::domain::library::{durability::DurableResource, next_hlc, PortableAssetReference};

impl PlaylistCoverEditPort for MemoryDatabase {
    fn set_library_playlist_cover(
        &self,
        control: &dyn ControlPlanePort,
        root: LibraryRootId,
        id: PlaylistId,
        asset: Option<PortableAssetReference>,
        device: DeviceId,
    ) -> Result<(), Error> {
        let mut store = self.lock();
        if device != store.device_id {
            return Err(Error::conflict(
                "library edit author is not the local device",
            ));
        }
        let durable = control
            .durability()
            .ok_or_else(|| Error::unavailable("cover edit", "directory adapter missing"))?;
        let replay = durable.recover(root)?;
        let manifest = control
            .read_manifest(root)?
            .ok_or_else(|| Error::unavailable("cover edit", "library manifest missing"))?;
        let Some(PortableRecord::Playlist(mut record)) =
            control.read_record(root, RecordKind::Playlist, &id.to_string())?
        else {
            return Err(Error::unavailable("playlist", "not in active library"));
        };
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |time| {
                u64::try_from(time.as_millis()).unwrap_or(u64::MAX)
            });
        record.cover_asset.clone_from(&asset);
        record.revision = Revision::from_u64(
            record
                .revision
                .as_u64()
                .checked_add(1)
                .ok_or_else(|| Error::conflict("playlist revision exhausted"))?,
        );
        record.hlc = next_hlc(record.hlc, now / 1000);
        record.updated_by_device_id = device;
        let record = PortableRecord::Playlist(record);
        let mut batch = LibraryBatch::new(
            uuid::Uuid::new_v4(),
            manifest.library_id,
            ChangeOrigin::LocalEdit,
            vec![],
        );
        batch.state = candidate::freeze(
            &replay,
            manifest.library_id,
            crate::domain::sync::DeviceId::from_uuid(device.as_uuid()),
            &record,
            now,
        )?;
        batch
            .records
            .push(PortableRecordEnvelope::new(record, None)?);
        if let Some(asset) = asset {
            batch.resources.push(DurableResource {
                relative_path: format!("echo/assets/{}", asset.content_hash),
                content_hash: asset.content_hash,
                byte_length: asset.byte_len,
            });
        }
        commit_then_project(control, root, &batch, |_| {
            let replay = durable.recover(root)?;
            let mut planned = store.clone();
            project_records(&mut planned, root, &replay);
            if self.fail_commit.load(std::sync::atomic::Ordering::Relaxed) {
                return Err(Error::unavailable(
                    "test transaction",
                    "simulated commit failure",
                ));
            }
            *store = planned;
            Ok(())
        })?;
        Ok(())
    }
}
