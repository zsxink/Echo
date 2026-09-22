//! `LibraryRepository` / `SongRepository` / `PlaylistRepository` views of
//! the shared store.

use super::*;

impl LibraryRepository for MemoryDatabase {
    fn active_root(&self) -> Result<Option<LibraryRoot>, Error> {
        Ok(self
            .lock()
            .roots
            .values()
            .find(|root| root.is_active())
            .cloned())
    }
    fn by_id(&self, id: LibraryRootId) -> Result<Option<LibraryRoot>, Error> {
        Ok(self.lock().roots.get(&id).cloned())
    }
    fn upsert(&self, root: &LibraryRoot) -> Result<(), Error> {
        if root.is_active() {
            let others: Vec<LibraryRootId> = self
                .lock()
                .roots
                .values()
                .filter(|other| other.is_active() && other.id() != root.id())
                .map(LibraryRoot::id)
                .collect();
            for id in others {
                self.deactivate(id)?;
            }
        }
        self.lock().roots.insert(root.id(), root.clone());
        Ok(())
    }
    fn deactivate(&self, id: LibraryRootId) -> Result<(), Error> {
        if let Some(root) = self.lock().roots.get_mut(&id) {
            let replacement = LibraryRoot::new(
                root.id(),
                root.absolute_path().to_path_buf(),
                false,
                root.observed_write_capable(),
            );
            let mut replacement = replacement;
            replacement.set_write_safety_locked(root.write_safety_locked());
            *root = replacement;
        }
        Ok(())
    }
    fn set_write_and_availability(
        &self,
        id: LibraryRootId,
        write_capable: bool,
        available: bool,
    ) -> Result<(), Error> {
        if let Some(root) = self.lock().roots.get_mut(&id) {
            let replacement = LibraryRoot::new(
                root.id(),
                root.absolute_path().to_path_buf(),
                root.is_active(),
                write_capable,
            );
            let mut replacement = replacement;
            replacement.set_write_safety_locked(root.write_safety_locked());
            replacement.set_availability(if available {
                crate::domain::entities::RootAvailability::Available
            } else {
                crate::domain::entities::RootAvailability::Unavailable
            });
            *root = replacement;
        }
        Ok(())
    }
    fn set_write_safety_locked(&self, id: LibraryRootId, locked: bool) -> Result<(), Error> {
        if let Some(root) = self.lock().roots.get_mut(&id) {
            root.set_write_safety_locked(locked);
        }
        Ok(())
    }
}

impl SongRepository for MemoryDatabase {
    fn by_id(&self, id: SongId) -> Result<Option<Song>, Error> {
        Ok(self.lock().songs.get(&id).cloned())
    }
    fn by_path(
        &self,
        root: LibraryRootId,
        path: &RelativeMediaPath,
    ) -> Result<Option<Song>, Error> {
        Ok(self
            .lock()
            .songs
            .values()
            .find(|song| song.root() == root && song.path() == path)
            .cloned())
    }
    fn all_in_root(&self, root: LibraryRootId) -> Result<Vec<Song>, Error> {
        Ok(self
            .lock()
            .songs
            .values()
            .filter(|song| song.root() == root)
            .cloned()
            .collect())
    }
    fn upsert(&self, song: &Song) -> Result<(), Error> {
        // Same contract as the SQL upsert (see `testing::song_upsert`): user
        // state on an existing row is never carried by a metadata write.
        let mut guard = self.lock();
        let merged = crate::application::testing::song_upsert::metadata_upsert(
            guard.songs.get(&song.id()),
            song,
        );
        guard.songs.insert(song.id(), merged);
        Ok(())
    }
    fn set_availability(&self, id: SongId, availability: SongAvailability) -> Result<(), Error> {
        if let Some(song) = self.lock().songs.get_mut(&id) {
            match availability {
                SongAvailability::Available => song.restore_available(),
                SongAvailability::Missing => song.mark_missing(),
                SongAvailability::PendingDelete => song.begin_pending_delete(),
            }
        }
        Ok(())
    }
    fn set_favorite(&self, id: SongId, favorite: bool) -> Result<(), Error> {
        if let Some(song) = self.lock().songs.get_mut(&id) {
            song.set_favorite(favorite);
        }
        Ok(())
    }
    fn increment_play_count(&self, id: SongId) -> Result<(), Error> {
        if let Some(song) = self.lock().songs.get_mut(&id) {
            song.record_play();
        }
        Ok(())
    }
    fn set_play_count(&self, id: SongId, count: u64) -> Result<(), Error> {
        if let Some(song) = self.lock().songs.get_mut(&id) {
            song.restore_play_count(crate::domain::ids::PlayCount::from_u64(count));
        }
        Ok(())
    }
}

