//! Atomic in-memory Unit of Work with real commit/rollback semantics.

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

/// Transactional in-memory state. Each `with_tx` closure receives an isolated
/// copy and commits it only when the closure and commit both succeed.
#[derive(Clone, Debug, Default)]
struct MemoryTxState {
    songs: BTreeMap<SongId, Song>,
    roots: BTreeMap<LibraryRootId, LibraryRoot>,
    playlists: BTreeMap<PlaylistId, (LibraryRootId, String)>,
    members: BTreeMap<(PlaylistId, SongId), PlaylistMember>,
    operations: BTreeMap<(OperationId, String), OperationItem>,
}

/// A Unit-of-Work fake with real commit/rollback semantics and a scriptable
/// commit failure. Use-case tests can assert multi-aggregate mutations are
/// atomic without opening SQLite or a user directory.
#[derive(Clone, Debug, Default)]
pub struct MemoryUnitOfWork {
    state: Shared<MemoryTxState>,
    fail_commit: Shared<bool>,
}

impl MemoryUnitOfWork {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Cause the next transaction commit attempt to fail without changing the
    /// stored snapshot.
    pub fn set_fail_commit(&self, fail: bool) {
        *self.fail_commit.lock().unwrap() = fail;
    }

    /// Assertion helper exposing the committed songs only.
    #[must_use]
    pub fn songs(&self) -> Vec<Song> {
        self.state.lock().unwrap().songs.values().cloned().collect()
    }
}

struct MemoryTx<'a> {
    state: &'a mut MemoryTxState,
}

impl TxAccess for MemoryTx<'_> {
    fn upsert_song(&mut self, song: &Song) -> Result<(), Error> {
        self.state.songs.insert(song.id(), song.clone());
        Ok(())
    }

    fn set_song_availability(
        &mut self,
        id: SongId,
        availability: SongAvailability,
    ) -> Result<(), Error> {
        if let Some(song) = self.state.songs.get_mut(&id) {
            match availability {
                SongAvailability::Available => song.restore_available(),
                SongAvailability::Missing => song.mark_missing(),
                SongAvailability::PendingDelete => song.begin_pending_delete(),
            }
        }
        Ok(())
    }

    fn set_song_favorite(&mut self, id: SongId, favorite: bool) -> Result<(), Error> {
        if let Some(song) = self.state.songs.get_mut(&id) {
            song.set_favorite(favorite);
        }
        Ok(())
    }

    fn increment_song_play_count(&mut self, id: SongId) -> Result<(), Error> {
        if let Some(song) = self.state.songs.get_mut(&id) {
            song.record_play();
        }
        Ok(())
    }

    fn upsert_root(&mut self, root: &LibraryRoot) -> Result<(), Error> {
        self.state.roots.insert(root.id(), root.clone());
        Ok(())
    }

    fn create_playlist(
        &mut self,
        id: PlaylistId,
        root: LibraryRootId,
        name: &str,
    ) -> Result<(), Error> {
        self.state.playlists.insert(id, (root, name.to_owned()));
        Ok(())
    }

    fn insert_member(&mut self, member: &PlaylistMember) -> Result<(), Error> {
        self.state
            .members
            .entry((member.playlist(), member.song()))
            .or_insert_with(|| member.clone());
        Ok(())
    }

    fn remove_member(&mut self, playlist: PlaylistId, song: SongId) -> Result<(), Error> {
        self.state.members.remove(&(playlist, song));
        Ok(())
    }

    fn upsert_operation_item(
        &mut self,
        operation: OperationId,
        item: OperationItem,
    ) -> Result<(), Error> {
        self.state
            .operations
            .insert((operation, item.target_path.normalized().to_owned()), item);
        Ok(())
    }
}

impl UnitOfWork for MemoryUnitOfWork {
    fn with_tx<T: Send + 'static>(
        &self,
        f: impl FnOnce(&mut dyn TxAccess) -> Result<T, Error> + Send + 'static,
    ) -> Result<T, Error> {
        let mut candidate = self.state.lock().unwrap().clone();
        let result = f(&mut MemoryTx {
            state: &mut candidate,
        })?;
        if *self.fail_commit.lock().unwrap() {
            return Err(Error::unavailable(
                "test transaction",
                "simulated commit failure",
            ));
        }
        *self.state.lock().unwrap() = candidate;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> LibraryRootId {
        LibraryRootId::new()
    }

    #[test]
    fn memory_unit_of_work_commits_all_changes_or_none() {
        let uow = MemoryUnitOfWork::new();
        let song = Song::new(
            SongId::new(),
            root(),
            RelativeMediaPath::new("one.flac").unwrap(),
            Revision::INITIAL,
        );
        uow.with_tx(move |tx| tx.upsert_song(&song)).unwrap();
        assert_eq!(uow.songs().len(), 1);

        uow.set_fail_commit(true);
        let second = Song::new(
            SongId::new(),
            root(),
            RelativeMediaPath::new("two.flac").unwrap(),
            Revision::INITIAL,
        );
        assert!(uow.with_tx(move |tx| tx.upsert_song(&second)).is_err());
        assert_eq!(uow.songs().len(), 1, "failed commit rolls back every write");
    }
}
