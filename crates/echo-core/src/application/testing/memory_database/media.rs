//! `LyricsRepository` / `CoverRepository` / `ScanRunRepository` views.

use super::*;

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
