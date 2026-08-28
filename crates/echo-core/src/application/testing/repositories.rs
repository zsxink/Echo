//! In-memory repository doubles mirroring each Repository port.

#![allow(
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_lossless,
    clippy::too_many_lines,
    clippy::must_use_candidate,
    clippy::unnecessary_to_owned,
    clippy::redundant_clone,
    clippy::doc_markdown,
    clippy::let_and_return,
    clippy::needless_borrow,
    clippy::needless_pass_by_value,
    clippy::manual_let_else,
    clippy::unchecked_time_subtraction,
    clippy::wildcard_imports,
    clippy::bool_assert_comparison,
    clippy::type_complexity,
    clippy::missing_const_for_fn,
    clippy::significant_drop_in_scrutinee,
    clippy::significant_drop_tightening,
    clippy::manual_map,
    clippy::map_unwrap_or
)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::application::ports::*;
use crate::domain::entities::{LibraryRoot, PlaylistMember, Song, SongAvailability};
use crate::domain::ids::*;
use crate::error::Error;

/// Shared interior-mutability cell backing every in-memory fake.
type Shared<T> = Arc<Mutex<T>>;

/// In-memory song store.
#[derive(Clone, Debug, Default)]
pub struct MemorySongRepository {
    songs: Shared<BTreeMap<SongId, Song>>,
}

impl MemorySongRepository {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Programmatic insertion for test setup (bypasses port API).
    pub fn seed(&self, song: Song) {
        self.songs.lock().unwrap().insert(song.id(), song);
    }

    /// Copy of all songs (assertion helper).
    pub fn snapshot(&self) -> Vec<Song> {
        self.songs.lock().unwrap().values().cloned().collect()
    }
}

impl SongRepository for MemorySongRepository {
    fn by_id(&self, id: SongId) -> Result<Option<Song>, Error> {
        Ok(self.songs.lock().unwrap().get(&id).cloned())
    }
    fn by_path(
        &self,
        root: LibraryRootId,
        path: &RelativeMediaPath,
    ) -> Result<Option<Song>, Error> {
        Ok(self
            .songs
            .lock()
            .unwrap()
            .values()
            .find(|s| s.root() == root && s.path() == path)
            .cloned())
    }
    fn upsert(&self, song: &Song) -> Result<(), Error> {
        self.songs.lock().unwrap().insert(song.id(), song.clone());
        Ok(())
    }
    fn set_availability(&self, id: SongId, availability: SongAvailability) -> Result<(), Error> {
        if let Some(s) = self.songs.lock().unwrap().get_mut(&id) {
            match availability {
                SongAvailability::Available => s.restore_available(),
                SongAvailability::Missing => s.mark_missing(),
                SongAvailability::PendingDelete => s.begin_pending_delete(),
            }
        }
        Ok(())
    }
    fn set_favorite(&self, id: SongId, favorite: bool) -> Result<(), Error> {
        if let Some(s) = self.songs.lock().unwrap().get_mut(&id) {
            s.set_favorite(favorite);
        }
        Ok(())
    }
    fn increment_play_count(&self, id: SongId) -> Result<(), Error> {
        if let Some(s) = self.songs.lock().unwrap().get_mut(&id) {
            s.record_play();
        }
        Ok(())
    }
}

/// In-memory library-root store.
#[derive(Clone, Debug, Default)]
pub struct MemoryLibraryRepository {
    roots: Shared<BTreeMap<LibraryRootId, LibraryRoot>>,
}

