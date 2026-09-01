//! One shared in-memory backend implementing every repository port *and* the
//! unit of work coherently — the fake counterpart of one SQLite database.
//!
//! The per-port `Memory*` doubles stay available for isolated unit tests, but
//! use-case tests need the same guarantee the real stack has: a transaction
//! commit is visible through every repository view. `MemoryDatabase` clones
//! share one store (like `SqliteDatabase` handles share one file).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::application::ports::*;
use crate::domain::catalog::{OpaqueCursor, Paged, SongSort};
use crate::domain::entities::{
    LibraryRoot, LyricsCandidate, LyricsSource, MediaDiagnostic, PlaylistMember, Song,
    SongAvailability,
};
use crate::domain::ids::*;
use crate::domain::state::scan::{ScanProgress, ScanState};
use crate::domain::text::playlist_name_key;
use crate::error::Error;

/// The shared transactional store.
#[derive(Clone, Debug, Default)]
struct Store {
    songs: BTreeMap<SongId, Song>,
    roots: BTreeMap<LibraryRootId, LibraryRoot>,
    playlists: BTreeMap<PlaylistId, (LibraryRootId, String)>,
    members: BTreeMap<(PlaylistId, SongId), PlaylistMember>,
    operations: BTreeMap<(OperationId, String), OperationItem>,
    envelopes: BTreeMap<OperationId, (LibraryRootId, String, Option<SongId>)>,
    undo_deadlines: BTreeMap<OperationId, i64>,
    released_claims: Vec<OperationId>,
    lyrics: BTreeMap<(SongId, LyricsSource), LyricsCandidate>,
    covers: BTreeMap<SongId, CoverAssetRef>,
    runs: BTreeMap<(LibraryRootId, u64), ScanRunRow>,
    issues: Vec<(LibraryRootId, u64, MediaDiagnostic)>,
    runtime_state: BTreeMap<String, String>,
    fail_root_isolation: bool,
}

/// The in-memory mirror of a `scan_runs` row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScanRunRow {
    pub state: ScanState,
    pub progress: ScanProgress,
    pub finished: bool,
    pub updates: u64,
}

type Shared = Arc<Mutex<Store>>;

/// One fake database. Clone = same store.
#[derive(Clone, Debug, Default)]
pub struct MemoryDatabase {
    store: Shared,
    fail_commit: Arc<std::sync::atomic::AtomicBool>,
}

impl MemoryDatabase {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Store> {
        self.store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Fail the next transaction commit (crash-injection point).
    pub fn set_fail_commit(&self, fail: bool) {
        self.fail_commit
            .store(fail, std::sync::atomic::Ordering::Relaxed);
    }

    /// Fail the root-isolation write inside a transaction (assertion hook for
    /// the trash unknown-outcome atomicity boundary).
    pub fn set_fail_root_isolation(&self, fail: bool) {
        self.lock().fail_root_isolation = fail;
    }

    /// All songs (assertion helper).
    #[must_use]
    pub fn songs(&self) -> Vec<Song> {
        self.lock().songs.values().cloned().collect()
    }

    /// One run row (assertion helper).
    #[must_use]
    pub fn run(&self, root: LibraryRootId, generation: u64) -> Option<ScanRunRow> {
        self.lock().runs.get(&(root, generation)).cloned()
    }

    /// Issues of one run (assertion helper).
    #[must_use]
    pub fn issues_of(&self, root: LibraryRootId, generation: u64) -> Vec<MediaDiagnostic> {
        self.lock()
            .issues
            .iter()
            .filter(|(r, g, _)| *r == root && *g == generation)
            .map(|(_, _, issue)| issue.clone())
            .collect()
    }

    /// Lyrics candidates of one song (assertion helper).
    #[must_use]
    pub fn lyrics_of(&self, song: SongId) -> Vec<LyricsCandidate> {
        self.lock()
            .lyrics
            .iter()
            .filter(|((s, _), _)| *s == song)
            .map(|(_, candidate)| candidate.clone())
            .collect()
    }

    /// Cover reference of one song (assertion helper).
    #[must_use]
    pub fn cover_of(&self, song: SongId) -> Option<CoverAssetRef> {
        self.lock().covers.get(&song).cloned()
    }

    /// Runtime state value (assertion helper).
    #[must_use]
    pub fn runtime_state(&self, key: &str) -> Option<String> {
        self.lock().runtime_state.get(key).cloned()
    }

    /// Operations whose claims were released (assertion helper).
    #[must_use]
    pub fn released_claims(&self) -> Vec<OperationId> {
        self.lock().released_claims.clone()
    }