impl PlaylistRepository for MemoryDatabase {
    fn by_id(&self, id: PlaylistId) -> Result<Option<PlaylistId>, Error> {
        Ok(self.lock().playlists.contains_key(&id).then_some(id))
    }
    fn name(&self, id: PlaylistId) -> Result<Option<String>, Error> {
        Ok(self
            .lock()
            .playlists
            .get(&id)
            .map(|(_, name, _)| name.clone()))
    }
    fn cover_key(&self, id: PlaylistId) -> Result<Option<String>, Error> {
        Ok(self
            .lock()
            .playlists
            .get(&id)
            .and_then(|(_, _, key)| key.clone()))
    }
    fn by_name(
        &self,
        root: LibraryRootId,
        normalized_name: &str,
    ) -> Result<Option<PlaylistId>, Error> {
        let key = playlist_name_key(normalized_name);
        Ok(self
            .lock()
            .playlists
            .iter()
            .find(|(_, (r, n, _))| *r == root && playlist_name_key(n) == key)
            .map(|(id, _)| *id))
    }
    fn list(&self, root: LibraryRootId) -> Result<Vec<PlaylistId>, Error> {
        Ok(self
            .lock()
            .playlists
            .iter()
            .filter(|(_, (r, _, _))| *r == root)
            .map(|(id, _)| *id)
            .collect())
    }
    fn create(&self, id: PlaylistId, root: LibraryRootId, name: &str) -> Result<(), Error> {
        let mut store = self.lock();
        let key = playlist_name_key(name);
        if store
            .playlists
            .iter()
            .any(|(_, (r, n, _))| *r == root && playlist_name_key(n) == key)
        {
            return Err(Error::conflict("playlist name already exists"));
        }
        store.playlists.insert(id, (root, name.to_owned(), None));
        Ok(())
    }
    fn rename(&self, id: PlaylistId, to_normalized_name: &str) -> Result<(), Error> {
        let mut store = self.lock();
        let Some((root, _, _)) = store.playlists.get(&id).cloned() else {
            return Ok(());
        };
        let key = playlist_name_key(to_normalized_name);
        if store
            .playlists
            .iter()
            .any(|(other, (r, n, _))| *other != id && *r == root && playlist_name_key(n) == key)
        {
            return Err(Error::conflict("playlist name already exists"));
        }
        if let Some((_, name, _)) = store.playlists.get_mut(&id) {
            to_normalized_name.clone_into(name);
        }
        Ok(())
    }
    fn set_cover_key(&self, id: PlaylistId, key: Option<&str>) -> Result<(), Error> {
        if let Some((_, _, cover)) = self.lock().playlists.get_mut(&id) {
            *cover = key.map(str::to_owned);
        }
        Ok(())
    }
    fn delete(&self, id: PlaylistId) -> Result<(), Error> {
        let mut store = self.lock();
        store.playlists.remove(&id);
        store.members.retain(|(playlist, _), _| *playlist != id);
        Ok(())
    }
    fn members(&self, id: PlaylistId) -> Result<Vec<PlaylistMember>, Error> {
        let store = self.lock();
        let mut rows: Vec<_> = store
            .members
            .values()
            .filter(|member| member.playlist() == id)
            .map(|member| {
                // Mirror SQLite's `members()` JOIN: the member's reported
                // availability is the *song's* live availability, so a
                // pending-delete song stops counting immediately while its
                // membership row survives (undo restores it). The stored
                // member is stale by design otherwise.
                let Some(song) = store.songs.get(&member.song()) else {
                    return member.clone();
                };
                if song.availability() == member.song_availability() {
                    return member.clone();
                }
                let mut live = member.clone();
                live.mirror_song(song.availability());
                live
            })
            .collect();
        // Mirror the SQL order: position, then stable UUID tie-break.
        rows.sort_by(|a, b| {
            a.position()
                .cmp(&b.position())
                .then(a.song().cmp(&b.song()))
        });
        Ok(rows)
    }
    fn add_member(&self, playlist: PlaylistId, song: SongId, position: u64) -> Result<(), Error> {
        let mut store = self.lock();
        if store.members.contains_key(&(playlist, song)) {
            return Ok(());
        }
        // Mirror SQLite: appending takes the next free position (max + 1), so
        // ordering is strictly positional and a fresh playlist starts at 0.
        let next = store
            .members
            .iter()
            .filter(|((p, _), _)| *p == playlist)
            .map(|(_, member)| member.position())
            .max()
            .map_or(0, |current| {
                if current == u64::MAX {
                    u64::MAX
                } else {
                    current + 1
                }
            });
        let position = if position == u64::MAX { next } else { position };
        store.members.insert(
            (playlist, song),
            PlaylistMember::new(playlist, song, position, SongAvailability::Available),
        );
        Ok(())
    }
    fn upsert_member(&self, member: &PlaylistMember) -> Result<(), Error> {
        self.lock()
            .members
            .insert((member.playlist(), member.song()), member.clone());
        Ok(())
    }
    fn remove_member(&self, playlist: PlaylistId, song: SongId) -> Result<(), Error> {
        self.lock().members.remove(&(playlist, song));
        Ok(())
    }
}
