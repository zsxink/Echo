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
use crate::domain::entities::{
    LibraryRoot, LyricsCandidate, LyricsSource, MediaDiagnostic, PlaylistMember, Song,
    SongAvailability,
};
use crate::domain::ids::*;
use crate::domain::state::scan::{ScanProgress, ScanState};
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
    fn all_in_root(&self, root: LibraryRootId) -> Result<Vec<Song>, Error> {
        Ok(self
            .songs
            .lock()
            .unwrap()
            .values()
            .filter(|s| s.root() == root)
            .cloned()
            .collect())
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
            let mut replacement = LibraryRoot::new(
                r.id(),
                r.absolute_path().to_path_buf(),
                false,
                r.observed_write_capable(),
            );
            replacement.set_write_safety_locked(r.write_safety_locked());
            *r = replacement;
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
    fn set_write_safety_locked(&self, id: LibraryRootId, locked: bool) -> Result<(), Error> {
        if let Some(r) = self.roots.lock().unwrap().get_mut(&id) {
            r.set_write_safety_locked(locked);
        }
        Ok(())
    }
}

/// In-memory playlist store (members + names).
#[derive(Clone, Debug, Default)]
pub struct MemoryPlaylistRepository {
    names: Shared<BTreeMap<PlaylistId, (LibraryRootId, String)>>,
    covers: Shared<BTreeMap<PlaylistId, String>>,
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
    fn name(&self, id: PlaylistId) -> Result<Option<String>, Error> {
        Ok(self
            .names
            .lock()
            .unwrap()
            .get(&id)
            .map(|(_, name)| name.clone()))
    }
    fn cover_key(&self, id: PlaylistId) -> Result<Option<String>, Error> {
        Ok(self.covers.lock().unwrap().get(&id).cloned())
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
    fn set_cover_key(&self, id: PlaylistId, key: Option<&str>) -> Result<(), Error> {
        let mut covers = self.covers.lock().unwrap();
        if let Some(key) = key {
            covers.insert(id, key.to_owned());
        } else {
            covers.remove(&id);
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
    /// Operations whose target claims were released (lifecycle mirror).
    released_claims: Shared<Vec<OperationId>>,
    /// Persisted undo deadlines (operation → epoch millis).
    undo_deadlines: Shared<BTreeMap<OperationId, i64>>,
}

impl MemoryOperationJournal {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Operations whose target claims were released (assertion helper).
    #[must_use]
    pub fn released_claims(&self) -> Vec<OperationId> {
        self.released_claims.lock().unwrap().clone()
    }
}

impl OperationJournalRepository for MemoryOperationJournal {
    fn ensure_operation(
        &self,
        _operation: OperationId,
        _root: LibraryRootId,
        _kind: &str,
        _reserved_song: Option<SongId>,
    ) -> Result<(), Error> {
        Ok(())
    }
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
    fn release_claims(&self, operation: OperationId) -> Result<(), Error> {
        self.released_claims.lock().unwrap().push(operation);
        Ok(())
    }

    fn incomplete_items(
        &self,
        root: LibraryRootId,
    ) -> Result<Vec<(OperationId, String, OperationItem)>, Error> {
        // This minimal per-port fake stores items without a root or envelope
        // kind; recovery tests that need root/kind filtering use `MemoryDatabase`
        // (which shares one store like the real SQLite file).
        let _ = root;
        Ok(self
            .items
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, item)| !item.state.is_terminal())
            .map(|((op, _), item)| (*op, "import".to_owned(), item.clone()))
            .collect())
    }

    fn set_undo_deadline(&self, operation: OperationId, deadline_ms: i64) -> Result<(), Error> {
        self.undo_deadlines
            .lock()
            .unwrap()
            .insert(operation, deadline_ms);
        Ok(())
    }

    fn undo_deadline(&self, operation: OperationId) -> Result<Option<i64>, Error> {
        Ok(self.undo_deadlines.lock().unwrap().get(&operation).copied())
    }
}

/// In-memory lyrics-candidate store (task 4.5 reads).
#[derive(Clone, Debug, Default)]
pub struct MemoryLyricsRepository {
    candidates: Shared<BTreeMap<(SongId, LyricsSource), LyricsCandidate>>,
}

impl MemoryLyricsRepository {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl LyricsRepository for MemoryLyricsRepository {
    fn candidates(&self, song: SongId) -> Result<Vec<LyricsCandidate>, Error> {
        Ok(self
            .candidates
            .lock()
            .unwrap()
            .iter()
            .filter(|((s, _), _)| *s == song)
            .map(|(_, c)| c.clone())
            .collect())
    }
}

/// In-memory cover-reference store (task 4.6 reads + GC keep-set).
#[derive(Clone, Debug, Default)]
pub struct MemoryCoverRepository {
    covers: Shared<BTreeMap<(LibraryRootId, SongId), CoverAssetRef>>,
}

impl MemoryCoverRepository {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
    /// Test setup: attach a cover reference directly.
    pub fn seed(&self, root: LibraryRootId, song: SongId, cover: CoverAssetRef) {
        self.covers.lock().unwrap().insert((root, song), cover);
    }
}

impl CoverRepository for MemoryCoverRepository {
    fn cover_of(&self, song: SongId) -> Result<Option<CoverAssetRef>, Error> {
        Ok(self
            .covers
            .lock()
            .unwrap()
            .iter()
            .find(|((_, s), _)| *s == song)
            .map(|(_, cover)| cover.clone()))
    }
    fn referenced_asset_keys(&self, root: LibraryRootId) -> Result<Vec<String>, Error> {
        Ok(self
            .covers
            .lock()
            .unwrap()
            .iter()
            .filter(|((r, _), _)| *r == root)
            .map(|(_, cover)| cover.asset_key.clone())
            .collect())
    }
}

/// In-memory scan-run progress/issue store (task 4.10).
#[derive(Clone, Debug, Default)]
pub struct MemoryScanRunRepository {
    runs: Shared<BTreeMap<(LibraryRootId, u64), ScanRunRow>>,
    issues: Shared<Vec<(LibraryRootId, u64, MediaDiagnostic)>>,
}

/// The in-memory mirror of a `scan_runs` row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScanRunRow {
    pub state: ScanState,
    pub progress: ScanProgress,
    pub finished: bool,
    pub updates: u64,
}

impl MemoryScanRunRepository {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
    /// Snapshot of one run row (assertion helper).
    #[must_use]
    pub fn run(&self, root: LibraryRootId, generation: u64) -> Option<ScanRunRow> {
        self.runs.lock().unwrap().get(&(root, generation)).cloned()
    }
    /// All issues recorded for one run (assertion helper).
    #[must_use]
    pub fn issues_of(&self, root: LibraryRootId, generation: u64) -> Vec<MediaDiagnostic> {
        self.issues
            .lock()
            .unwrap()
            .iter()
            .filter(|(r, g, _)| *r == root && *g == generation)
            .map(|(_, _, issue)| issue.clone())
            .collect()
    }
}

impl ScanRunRepository for MemoryScanRunRepository {
    fn begin_run(&self, root: LibraryRootId, generation: u64) -> Result<(), Error> {
        let mut runs = self.runs.lock().unwrap();
        if runs.contains_key(&(root, generation)) {
            return Err(Error::conflict("scan run already exists"));
        }
        runs.insert(
            (root, generation),
            ScanRunRow {
                state: ScanState::Queued,
                progress: ScanProgress::default(),
                finished: false,
                updates: 0,
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
        let mut runs = self.runs.lock().unwrap();
        let row = runs
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
        self.issues
            .lock()
            .unwrap()
            .push((root, generation, issue.clone()));
        Ok(())
    }
    fn finish_run(
        &self,
        root: LibraryRootId,
        generation: u64,
        state: ScanState,
        progress: &ScanProgress,
    ) -> Result<(), Error> {
        let mut runs = self.runs.lock().unwrap();
        let row = runs
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
            .runs
            .lock()
            .unwrap()
            .keys()
            .filter(|(r, _)| *r == root)
            .map(|(_, g)| *g)
            .max())
    }
}

/// In-memory runtime key/value store (`root_epoch` persistence read side).
#[derive(Clone, Debug, Default)]
pub struct MemoryRuntimeState {
    values: Shared<BTreeMap<String, String>>,
}

impl MemoryRuntimeState {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl RuntimeStateStore for MemoryRuntimeState {
    fn load(&self, key: &str) -> Result<Option<String>, Error> {
        Ok(self.values.lock().unwrap().get(key).cloned())
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
