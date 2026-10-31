//! Save the removed identity and the surviving membership snapshot atomically.
use super::{
    candidate, commit_then_project, project_records, ChangeOrigin, ControlPlanePort, DeviceId,
    Error, LibraryBatch, LibraryRootId, MemoryDatabase, PlaylistId, PortableRecord,
    PortableRecordEnvelope, RecordKind, Revision, SongId,
};
use crate::domain::library::{next_hlc, TombstoneRecord};

pub(super) fn remove(
    db: &MemoryDatabase,
    control: &dyn ControlPlanePort,
    root: LibraryRootId,
    playlist: PlaylistId,
    song: SongId,
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
    let mut before = match control.read_record(root, RecordKind::Playlist, &playlist.to_string())? {
        Some(PortableRecord::Playlist(record)) => record,
        _ => return Err(Error::unavailable("playlist", "not in active library")),
    };
    let mut planned = store.clone();
    project_records(&mut planned, root, &replay);
    let Some(member) = planned.members.get(&(playlist, song)) else {
        *store = planned;
        return Ok(());
    };
    let member = member.id();
    let previous = control
        .read_record(root, RecordKind::PlaylistItem, &member.to_string())?
        .ok_or_else(|| Error::unavailable("member", "durable identity missing"))?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |time| {
            u64::try_from(time.as_millis()).unwrap_or(u64::MAX)
        });
    let record = PortableRecord::Tombstone(TombstoneRecord {
        object_uuid: member.as_uuid(),
        deleted_kind: RecordKind::PlaylistItem,
        revision: Revision::from_u64(
            previous
                .revision()
                .as_u64()
                .checked_add(1)
                .ok_or_else(|| Error::conflict("member revision exhausted"))?,
        ),
        hlc: next_hlc(previous.hlc(), now / 1000),
        updated_by_device_id: device,
    });
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
    before.revision = Revision::from_u64(
        before
            .revision
            .as_u64()
            .checked_add(1)
            .ok_or_else(|| Error::conflict("playlist revision exhausted"))?,
    );
    before.hlc = next_hlc(before.hlc, now / 1000);
    before.updated_by_device_id = device;
    let record = PortableRecord::Playlist(before);
    batch.state.extend(candidate::freeze_planned(
        &replay,
        manifest.library_id,
        crate::domain::sync::DeviceId::from_uuid(device.as_uuid()),
        &record,
        now,
        (&batch.state, &batch.records),
    )?);
    batch
        .records
        .push(PortableRecordEnvelope::new(record, None)?);
    commit_then_project(control, root, &batch, |_| {
        let replay = durable.recover(root)?;
        let mut planned = store.clone();
        project_records(&mut planned, root, &replay);
        if db.fail_commit.load(std::sync::atomic::Ordering::Relaxed) {
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