impl MemoryLibraryRepository {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl LibraryRepository for MemoryLibraryRepository {
    fn active_root(&self) -> Result<Option<LibraryRoot>, Error> {
        Ok(self
            .roots
            .lock()
            .unwrap()
            .values()
            .find(|r| r.is_active())
            .cloned())
    }
    fn by_id(&self, id: LibraryRootId) -> Result<Option<LibraryRoot>, Error> {
        Ok(self.roots.lock().unwrap().get(&id).cloned())
    }
    fn upsert(&self, root: &LibraryRoot) -> Result<(), Error> {
        self.roots.lock().unwrap().insert(root.id(), root.clone());
        Ok(())
    }
    fn deactivate(&self, id: LibraryRootId) -> Result<(), Error> {
        if let Some(r) = self.roots.lock().unwrap().get_mut(&id) {
            *r = LibraryRoot::new(
                r.id(),
                r.absolute_path().to_path_buf(),
                false,
                r.write_capable(),
            );
        }
        Ok(())
    }
    fn set_write_and_availability(
        &self,
        id: LibraryRootId,
        write_capable: bool,
        available: bool,
    ) -> Result<(), Error> {
        if let Some(r) = self.roots.lock().unwrap().get_mut(&id) {
            r.set_write_capable(write_capable);
            r.set_availability(if available {
                crate::domain::entities::RootAvailability::Available
            } else {
                crate::domain::entities::RootAvailability::Unavailable
            });
        }
        Ok(())
    }
}

/// In-memory playlist store (members + names).
#[derive(Clone, Debug, Default)]
pub struct MemoryPlaylistRepository {
    names: Shared<BTreeMap<PlaylistId, (LibraryRootId, String)>>,
    members: Shared<BTreeMap<(PlaylistId, SongId), PlaylistMember>>,
    next_position: Shared<BTreeMap<PlaylistId, u64>>,
}

impl MemoryPlaylistRepository {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl PlaylistRepository for MemoryPlaylistRepository {
    fn by_id(&self, id: PlaylistId) -> Result<Option<PlaylistId>, Error> {
        Ok(self.names.lock().unwrap().contains_key(&id).then_some(id))
    }
    fn by_name(
        &self,
        root: LibraryRootId,
        normalized_name: &str,
    ) -> Result<Option<PlaylistId>, Error> {
        Ok(self
            .names
            .lock()
            .unwrap()
            .iter()
            .find(|(_, (r, n))| *r == root && n == normalized_name)
            .map(|(id, _)| *id))
    }
    fn list(&self, root: LibraryRootId) -> Result<Vec<PlaylistId>, Error> {
        Ok(self
            .names
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, (r, _))| *r == root)
            .map(|(id, _)| *id)
            .collect())
    }
    fn create(&self, id: PlaylistId, root: LibraryRootId, name: &str) -> Result<(), Error> {
        self.names
            .lock()
            .unwrap()
            .insert(id, (root, name.to_owned()));
        self.next_position.lock().unwrap().insert(id, 0);
        Ok(())
    }
    fn rename(&self, id: PlaylistId, to_normalized_name: &str) -> Result<(), Error> {
        if let Some((_, n)) = self.names.lock().unwrap().get_mut(&id) {
            to_normalized_name.clone_into(n);
        }
        Ok(())
    }
    fn delete(&self, id: PlaylistId) -> Result<(), Error> {
        self.names.lock().unwrap().remove(&id);
        let mut members = self.members.lock().unwrap();
        members.retain(|(p, _), _| *p != id);
        Ok(())
    }
    fn members(&self, id: PlaylistId) -> Result<Vec<PlaylistMember>, Error> {
        Ok(self
            .members
            .lock()
            .unwrap()
            .iter()
            .filter(|((p, _), _)| *p == id)
            .map(|(_, m)| m.clone())
            .collect())
    }
    fn add_member(&self, playlist: PlaylistId, song: SongId, position: u64) -> Result<(), Error> {
        // A duplicate (playlist, song) is a no-op — never overwrites the
        // original position (design: repeated add is idempotent).
        let members = self.members.lock().unwrap();
        if members.contains_key(&(playlist, song)) {
            return Ok(());
        }
        drop(members);

        let mut next = self.next_position.lock().unwrap();
        let pos = next.entry(playlist).or_insert(0);
        let position = if position == u64::MAX { *pos } else { position };
        *pos = position.saturating_add(1).max(*pos);
        self.members.lock().unwrap().insert(
            (playlist, song),
            PlaylistMember::new(playlist, song, position, SongAvailability::Available),
        );
        Ok(())
    }
    fn remove_member(&self, playlist: PlaylistId, song: SongId) -> Result<(), Error> {
        self.members.lock().unwrap().remove(&(playlist, song));
        Ok(())
    }
}

/// In-memory operation journal.
#[derive(Clone, Debug, Default)]
pub struct MemoryOperationJournal {
    items: Shared<BTreeMap<(OperationId, String), OperationItem>>,
}

impl MemoryOperationJournal {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl OperationJournalRepository for MemoryOperationJournal {
    fn item_state(
        &self,
        operation: OperationId,
        item: &str,
    ) -> Result<Option<OperationItem>, Error> {
        Ok(self
            .items
            .lock()
            .unwrap()
            .get(&(operation, item.to_owned()))
            .cloned())
    }
    fn upsert_item(&self, operation: OperationId, item: OperationItem) -> Result<(), Error> {
        self.items
            .lock()
            .unwrap()
            .insert((operation, item.target_path.normalized().to_owned()), item);
        Ok(())
    }
    fn items(&self, operation: OperationId) -> Result<Vec<OperationItem>, Error> {
        Ok(self
            .items
            .lock()
            .unwrap()
            .iter()
            .filter(|((op, _), _)| *op == operation)
            .map(|(_, i)| i.clone())
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> LibraryRootId {
        LibraryRootId::new()
    }

    #[test]
    fn memory_repositories_round_trip() {
        let songs = MemorySongRepository::new();
        let root = LibraryRootId::new();
        let sid = SongId::new();
        let path = RelativeMediaPath::new("周杰伦/晴天.flac").unwrap();
        let mut song = Song::new(sid, root, path.clone(), Revision::INITIAL);
        song.set_favorite(true);
        songs.upsert(&song).unwrap();
        assert_eq!(songs.by_id(sid).unwrap().unwrap().favorite(), true);
        assert_eq!(
            songs.by_path(root, &path).unwrap().unwrap().id(),
            sid,
            "by_path hits the same record"
        );
        songs.increment_play_count(sid).unwrap();
        assert_eq!(songs.by_id(sid).unwrap().unwrap().play_count().as_u64(), 1);
        songs
            .set_availability(sid, SongAvailability::Missing)
            .unwrap();
        assert_eq!(
            songs.by_id(sid).unwrap().unwrap().availability(),
            SongAvailability::Missing
        );
    }

    #[test]
    fn memory_playlist_members_append_without_duplication() {
        let repo = MemoryPlaylistRepository::new();
        let pid = PlaylistId::new();
        let sid = SongId::new();
        let r = root();
        repo.create(pid, r, "通勤路上").unwrap();
        repo.add_member(pid, sid, 0).unwrap();
        repo.add_member(pid, sid, u64::MAX).unwrap(); // duplicate append
        let members = repo.members(pid).unwrap();
        assert_eq!(members.len(), 1, "same (playlist, song) collapses");
        assert_eq!(members[0].position(), 0);
    }
}
