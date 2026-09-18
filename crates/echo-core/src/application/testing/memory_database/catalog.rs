//! `CatalogQueryRepository` — the paged read model (all songs, favorites,
//! recent, counts, playlist songs, search). Every query funnels through the
//! shared `mem_catalog` so the playlist pin behaves identically everywhere.

use super::*;

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
        songs.truncate(RECENT_VIEW_LIMIT);
        Ok(songs)
    }

    fn counts(&self) -> Result<CatalogCounts, Error> {
        let root = self.active_root()?.map(|record| record.id());
        let Some(root) = root else {
            // Zero would assert "the library is empty", which is a different
            // fact from "there is no library to count".
            return Err(Error::unavailable("library", "no active root"));
        };
        let store = self.lock();
        let mut available = 0usize;
        let mut favorites = 0usize;
        for song in store.songs.values() {
            if song.root() != root || song.availability() != SongAvailability::Available {
                continue;
            }
            available += 1;
            if song.favorite() {
                favorites += 1;
            }
        }
        Ok(CatalogCounts::new(available, favorites))
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