    /// The journal envelope of one operation (assertion helper: the import
    /// must ensure the envelope before its first item write).
    #[must_use]
    pub fn envelope_of(&self, operation: OperationId) -> Option<(LibraryRootId, String)> {
        self.lock()
            .envelopes
            .get(&operation)
            .map(|(root, kind, _)| (*root, kind.clone()))
    }

    /// The journal operations reserved for one song (assertion helper for the
    /// external-missing vs Echo-delete distinction: external missing must
    /// create none, an Echo delete creates exactly one delete operation).
    #[must_use]
    pub fn operations_for_song(&self, song: SongId) -> Vec<OperationId> {
        self.lock()
            .envelopes
            .iter()
            .filter(|(_, (_, _, reserved))| *reserved == Some(song))
            .map(|(operation, _)| *operation)
            .collect()
    }
}

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
        self.lock().songs.insert(song.id(), song.clone());
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
}

impl PlaylistRepository for MemoryDatabase {
    fn by_id(&self, id: PlaylistId) -> Result<Option<PlaylistId>, Error> {
        Ok(self.lock().playlists.contains_key(&id).then_some(id))
    }
    fn name(&self, id: PlaylistId) -> Result<Option<String>, Error> {
        Ok(self.lock().playlists.get(&id).map(|(_, name)| name.clone()))
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
            .find(|(_, (r, n))| *r == root && playlist_name_key(n) == key)
            .map(|(id, _)| *id))
    }
    fn list(&self, root: LibraryRootId) -> Result<Vec<PlaylistId>, Error> {
        Ok(self
            .lock()
            .playlists
            .iter()
            .filter(|(_, (r, _))| *r == root)
            .map(|(id, _)| *id)
            .collect())
    }
    fn create(&self, id: PlaylistId, root: LibraryRootId, name: &str) -> Result<(), Error> {
        let mut store = self.lock();
        let key = playlist_name_key(name);
        if store
            .playlists
            .iter()
            .any(|(_, (r, n))| *r == root && playlist_name_key(n) == key)
        {
            return Err(Error::conflict("playlist name already exists"));
        }
        store.playlists.insert(id, (root, name.to_owned()));
        Ok(())
    }
    fn rename(&self, id: PlaylistId, to_normalized_name: &str) -> Result<(), Error> {
        let mut store = self.lock();
        let Some((root, _)) = store.playlists.get(&id).cloned() else {
            return Ok(());
        };
        let key = playlist_name_key(to_normalized_name);
        if store
            .playlists
            .iter()
            .any(|(other, (r, n))| *other != id && *r == root && playlist_name_key(n) == key)
        {
            return Err(Error::conflict("playlist name already exists"));
        }
        if let Some((_, name)) = store.playlists.get_mut(&id) {
            to_normalized_name.clone_into(name);
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
        let mut rows: Vec<_> = self
            .lock()
            .members
            .values()
            .filter(|member| member.playlist() == id)
            .cloned()
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
    fn remove_member(&self, playlist: PlaylistId, song: SongId) -> Result<(), Error> {
        self.lock().members.remove(&(playlist, song));
        Ok(())
    }
}

impl CatalogQueryRepository for MemoryDatabase {
    fn all_songs(
        &self,
        sort: SongSort,
        cursor: Option<&OpaqueCursor>,
        limit: usize,
    ) -> Result<Paged<Song>, Error> {
        self.mem_catalog(false, None, sort, cursor, limit)
    }

    fn favorites(
        &self,
        sort: SongSort,
        cursor: Option<&OpaqueCursor>,
        limit: usize,
    ) -> Result<Paged<Song>, Error> {
        self.mem_catalog(true, None, sort, cursor, limit)
    }

    fn recent_100(&self) -> Result<Vec<Song>, Error> {
        let root = self.active_root()?.map(|record| record.id());
        let mut songs: Vec<Song> = self
            .lock()
            .songs
            .values()
            .filter(|song| {
                root.is_some_and(|r| song.root() == r)
                    && song.availability() == SongAvailability::Available
            })
            .cloned()
            .collect();
        songs.sort_by(|a, b| b.added_at().cmp(&a.added_at()).then(b.id().cmp(&a.id())));
        songs.truncate(100);
        Ok(songs)
    }

    fn playlist_songs(&self, playlist: PlaylistId) -> Result<Vec<Song>, Error> {
        let root = self.active_root()?.map(|record| record.id());
        let store = self.lock();
        let mut rows: Vec<(u64, SongId)> = store
            .members
            .iter()
            .filter(|((playlist_id, _), _)| *playlist_id == playlist)
            .map(|((_, song), member)| (member.position(), *song))
            .collect();
        rows.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
        let mut songs = Vec::with_capacity(rows.len());
        for (_, song_id) in rows {
            if let Some(song) = store.songs.get(&song_id) {
                let in_active_root = root.is_some_and(|r| song.root() == r);
                let hidden = song.availability() == SongAvailability::PendingDelete;
                if in_active_root && !hidden {
                    songs.push(song.clone());
                }
            }
        }
        Ok(songs)
    }

    fn search(
        &self,
        query: &str,
        in_favorites: bool,
        sort: SongSort,
        cursor: Option<&OpaqueCursor>,
        limit: usize,
    ) -> Result<Paged<Song>, Error> {
        self.mem_catalog(in_favorites, Some(query), sort, cursor, limit)
    }
}

impl MemoryDatabase {
    fn mem_catalog(
        &self,
        favorites: bool,
        query: Option<&str>,
        sort: SongSort,
        _cursor: Option<&OpaqueCursor>,
        limit: usize,
    ) -> Result<Paged<Song>, Error> {
        if limit == 0 || limit > 500 {
            return Err(Error::validation(
                crate::error::Subject::Query,
                "page limit",
                "must be 1 through 500",
            ));
        }
        let root = self.active_root()?.map(|record| record.id());
        let normalized_query = query
            .filter(|value| !value.is_empty())
            .map(crate::domain::text::normalized_key);
        let mut songs: Vec<Song> = self
            .lock()
            .songs
            .values()
            .filter(|song| {
                root.is_some_and(|r| song.root() == r)
                    && song.availability() == SongAvailability::Available
                    && (!favorites || song.favorite())
                    && normalized_query.as_deref().map_or(true, |needle| {
                        let haystacks = [
                            song.title().unwrap_or(""),
                            song.artist().unwrap_or(""),
                            song.album().unwrap_or(""),
                        ];
                        haystacks.iter().any(|value| {
                            crate::domain::text::normalized_key(value).contains(needle)
                        })
                    })
            })
            .cloned()
            .collect();
        songs.sort_by(|a, b| sort.compare(a, b));
        let is_last = songs.len() <= limit;
        songs.truncate(limit);
        Ok(Paged::new(songs, None, is_last))
    }
}

impl OperationJournalRepository for MemoryDatabase {
    fn ensure_operation(
        &self,
        operation: OperationId,
        root: LibraryRootId,
        kind: &str,
        reserved_song: Option<SongId>,
    ) -> Result<(), Error> {
        self.lock()
            .envelopes
            .entry(operation)
            .or_insert_with(|| (root, kind.to_owned(), reserved_song));
        Ok(())
    }
    fn item_state(
        &self,
        operation: OperationId,
        item: &str,
    ) -> Result<Option<OperationItem>, Error> {
        Ok(self
            .lock()
            .operations
            .get(&(operation, item.to_owned()))
            .cloned())
    }
    fn upsert_item(&self, operation: OperationId, item: OperationItem) -> Result<(), Error> {
        self.lock()
            .operations
            .insert((operation, item.item_key.clone()), item);
        Ok(())
    }
    fn items(&self, operation: OperationId) -> Result<Vec<OperationItem>, Error> {
        Ok(self
            .lock()
            .operations
            .iter()
            .filter(|((op, _), _)| *op == operation)
            .map(|(_, item)| item.clone())
            .collect())
    }
    fn release_claims(&self, operation: OperationId) -> Result<(), Error> {
        self.lock().released_claims.push(operation);
        Ok(())
    }

    fn set_undo_deadline(&self, operation: OperationId, deadline_ms: i64) -> Result<(), Error> {
        self.lock().undo_deadlines.insert(operation, deadline_ms);
        Ok(())
    }

    fn undo_deadline(&self, operation: OperationId) -> Result<Option<i64>, Error> {
        Ok(self.lock().undo_deadlines.get(&operation).copied())
    }

    fn incomplete_items(
        &self,
        root: LibraryRootId,
    ) -> Result<Vec<(OperationId, String, OperationItem)>, Error> {
        let store = self.lock();
        let mut out = Vec::new();
        for ((operation, _item_key), item) in &store.operations {
            // Only items whose envelope belongs to `root`.
            let Some((op_root, kind, _)) = store.envelopes.get(operation) else {
                continue;
            };
            if *op_root != root {
                continue;
            }
            if item.state.is_terminal() {
                continue;
            }
            out.push((*operation, kind.clone(), item.clone()));
        }
        Ok(out)
    }
}

impl LyricsRepository for MemoryDatabase {
    fn candidates(&self, song: SongId) -> Result<Vec<LyricsCandidate>, Error> {
        Ok(self.lyrics_of(song))
    }
}

impl CoverRepository for MemoryDatabase {
    fn cover_of(&self, song: SongId) -> Result<Option<CoverAssetRef>, Error> {
        Ok(self.cover_of(song))
    }
    fn referenced_asset_keys(&self, root: LibraryRootId) -> Result<Vec<String>, Error> {
        let songs = self.all_in_root(root)?;
        let store = self.lock();
        Ok(songs
            .iter()
            .filter_map(|song| store.covers.get(&song.id()))
            .map(|cover| cover.asset_key.clone())
            .collect())
    }
}

impl ScanRunRepository for MemoryDatabase {
    fn begin_run(&self, root: LibraryRootId, generation: u64) -> Result<(), Error> {
        let mut store = self.lock();
        if store.runs.contains_key(&(root, generation)) {
            return Err(Error::conflict("scan run already exists"));
        }
        store.runs.insert(
            (root, generation),
            ScanRunRow {
                state: ScanState::Queued,
                ..ScanRunRow::default()
            },
        );
        Ok(())
    }
    fn update_progress(
        &self,
        root: LibraryRootId,
        generation: u64,
        progress: &ScanProgress,
    ) -> Result<(), Error> {
        let mut store = self.lock();
        let row = store
            .runs
            .get_mut(&(root, generation))
            .ok_or_else(|| Error::unavailable("scan run", "unknown generation"))?;
        row.progress = *progress;
        row.state = progress.state;
        row.updates += 1;
        Ok(())
    }
    fn record_issue(
        &self,
        root: LibraryRootId,
        generation: u64,
        issue: &MediaDiagnostic,
    ) -> Result<(), Error> {
        self.lock().issues.push((root, generation, issue.clone()));
        Ok(())
    }
    fn finish_run(
        &self,
        root: LibraryRootId,
        generation: u64,
        state: ScanState,
        progress: &ScanProgress,
    ) -> Result<(), Error> {
        let mut store = self.lock();
        let row = store
            .runs
            .get_mut(&(root, generation))
            .ok_or_else(|| Error::unavailable("scan run", "unknown generation"))?;
        row.state = state;
        row.progress = *progress;
        row.finished = true;
        row.updates += 1;
        Ok(())
    }
    fn latest_generation(&self, root: LibraryRootId) -> Result<Option<u64>, Error> {
        Ok(self
            .lock()
            .runs
            .keys()
            .filter(|(r, _)| *r == root)
            .map(|(_, generation)| *generation)
            .max())
    }
}

impl RuntimeStateStore for MemoryDatabase {
    fn load(&self, key: &str) -> Result<Option<String>, Error> {
        Ok(self.lock().runtime_state.get(key).cloned())
    }
}

/// The transaction view: an isolated copy committed on success.
struct MemoryTx<'a> {
    store: &'a mut Store,
}

