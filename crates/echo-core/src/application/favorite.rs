//! Favorite mutation with an authoritative committed result (task 6.3,
//! spec "歌曲收藏").
//!
//! Toggling a song's favorite is a single atomic write (the repository's
//! `set_favorite` runs inside one transaction). The use case then re-reads the
//! committed row through the same `SongId` and returns it as the authoritative
//! state, so the library row, the favorites view, the song detail and the
//! now-playing bar all agree on one snapshot.
//!
//! A missing song or one already inside Echo's delete window is not toggleable
//! and surfaces `Unavailable`; nothing is inferred.

use crate::application::ports::SongRepository;
use crate::domain::entities::Song;
use crate::domain::ids::SongId;
use crate::error::Error;

/// The result of a favorite mutation: the song's authoritative committed state
/// plus whether the toggle flipped it on or off.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FavoriteResult {
    /// The committed song snapshot (same `SongId` everywhere downstream).
    pub song: Song,
    /// The new favorite value after the toggle.
    pub favorite: bool,
}

/// Set (or clear) the favorite flag of one song.
pub struct SetFavorite<'a> {
    repository: &'a dyn SongRepository,
}

impl<'a> SetFavorite<'a> {
    #[must_use]
    pub const fn new(repository: &'a dyn SongRepository) -> Self {
        Self { repository }
    }

    /// Toggle `id`'s favorite to `favorite`. The write is a single atomic
    /// transaction; on success the committed row is re-read and returned so
    /// every surface renders one authoritative state.
    ///
    /// # Errors
    ///
    /// `Unavailable` when the song is unknown or inside Echo's delete window
    /// (not visible/toggleable); storage errors propagate.
    pub fn execute(&self, id: SongId, favorite: bool) -> Result<FavoriteResult, Error> {
        self.repository.set_favorite(id, favorite)?;
        let song = self.authoritative(id)?;
        Ok(FavoriteResult {
            song: song.clone(),
            favorite: song.favorite(),
        })
    }

    /// The committed state of `id`. A song that is missing entirely, or one
    /// Echo is deleting (pending-delete), is not a toggleable visible row.
    fn authoritative(&self, id: SongId) -> Result<Song, Error> {
        let song = self
            .repository
            .by_id(id)?
            .ok_or_else(|| Error::unavailable("song", "not in the library"))?;
        if !song.availability().is_visible() {
            return Err(Error::unavailable(
                "song",
                "cannot favourite a song inside the delete window",
            ));
        }
        Ok(song)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::application::catalog::CatalogQuery;
    use crate::application::ports::LibraryRepository;
    use crate::application::testing::memory_database::MemoryDatabase;
    use crate::domain::catalog::SongSort;
    use crate::domain::entities::{LibraryRoot, SongAvailability};
    use crate::domain::ids::{LibraryRootId, RelativeMediaPath, Revision};

    fn seed(db: &MemoryDatabase, root: LibraryRootId) -> Song {
        let mut song = Song::new(
            SongId::new(),
            root,
            RelativeMediaPath::new("晴天.flac").expect("path"),
            Revision::INITIAL,
        );
        song.apply_metadata(
            Some("晴天".to_owned()),
            Some("周杰伦".to_owned()),
            Some("叶惠美".to_owned()),
            Some(Duration::from_secs(240)),
        );
        SongRepository::upsert(db, &song).expect("seed");
        song
    }

    fn catalogue(db: &MemoryDatabase) -> CatalogQuery<'_> {
        CatalogQuery::new(db)
    }

    #[test]
    fn toggle_favorite_returns_the_committed_authoritative_song() {
        let db = MemoryDatabase::new();
        let root = LibraryRootId::new();
        LibraryRepository::upsert(&db, &LibraryRoot::new(root, ".".into(), true, true))
            .expect("active root");
        let song = seed(&db, root);
        let views = catalogue(&db);

        let use_case = SetFavorite::new(&db);
        let on = use_case.execute(song.id(), true).expect("favourite");
        assert!(on.favorite);
        assert_eq!(on.song.id(), song.id());
        assert!(on.song.favorite(), "authoritative row reflects the toggle");

        // The same SongId drives the favorites view: it must now appear.
        let favs = views
            .favorites(SongSort::default(), None, 100)
            .expect("favorites");
        assert_eq!(favs.items.len(), 1);
        assert_eq!(favs.items[0].id(), song.id());

        // Off: the first result is the authoritative cleared state and the
        // favorites view drops it immediately.
        let off = use_case.execute(song.id(), false).expect("unfavourite");
        assert!(!off.favorite);
        assert!(!off.song.favorite());
        let favs_after = views
            .favorites(SongSort::default(), None, 100)
            .expect("favorites after");
        assert!(favs_after.items.is_empty(), "取消收藏后立即从喜欢视图移除");
    }

    #[test]
    fn favourite_of_missing_or_pending_delete_song_is_unavailable() {
        let db = MemoryDatabase::new();
        let root = LibraryRootId::new();
        LibraryRepository::upsert(&db, &LibraryRoot::new(root, ".".into(), true, true))
            .expect("active root");
        // A song that is not in the library at all.
        let ghost = SongId::new();
        assert!(SetFavorite::new(&db).execute(ghost, true).is_err());

        // A song inside Echo's delete window (pending-delete) is not toggleable.
        let mut song = seed(&db, root);
        song.begin_pending_delete();
        SongRepository::set_availability(&db, song.id(), SongAvailability::PendingDelete)
            .expect("pending delete");
        assert!(
            SetFavorite::new(&db).execute(song.id(), true).is_err(),
            "pending-delete songs are not favourite-able"
        );
    }
}
