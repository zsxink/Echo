//! The fake saves the same complete cascade and frozen allocation chain.
use super::{
    candidate, commit_then_project, project_records, ControlPlanePort, DeviceId, Error,
    LibraryBatch, LibraryRootId, MemoryDatabase, PlaylistId, PortableRecord,
    PortableRecordEnvelope, RecordKind, Revision,
};
use crate::domain::library::durability::ChangeOrigin;
use crate::domain::library::{next_hlc, TombstoneRecord};

pub(super) fn delete(
    db: &MemoryDatabase,
    control: &dyn ControlPlanePort,
    root: LibraryRootId,
    id: PlaylistId,
    device: DeviceId,
) -> Result<(), Error> {
    let mut store = db.lock();
    if device != store.device_id {
        return Err(Error::conflict(
            "library edit author is not the local device",
        ));
    }
    let durable = control
        .durability()
        .ok_or_else(|| Error::unavailable("library edits", "directory commit adapter missing"))?;
    let replay = durable.recover(root)?;
    let manifest = control
        .read_manifest(root)?
        .ok_or_else(|| Error::unavailable("library edits", "library manifest missing"))?;
    let playlist = control
        .read_record(root, RecordKind::Playlist, &id.to_string())?
        .ok_or_else(|| Error::unavailable("playlist", "not in the active library"))?;
    let mut targets = std::collections::BTreeMap::new();
    for envelope in replay
        .checkpoint
        .iter()
        .flat_map(|image| &image.records)
        .chain(
            replay
                .commits
                .iter()
                .flat_map(|commit| &commit.batch.records),
        )
    {
        match envelope.record() {
            PortableRecord::PlaylistItem(item) if item.playlist_uuid == id.as_uuid() => {
                targets.insert(item.item_uuid, envelope.record().clone());
            }
            PortableRecord::Tombstone(t) if t.deleted_kind == RecordKind::PlaylistItem => {
                targets.remove(&t.object_uuid);
            }
            _ => {}
        }
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |value| {
            u64::try_from(value.as_millis()).unwrap_or(u64::MAX)
        });
    let mut batch = LibraryBatch::new(
        uuid::Uuid::new_v4(),
        manifest.library_id,
        ChangeOrigin::LocalEdit,
        vec![],
    );
    for before in targets.into_values().chain(std::iter::once(playlist)) {
        let record = PortableRecord::Tombstone(TombstoneRecord {
            object_uuid: before.object_uuid(),
            deleted_kind: before.kind(),
            revision: Revision::from_u64(
                before
                    .revision()
                    .as_u64()
                    .checked_add(1)
                    .ok_or_else(|| Error::conflict("revision exhausted"))?,
            ),
            hlc: next_hlc(before.hlc(), now / 1000),
            updated_by_device_id: device,
        });
        let state = candidate::freeze_after(
            &replay,
            manifest.library_id,
            crate::domain::sync::DeviceId::from_uuid(device.as_uuid()),
            &record,
            now,
            &batch.state,
        )?;
        batch.state.extend(state);
        batch
            .records
            .push(PortableRecordEnvelope::new(record, None)?);
    }
    commit_then_project(control, root, &batch, |_| {
        let replay = durable.recover(root)?;
        let mut candidate = store.clone();
        project_records(&mut candidate, root, &replay);
        if db.fail_commit.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(Error::unavailable(
                "test transaction",
                "simulated commit failure",
            ));
        }
        *store = candidate;
        Ok(())
    })?;
    Ok(())
}