impl TxAccess for MemoryTx<'_> {
    fn upsert_song(&mut self, song: &Song) -> Result<(), Error> {
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
    fn create_playlist(
        &mut self,
        id: PlaylistId,
        root: LibraryRootId,
        name: &str,
    ) -> Result<(), Error> {
        self.store.playlists.insert(id, (root, name.to_owned()));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memory_database_commits_transactionally_and_is_visible_everywhere() {
        let database = MemoryDatabase::new();
        let root = LibraryRootId::new();
        let song = Song::new(
            SongId::new(),
            root,
            RelativeMediaPath::new("a.flac").unwrap(),
            Revision::INITIAL,
        );
        database
            .with_tx(Box::new(move |tx: &mut dyn TxAccess| {
                tx.upsert_song(&song)?;
                Ok(())
            }))
            .unwrap();
        // Visible through the repository view (shared store).
        assert_eq!(
            SongRepository::all_in_root(&database, root).unwrap().len(),
            1
        );

        database.set_fail_commit(true);
        let second = Song::new(
            SongId::new(),
            root,
            RelativeMediaPath::new("b.flac").unwrap(),
            Revision::INITIAL,
        );
        assert!(database
            .with_tx(Box::new(move |tx: &mut dyn TxAccess| {
                tx.upsert_song(&second)?;
                Ok(())
            }))
            .is_err());
        assert_eq!(database.songs().len(), 1, "failed commit rolled back");
    }
}
