//! Directory-first membership planning for the in-memory test adapter.
use super::{
    candidate, commit_then_project, project_records, ChangeOrigin, ControlPlanePort, DeviceId,
    Error, HybridLogicalClock, LibraryBatch, LibraryRootId, MemoryDatabase, PlaylistId,
    PortableRecord, PortableRecordEnvelope, RecordKind, Revision, SongId,
};
use crate::domain::ids::PlaylistItemId;
use crate::domain::library::{next_hlc, PlaylistItemRecord};

pub(super) fn append(
    db: &MemoryDatabase,
    control: &dyn ControlPlanePort,
    root: LibraryRootId,
    song: SongId,
    targets: &[PlaylistId],
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
    let mut planned = store.clone();
    project_records(&mut planned, root, &replay);
    let current = planned
        .songs
        .get(&song)
        .ok_or_else(|| Error::unavailable("song", "not in library"))?;
    if current.root() != root || !current.availability().is_playable() {
        return Err(Error::unavailable("song", "not playable in active library"));
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |time| {
            u64::try_from(time.as_millis()).unwrap_or(u64::MAX)
        });
    let mut batch = LibraryBatch::new(
        uuid::Uuid::new_v4(),
        manifest.library_id,
        ChangeOrigin::LocalEdit,
        vec![],
    );
    let mut targets = targets.to_vec();
    targets.sort_unstable();
    targets.dedup();
    for playlist in targets {
        let mut before =
            match control.read_record(root, RecordKind::Playlist, &playlist.to_string())? {
                Some(PortableRecord::Playlist(record)) => record,
                _ => return Err(Error::unavailable("playlist", "not in active library")),
            };
        if planned.members.contains_key(&(playlist, song)) {
            continue;
        }
        let position = planned
            .members
            .values()
            .filter(|member| member.playlist() == playlist)
            .map(crate::domain::entities::PlaylistMember::position)
            .max()
            .map_or(Ok(0), |position| {
                position
                    .checked_add(1)
                    .ok_or_else(|| Error::conflict("playlist position exhausted"))
            })?;
        let member = PortableRecord::PlaylistItem(PlaylistItemRecord {
            item_uuid: PlaylistItemId::new().as_uuid(),
            playlist_uuid: playlist.as_uuid(),
            song_uuid: song.as_uuid(),
            position,
            added_at: Some(now),
            revision: Revision::from_u64(1),
            hlc: next_hlc(HybridLogicalClock::default(), now / 1000),
            updated_by_device_id: device,
        });
        batch
            .records
            .push(PortableRecordEnvelope::new(member, None)?);
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
        let state = candidate::freeze_planned(
            &replay,
            manifest.library_id,
            crate::domain::sync::DeviceId::from_uuid(device.as_uuid()),
            &record,
            now,
            (&batch.state, &batch.records),
        )?;
        batch.state.extend(state);
        batch
            .records
            .push(PortableRecordEnvelope::new(record, None)?);
    }
    if batch.records.is_empty() {
        *store = planned;
        return Ok(());
    }
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
