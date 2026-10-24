//! In-memory directory-first playlist capability for application tests.
mod candidate;
mod cover;
mod deletion;
mod members;
mod playback;
mod removal;
#[cfg(test)]
mod tests;
use super::*;
use crate::application::library_commit::commit_then_project;
use crate::domain::library::durability::{ChangeOrigin, LibraryBatch};
use crate::domain::library::{PlaylistRecord, PortableRecord, PortableRecordEnvelope, RecordKind};

impl LocalLibraryEditPort for MemoryDatabase {
    fn remove_library_member(
        &self,
        control: &dyn ControlPlanePort,
        root: LibraryRootId,
        playlist: PlaylistId,
        song: SongId,
        device: DeviceId,
    ) -> Result<(), Error> {
        removal::remove(self, control, root, playlist, song, device)
    }

    fn append_library_members(
        &self,
        control: &dyn ControlPlanePort,
        root: LibraryRootId,
        song: SongId,
        targets: &[PlaylistId],
        device: DeviceId,
    ) -> Result<(), Error> {
        members::append(self, control, root, song, targets, device)
    }

    fn delete_library_playlist(
        &self,
        control: &dyn ControlPlanePort,
        root: LibraryRootId,
        id: PlaylistId,
        device: DeviceId,
    ) -> Result<(), Error> {
        deletion::delete(self, control, root, id, device)
    }

