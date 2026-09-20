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
use crate::domain::entities::{
    LibraryRoot, LyricsCandidate, LyricsSource, PlaylistMember, Song, SongAvailability,
};
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
    undo_deadlines: BTreeMap<OperationId, i64>,
    lyrics: BTreeMap<(SongId, LyricsSource), LyricsCandidate>,
    covers: BTreeMap<SongId, CoverAssetRef>,
    runtime_state: BTreeMap<String, String>,
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

impl TxSongWriter for MemoryTx<'_> {
    fn upsert_song(&mut self, song: &Song) -> Result<(), Error> {
        // Same contract as the SQL upsert (see `testing::song_upsert`): user
        // state on an existing row is never carried by a metadata write.
        let merged = crate::application::testing::song_upsert::metadata_upsert(
            self.state.songs.get(&song.id()),
            song,
        );
        self.state.songs.insert(song.id(), merged);
        Ok(())
    }

    fn delete_song(&mut self, id: SongId) -> Result<(), Error> {
        self.state.songs.remove(&id);
        self.state.members.retain(|(_, song), _| *song != id);
        self.state.lyrics.retain(|(song, _), _| *song != id);
        self.state.covers.remove(&id);
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
}

impl TxRootWriter for MemoryTx<'_> {
    fn upsert_root(&mut self, root: &LibraryRoot) -> Result<(), Error> {
        self.state.roots.insert(root.id(), root.clone());
        Ok(())
    }
    fn isolate_root_writes(&mut self, id: LibraryRootId, available: bool) -> Result<(), Error> {
        if let Some(root) = self.state.roots.get_mut(&id) {
            root.set_write_safety_locked(true);
            root.set_availability(if available {
                crate::domain::entities::RootAvailability::Available
            } else {
                crate::domain::entities::RootAvailability::Unavailable
            });
        }
        Ok(())
    }
}

impl TxPlaylistWriter for MemoryTx<'_> {
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
}

impl TxOperationWriter for MemoryTx<'_> {
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

    fn release_operation_claims(&mut self, _operation: OperationId) -> Result<(), Error> {
        Ok(())
    }

    fn set_undo_deadline(&mut self, operation: OperationId, deadline_ms: i64) -> Result<(), Error> {
        self.state.undo_deadlines.insert(operation, deadline_ms);
        Ok(())
    }
}

impl TxLyricsWriter for MemoryTx<'_> {
    fn set_lyrics_candidate(
        &mut self,
        song: SongId,
        candidate: &LyricsCandidate,
    ) -> Result<(), Error> {
        self.state
            .lyrics
            .insert((song, candidate.source()), candidate.clone());
        Ok(())
    }

    fn clear_lyrics_candidate(&mut self, song: SongId, source: LyricsSource) -> Result<(), Error> {
        self.state.lyrics.remove(&(song, source));
        Ok(())
    }
}

impl TxStateWriter for MemoryTx<'_> {
    fn attach_cover(&mut self, song: SongId, cover: &CoverAssetRef) -> Result<(), Error> {
        self.state.covers.insert(song, cover.clone());
        Ok(())
    }

    fn set_runtime_state(&mut self, key: &str, value: &str) -> Result<(), Error> {
        self.state
            .runtime_state
            .insert(key.to_owned(), value.to_owned());
        Ok(())
    }
}

impl MemoryUnitOfWork {
    /// Assertion helper: committed lyrics candidates of one song.
    #[must_use]
    pub fn lyrics_of(&self, song: SongId) -> Vec<LyricsCandidate> {
        self.state
            .lock()
            .unwrap()
            .lyrics
            .iter()
            .filter(|((s, _), _)| *s == song)
            .map(|(_, c)| c.clone())
            .collect()
    }

    /// Assertion helper: committed cover reference of one song.
    #[must_use]
    pub fn cover_of(&self, song: SongId) -> Option<CoverAssetRef> {
        self.state.lock().unwrap().covers.get(&song).cloned()
    }

    /// Assertion helper: committed runtime key/value state.
    #[must_use]
    pub fn runtime_state(&self, key: &str) -> Option<String> {
        self.state.lock().unwrap().runtime_state.get(key).cloned()
    }
}

impl UnitOfWork for MemoryUnitOfWork {
    fn with_tx(&self, f: TxWork) -> Result<(), Error> {
        let mut candidate = self.state.lock().unwrap().clone();
        f(&mut MemoryTx {
            state: &mut candidate,
        })?;
        if *self.fail_commit.lock().unwrap() {
            return Err(Error::unavailable(
                "test transaction",
                "simulated commit failure",
            ));
        }
        *self.state.lock().unwrap() = candidate;
        Ok(())
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
        uow.with_tx(Box::new(move |tx: &mut dyn TxAccess| tx.upsert_song(&song)))
            .unwrap();
        assert_eq!(uow.songs().len(), 1);

        uow.set_fail_commit(true);
        let second = Song::new(
            SongId::new(),
            root(),
            RelativeMediaPath::new("two.flac").unwrap(),
            Revision::INITIAL,
        );
        assert!(uow
            .with_tx(Box::new(
                move |tx: &mut dyn TxAccess| tx.upsert_song(&second)
            ))
            .is_err());
        assert_eq!(uow.songs().len(), 1, "failed commit rolls back every write");
    }
}
