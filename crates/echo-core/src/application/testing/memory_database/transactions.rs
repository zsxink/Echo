//! The unit of work: `MemoryTx` plus the per-aggregate writers that only
//! exist inside a transaction.

use super::*;

/// The transaction view: an isolated copy committed on success.
struct MemoryTx<'a> {
    store: &'a mut Store,
}

impl MemoryTx<'_> {
    /// Mirror the sqlite write path: a song logic change is a syncable fact —
    /// bump the object's outbox revision in the same transaction.
    fn bump_song_revision(&mut self, id: SongId) {
        let key = ("song".to_owned(), id.to_string());
        let next = self
            .store
            .outbox_revisions
            .get(&key)
            .copied()
            .map_or(1, |rev| rev + 1);
        self.store.outbox_revisions.insert(key, next);
    }
}

impl TxSongWriter for MemoryTx<'_> {
    fn upsert_song(&mut self, song: &Song) -> Result<(), Error> {
        self.bump_song_revision(song.id());
        self.store.songs.insert(song.id(), song.clone());
        Ok(())
    }
    fn delete_song(&mut self, id: SongId) -> Result<(), Error> {
        self.store.songs.remove(&id);
        self.store.members.retain(|(_, song), _| *song != id);
        self.store.lyrics.retain(|(song, _), _| *song != id);
        self.store.covers.remove(&id);
        Ok(())
    }
    fn set_song_availability(
        &mut self,
        id: SongId,
        availability: SongAvailability,
    ) -> Result<(), Error> {
        self.bump_song_revision(id);
        if let Some(song) = self.store.songs.get_mut(&id) {
            match availability {
                SongAvailability::Available => song.restore_available(),
                SongAvailability::Missing => song.mark_missing(),
                SongAvailability::PendingDelete => song.begin_pending_delete(),
            }
        }
        Ok(())
    }
    fn set_song_favorite(&mut self, id: SongId, favorite: bool) -> Result<(), Error> {
        self.bump_song_revision(id);
        if let Some(song) = self.store.songs.get_mut(&id) {
            song.set_favorite(favorite);
        }
        Ok(())
    }
    fn increment_song_play_count(&mut self, id: SongId) -> Result<(), Error> {
        if let Some(song) = self.store.songs.get_mut(&id) {
            song.record_play();
        }
        Ok(())
    }
}

impl TxRootWriter for MemoryTx<'_> {
    fn upsert_root(&mut self, root: &LibraryRoot) -> Result<(), Error> {
        if root.is_active() {
            let others: Vec<LibraryRootId> = self
                .store
                .roots
                .values()
                .filter(|other| other.is_active() && other.id() != root.id())
                .map(LibraryRoot::id)
                .collect();
            for id in others {
                if let Some(other) = self.store.roots.get_mut(&id) {
                    *other = LibraryRoot::new(
                        other.id(),
                        other.absolute_path().to_path_buf(),
                        false,
                        other.write_capable(),
                    );
                }
            }
        }
        self.store.roots.insert(root.id(), root.clone());
        Ok(())
    }
    fn isolate_root_writes(&mut self, id: LibraryRootId, available: bool) -> Result<(), Error> {
        if self.store.fail_root_isolation {
            return Err(Error::unavailable(
                "test root isolation",
                "simulated root isolation write failure",
            ));
        }
        if let Some(root) = self.store.roots.get_mut(&id) {
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
        self.store
            .playlists
            .insert(id, (root, name.to_owned(), None));
        Ok(())
    }
    fn insert_member(&mut self, member: &PlaylistMember) -> Result<(), Error> {
        self.store
            .members
            .entry((member.playlist(), member.song()))
            .or_insert_with(|| member.clone());
        Ok(())
    }
    fn remove_member(&mut self, playlist: PlaylistId, song: SongId) -> Result<(), Error> {
        self.store.members.remove(&(playlist, song));
        Ok(())
    }
}

impl TxOperationWriter for MemoryTx<'_> {
    fn upsert_operation_item(
        &mut self,
        operation: OperationId,
        item: OperationItem,
    ) -> Result<(), Error> {
        self.store
            .operations
            .insert((operation, item.item_key.clone()), item);
        Ok(())
    }
    fn release_operation_claims(&mut self, operation: OperationId) -> Result<(), Error> {
        self.store.released_claims.push(operation);
        Ok(())
    }
    fn set_undo_deadline(&mut self, operation: OperationId, deadline_ms: i64) -> Result<(), Error> {
        self.store.undo_deadlines.insert(operation, deadline_ms);
        Ok(())
    }
}

impl TxLyricsWriter for MemoryTx<'_> {
    fn set_lyrics_candidate(
        &mut self,
        song: SongId,
        candidate: &LyricsCandidate,
    ) -> Result<(), Error> {
        self.store
            .lyrics
            .insert((song, candidate.source()), candidate.clone());
        Ok(())
    }
    fn clear_lyrics_candidate(&mut self, song: SongId, source: LyricsSource) -> Result<(), Error> {
        self.store.lyrics.remove(&(song, source));
        Ok(())
    }
}

impl TxStateWriter for MemoryTx<'_> {
    fn attach_cover(&mut self, song: SongId, cover: &CoverAssetRef) -> Result<(), Error> {
        self.store.covers.insert(song, cover.clone());
        Ok(())
    }
    fn set_runtime_state(&mut self, key: &str, value: &str) -> Result<(), Error> {
        self.store
            .runtime_state
            .insert(key.to_owned(), value.to_owned());
        Ok(())
    }
}

impl UnitOfWork for MemoryDatabase {
    fn with_tx(&self, f: TxWork) -> Result<(), Error> {
        let mut candidate = self.lock().clone();
        f(&mut MemoryTx {
            store: &mut candidate,
        })?;
        if self.fail_commit.load(std::sync::atomic::Ordering::Relaxed) {
            return Err(Error::unavailable(
                "test transaction",
                "simulated commit failure",
            ));
        }
        *self.lock() = candidate;
        Ok(())
    }
}
