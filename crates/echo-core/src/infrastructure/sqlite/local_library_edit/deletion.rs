//! Freeze a complete playlist cascade, including retained orphan memberships.
use super::{
    storage, Connection, DeviceId, Error, LibraryBatch, LibraryId, LibraryRootId, ObjectKey,
    PlaylistId, PortableRecord, PortableRecordEnvelope, RecordKind, Revision,
};
use crate::domain::ids::PlaylistItemId;
use crate::domain::library::durability::ChangeOrigin;
use crate::domain::library::{next_hlc, HybridLogicalClock, TombstoneRecord};
use rusqlite::params;

pub(super) fn plan(
    c: &Connection,
    root: LibraryRootId,
    library: LibraryId,
    playlist: PlaylistId,
    device: DeviceId,
    sequence: u64,
) -> Result<LibraryBatch, Error> {
    let exists: bool = c
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM playlists WHERE uuid=?1 AND library_root_uuid=?2)",
            params![playlist.to_string(), root.to_string()],
            |row| row.get(0),
        )
        .map_err(storage)?;
    if !exists {
        return Err(Error::unavailable("playlist", "not in the active library"));
    }
    let retained = super::durability_projection::read(c, root)?;
    let mut targets = std::collections::BTreeMap::new();
    let mut playlist_clock = HybridLogicalClock::default();
    for envelope in &retained.records {
        match envelope.record() {
            PortableRecord::PlaylistItem(item) if item.playlist_uuid == playlist.as_uuid() => {
                targets.insert(item.item_uuid, item.hlc);
            }
            PortableRecord::Playlist(record) if record.playlist_uuid == playlist.as_uuid() => {
                playlist_clock = record.hlc;
            }
            _ => {}
        }
    }
    let mut statement = c
        .prepare("SELECT member_uuid FROM playlist_songs WHERE playlist_uuid=?1")
        .map_err(storage)?;
    let members = statement
        .query_map([playlist.to_string()], |row| row.get::<_, String>(0))
        .map_err(storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage)?;
    for member in members {
        let id = uuid::Uuid::parse_str(&member).map_err(storage)?;
        targets
            .entry(id)
            .or_insert_with(HybridLogicalClock::default);
    }
    let mut batch = LibraryBatch::new(
        uuid::Uuid::new_v4(),
        library,
        ChangeOrigin::LocalEdit,
        vec![],
    );
    let targets = targets
        .into_iter()
        .map(|(id, clock)| {
            (
                ObjectKey::PlaylistItem(PlaylistItemId::from_uuid(id)),
                clock,
            )
        })
        .chain(std::iter::once((
            ObjectKey::Playlist(playlist),
            playlist_clock,
        )));
    for (object, clock) in targets {
        let revision = super::super::sync::durability::allocate_local_delete(c, root, object)?;
        let (kind, id) = match object {
            ObjectKey::Playlist(id) => (RecordKind::Playlist, id.as_uuid()),
            ObjectKey::PlaylistItem(id) => (RecordKind::PlaylistItem, id.as_uuid()),
            _ => return Err(Error::conflict("invalid playlist cascade target")),
        };
        let record = PortableRecord::Tombstone(TombstoneRecord {
            object_uuid: id,
            deleted_kind: kind,
            revision: Revision::from_u64(revision),
            hlc: next_hlc(
                clock,
                u64::try_from(super::super::now_ms().max(0)).unwrap_or(0) / 1000,
            ),
            updated_by_device_id: device,
        });
        batch
            .records
            .push(PortableRecordEnvelope::new(record, None)?);
        batch.state.extend(
            super::super::sync::durability::capture_local_object_mutation(
                c,
                root,
                object,
                library,
                crate::domain::sync::DeviceId::from_uuid(device.as_uuid()),
                sequence,
            )?,
        );
    }
    Ok(batch)
}