    fn set_library_favorite(
        &self,
        control: &dyn ControlPlanePort,
        root: LibraryRootId,
        song: SongId,
        favorite: bool,
        device: DeviceId,
    ) -> Result<(), Error> {
        let mut store = self.lock();
        if device != store.device_id {
            return Err(Error::conflict(
                "library edit author is not the local device",
            ));
        }
        let current = store
            .songs
            .get(&song)
            .ok_or_else(|| Error::unavailable("song", "not in the library"))?;
        if current.root() != root || !current.availability().is_visible() {
            return Err(Error::unavailable(
                "song",
                "not visible in the active library",
            ));
        }
        let durable = control.durability().ok_or_else(|| {
            Error::unavailable("library edits", "directory commit adapter missing")
        })?;
        let baseline = durable.recover(root)?;
        let manifest = control
            .read_manifest(root)?
            .ok_or_else(|| Error::unavailable("library edits", "library manifest missing"))?;
        let before = control.read_record(root, RecordKind::Favorite, &song.to_string())?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |value| {
                u64::try_from(value.as_millis()).unwrap_or(u64::MAX)
            });
        let revision = before
            .as_ref()
            .map_or(0, |record| record.revision().as_u64())
            .checked_add(1)
            .ok_or_else(|| Error::conflict("favorite revision exhausted"))?;
        let record = PortableRecord::Favorite(crate::domain::library::FavoriteRecord {
            song_uuid: song.as_uuid(),
            is_favorite: favorite,
            favorited_at: if favorite {
                match &before {
                    Some(PortableRecord::Favorite(record)) if record.is_favorite => {
                        record.favorited_at
                    }
                    _ => Some(now),
                }
            } else {
                None
            },
            revision: Revision::from_u64(revision),
            updated_by_device_id: device,
            hlc: crate::domain::library::next_hlc(
                before
                    .as_ref()
                    .map_or_else(HybridLogicalClock::default, PortableRecord::hlc),
                now / 1000,
            ),
        });
        let mut batch = LibraryBatch::new(
            uuid::Uuid::new_v4(),
            manifest.library_id,
            ChangeOrigin::LocalEdit,
            vec![PortableRecordEnvelope::new(record.clone(), None)?],
        );
        batch.state = candidate::freeze(
            &baseline,
            manifest.library_id,
            crate::domain::sync::DeviceId::from_uuid(device.as_uuid()),
            &record,
            now,
        )?;
        commit_then_project(control, root, &batch, |_| {
            let replay = durable.recover(root)?;
            let mut candidate = store.clone();
            project_records(&mut candidate, root, &replay);
            if self.fail_commit.load(std::sync::atomic::Ordering::Relaxed) {
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

    fn create_library_playlist(
        &self,
        control: &dyn ControlPlanePort,
        root: LibraryRootId,
        id: PlaylistId,
        name: &str,
        device: DeviceId,
    ) -> Result<(), Error> {
        self.edit_library_playlist(control, root, id, name, device, false)
    }

    fn rename_library_playlist(
        &self,
        control: &dyn ControlPlanePort,
        root: LibraryRootId,
        id: PlaylistId,
        name: &str,
        device: DeviceId,
    ) -> Result<(), Error> {
        self.edit_library_playlist(control, root, id, name, device, true)
    }
}

impl MemoryDatabase {
    fn edit_library_playlist(
        &self,
        control: &dyn ControlPlanePort,
        root: LibraryRootId,
        id: PlaylistId,
        name: &str,
        device: DeviceId,
        rename: bool,
    ) -> Result<(), Error> {
        let mut store = self.lock();
        if device != store.device_id {
            return Err(Error::conflict(
                "library edit author is not the local device",
            ));
        }
        let durability = control.durability().ok_or_else(|| {
            Error::unavailable("library edits", "directory commit adapter missing")
        })?;
        let baseline = durability.recover(root)?;
        let manifest = control
            .read_manifest(root)?
            .ok_or_else(|| Error::unavailable("library edits", "library manifest missing"))?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let now = u64::try_from(now).unwrap_or(u64::MAX);
        let mut playlist = if rename {
            match control.read_record(root, RecordKind::Playlist, &id.to_string())? {
                Some(PortableRecord::Playlist(record)) => record,
                _ => return Err(Error::unavailable("playlist", "not in the active library")),
            }
        } else {
            PlaylistRecord {
                playlist_uuid: id.as_uuid(),
                display_name: name.to_owned(),
                created_at: Some(now),
                cover_asset: None,
                revision: Revision::INITIAL,
                hlc: HybridLogicalClock::default(),
                updated_by_device_id: device,
            }
        };
        name.clone_into(&mut playlist.display_name);
        playlist.revision = Revision::from_u64(
            playlist
                .revision
                .as_u64()
                .checked_add(1)
                .ok_or_else(|| Error::conflict("playlist revision exhausted"))?,
        );
        playlist.hlc = crate::domain::library::next_hlc(playlist.hlc, now / 1000);
        playlist.updated_by_device_id = device;
        let record = PortableRecord::Playlist(playlist);
        let state = candidate::freeze(
            &baseline,
            manifest.library_id,
            crate::domain::sync::DeviceId::from_uuid(device.as_uuid()),
            &record,
            now,
        )?;
        let record = PortableRecordEnvelope::new(record, None)?;
        let mut batch = LibraryBatch::new(
            uuid::Uuid::new_v4(),
            manifest.library_id,
            ChangeOrigin::LocalEdit,
            vec![record],
        );
        batch.state = state;
        commit_then_project(control, root, &batch, |_| {
            let replay = durability.recover(root)?;
            let mut candidate = store.clone();
            project_records(&mut candidate, root, &replay);
            if self.fail_commit.load(std::sync::atomic::Ordering::Relaxed) {
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
}

fn project_records(
    store: &mut Store,
    root: LibraryRootId,
    replay: &crate::domain::library::durability::LibraryReplay,
) {
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
            PortableRecord::Favorite(record) => {
                if let Some(song) = store.songs.get_mut(&SongId::from_uuid(record.song_uuid)) {
                    if song.root() == root {
                        song.set_favorite(record.is_favorite);
                    }
                }
            }
            PortableRecord::PlayStats(record) => {
                if let Some(song) = store.songs.get_mut(&SongId::from_uuid(record.song_uuid)) {
                    if song.root() == root {
                        song.restore_play_count(crate::domain::ids::PlayCount::from_u64(
                            record.total(),
                        ));
                    }
                }
            }
            PortableRecord::Playlist(record) => {
                let id = PlaylistId::from_uuid(record.playlist_uuid);
                let cover = store
                    .playlists
                    .get(&id)
                    .and_then(|(_, _, cover)| cover.clone());
                store
                    .playlists
                    .insert(id, (root, record.display_name.clone(), cover));
            }
            PortableRecord::PlaylistItem(record) => {
                let playlist = PlaylistId::from_uuid(record.playlist_uuid);
                let song = SongId::from_uuid(record.song_uuid);
                if let Some(current) = store.songs.get(&song) {
                    if current.root() == root && store.playlists.contains_key(&playlist) {
                        let member = crate::domain::entities::PlaylistMember::with_id(
                            crate::domain::ids::PlaylistItemId::from_uuid(record.item_uuid),
                            playlist,
                            song,
                            record.position,
                            current.availability(),
                        );
                        store.members.insert((playlist, song), member);
                    }
                }
            }
            PortableRecord::Tombstone(record)
                if record.deleted_kind == RecordKind::PlaylistItem =>
            {
                store
                    .members
                    .retain(|_, member| member.id().as_uuid() != record.object_uuid);
            }
            PortableRecord::Tombstone(record) if record.deleted_kind == RecordKind::Playlist => {
                let id = PlaylistId::from_uuid(record.object_uuid);
                store.playlists.remove(&id);
                store.members.retain(|(playlist, _), _| *playlist != id);
            }
            _ => {}
        }
    }
}
