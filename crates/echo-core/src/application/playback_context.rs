//! Resolve a library view or playlist into one ordered playback context.
//!
//! This application use case owns the view-specific paging, filtering and
//! ordering rules. Platform layers only provide the declarative request and
//! hand the resulting identifiers to their player adapters.

use crate::application::catalog::CatalogQuery;
use crate::application::ports::CatalogQueryRepository;
use crate::domain::catalog::{PlaybackContextRequest, PlaybackContextResolved, ViewRef};
use crate::domain::entities::Song;
use crate::domain::ids::SongId;
use crate::error::Error;

const PAGE_SIZE: usize = 500;

/// Resolve typed playback requests through the catalog read-model port.
pub struct ResolvePlaybackContext<'a> {
    catalog: &'a dyn CatalogQueryRepository,
}

impl<'a> ResolvePlaybackContext<'a> {
    #[must_use]
    pub const fn new(catalog: &'a dyn CatalogQueryRepository) -> Self {
        Self { catalog }
    }

    /// Resolve the complete ordered context and validate its selected entry.
    ///
    /// # Errors
    ///
    /// Propagates catalog failures and returns `Conflict` when the selected
    /// song no longer belongs to the requested view.
    pub fn run(&self, request: &PlaybackContextRequest) -> Result<PlaybackContextResolved, Error> {
        let songs = match request.view {
            ViewRef::Recent => self.resolve_recent(request)?,
            ViewRef::Playlist { id } if request.query.trim().is_empty() => {
                self.resolve_playlist(id)?
            }
            ViewRef::AllSongs | ViewRef::Favorites | ViewRef::Playlist { .. } => {
                self.resolve_library(request)?
            }
        };
        let conflict = match request.view {
            ViewRef::Playlist { .. } => "selected song is no longer a member of the playlist",
            ViewRef::AllSongs | ViewRef::Recent | ViewRef::Favorites => {
                "selected song is no longer in the active library view"
            }
        };
        resolve_selected(songs, request.selected, conflict)
    }

    fn resolve_recent(&self, request: &PlaybackContextRequest) -> Result<Vec<SongId>, Error> {
        let needle = request.query.trim().to_lowercase();
        Ok(CatalogQuery::new(self.catalog)
            .recent_100()?
            .into_iter()
            .filter(|song| matches_recent_query(song, &needle))
            .map(|song| song.id())
            .collect())
    }

    fn resolve_library(&self, request: &PlaybackContextRequest) -> Result<Vec<SongId>, Error> {
        let catalog = CatalogQuery::new(self.catalog);
        let mut cursor = request.cursor.clone();
        let mut ids = Vec::new();
        loop {
            let page = match request.view {
                ViewRef::AllSongs if request.query.trim().is_empty() => {
                    catalog.all_songs(request.sort, cursor.as_ref(), PAGE_SIZE)?
                }
                ViewRef::AllSongs => catalog.search(
                    &request.query,
                    false,
                    None,
                    request.sort,
                    cursor.as_ref(),
                    PAGE_SIZE,
                )?,
                ViewRef::Favorites if request.query.trim().is_empty() => {
                    catalog.favorites(request.sort, cursor.as_ref(), PAGE_SIZE)?
                }
                ViewRef::Favorites => catalog.search(
                    &request.query,
                    true,
                    None,
                    request.sort,
                    cursor.as_ref(),
                    PAGE_SIZE,
                )?,
                ViewRef::Playlist { .. } if request.query.trim().is_empty() => {
                    // The full playlist is read eagerly by `resolve_playlist`
                    // (newest-first to mirror the visible order); this search
                    // path is only reached for playlist + query, matching the
                    // spec "歌单内搜索" filtered playback queue.
                    unreachable!("playlist without query goes through resolve_playlist")
                }
                ViewRef::Playlist { id } => catalog.search(
                    &request.query,
                    false,
                    Some(id),
                    request.sort,
                    cursor.as_ref(),
                    PAGE_SIZE,
                )?,
                ViewRef::Recent => unreachable!("library view only"),
            };
            ids.extend(page.items.into_iter().map(|song| song.id()));
            if page.is_last {
                break;
            }
            let Some(next) = page.next_cursor else {
                break;
            };
            cursor = Some(next);
        }
        Ok(ids)
    }

    fn resolve_playlist(
        &self,
        playlist: crate::domain::ids::PlaylistId,
    ) -> Result<Vec<SongId>, Error> {
        let mut songs = CatalogQuery::new(self.catalog).playlist(playlist)?;
        // Repository position is append order; the user-facing playlist is
        // newest-first, and playback must mirror that visible order.
        songs.reverse();
        Ok(songs.into_iter().map(|song| song.id()).collect())
    }
}

fn matches_recent_query(song: &Song, needle: &str) -> bool {
    needle.is_empty()
        || [song.title(), song.artist(), song.album()]
            .into_iter()
            .flatten()
            .any(|field| field.to_lowercase().contains(needle))
}

fn resolve_selected(
    songs: Vec<SongId>,
    selected: SongId,
    conflict: &str,
) -> Result<PlaybackContextResolved, Error> {
    let Some(selected_index) = songs.iter().position(|song| *song == selected) else {
        return Err(Error::conflict(conflict));
    };
    Ok(PlaybackContextResolved {
        songs,
        selected_index,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::{
        CatalogQueryRepository, LibraryRepository, PlaylistRepository, SongRepository,
    };
    use crate::application::testing::memory_database::MemoryDatabase;
    use crate::domain::catalog::{
        CatalogCounts, OpaqueCursor, Paged, SongSort, SongSortField, SortDirection, ViewRef,
    };
    use crate::domain::entities::LibraryRoot;
    use crate::domain::ids::{LibraryRootId, PlaylistId, RelativeMediaPath, Revision};

    fn sort() -> SongSort {
        SongSort {
            field: SongSortField::AddedAt,
            direction: SortDirection::Desc,
        }
    }

    fn song(root: LibraryRootId, path: &str, title: &str) -> Song {
        let mut song = Song::new(
            SongId::new(),
            root,
            RelativeMediaPath::new(path).expect("safe test path"),
            Revision::INITIAL,
        );
        song.apply_metadata(
            Some(title.to_owned()),
            Some("Artist".to_owned()),
            None,
            None,
        );
        song
    }

    fn catalog() -> (MemoryDatabase, LibraryRootId) {
        let database = MemoryDatabase::new();
        let root = LibraryRootId::new();
        LibraryRepository::upsert(
            &database,
            &LibraryRoot::new(root, "/library".into(), true, true),
        )
        .expect("active root");
        (database, root)
    }

    struct PagedCatalog {
        all_pages: std::sync::Mutex<std::collections::VecDeque<Paged<Song>>>,
    }

    impl PagedCatalog {
        fn with_all_pages(pages: impl IntoIterator<Item = Paged<Song>>) -> Self {
            Self {
                all_pages: std::sync::Mutex::new(pages.into_iter().collect()),
            }
        }
    }

    impl CatalogQueryRepository for PagedCatalog {
        fn all_songs(
            &self,
            _sort: SongSort,
            _cursor: Option<&OpaqueCursor>,
            _limit: usize,
        ) -> Result<Paged<Song>, Error> {
            self.all_pages
                .lock()
                .expect("page fixture lock")
                .pop_front()
                .ok_or_else(|| Error::unavailable("page fixture", "no scripted page"))
        }

        fn favorites(
            &self,
            _sort: SongSort,
            _cursor: Option<&OpaqueCursor>,
            _limit: usize,
        ) -> Result<Paged<Song>, Error> {
            Err(Error::unavailable("page fixture", "favorites not scripted"))
        }

        fn recent_100(&self) -> Result<Vec<Song>, Error> {
            Err(Error::unavailable("page fixture", "recent not scripted"))
        }

        fn playlist_songs(&self, _playlist: PlaylistId) -> Result<Vec<Song>, Error> {
            Err(Error::unavailable("page fixture", "playlist not scripted"))
        }

        fn counts(&self) -> Result<CatalogCounts, Error> {
            Err(Error::unavailable("page fixture", "counts not scripted"))
        }

        fn search(
            &self,
            _query: &str,
            _in_favorites: bool,
            _playlist: Option<PlaylistId>,
            _sort: SongSort,
            _cursor: Option<&OpaqueCursor>,
            _limit: usize,
        ) -> Result<Paged<Song>, Error> {
            Err(Error::unavailable("page fixture", "search not scripted"))
        }
    }

    #[test]
    fn resolves_all_pages_and_validates_selected_song() {
        let root = LibraryRootId::new();
        let mut selected = None;
        let mut songs = Vec::new();
        for number in 0..501 {
            let entry = song(root, &format!("{number}.flac"), &format!("title {number}"));
            if number == 500 {
                selected = Some(entry.id());
            }
            songs.push(entry);
        }
        let selected = selected.expect("selected song");
        let first_page = songs.drain(..500).collect();
        let catalog = PagedCatalog::with_all_pages([
            Paged::new(first_page, Some(OpaqueCursor::start()), false),
            Paged::new(songs, None, true),
        ]);
        let resolved = ResolvePlaybackContext::new(&catalog)
            .run(&PlaybackContextRequest::new(
                ViewRef::AllSongs,
                sort(),
                selected,
            ))
            .expect("all pages resolve");
        assert_eq!(resolved.songs.len(), 501);
        assert_eq!(resolved.songs[resolved.selected_index], selected);
    }

    #[test]
    fn recent_filters_case_insensitively_and_rejects_absent_selection() {
        let (database, root) = catalog();
        let matched = song(root, "matched.flac", "MiXeD Title");
        let other = song(root, "other.flac", "Other");
        SongRepository::upsert(&database, &matched).expect("seed matched");
        SongRepository::upsert(&database, &other).expect("seed other");
        let request =
            PlaybackContextRequest::new(ViewRef::Recent, sort(), matched.id()).with_query("mixed");
        let resolved = ResolvePlaybackContext::new(&database)
            .run(&request)
            .expect("recent query resolves");
        assert_eq!(resolved.songs, vec![matched.id()]);

        let err = ResolvePlaybackContext::new(&database)
            .run(
                &PlaybackContextRequest::new(ViewRef::Recent, sort(), other.id())
                    .with_query("mixed"),
            )
            .expect_err("filtered-out selection conflicts");
        assert!(matches!(err, Error::Conflict { .. }));
    }

    #[test]
    fn playlist_uses_newest_member_first() {
        let (database, root) = catalog();
        let first = song(root, "first.flac", "First");
        let second = song(root, "second.flac", "Second");
        SongRepository::upsert(&database, &first).expect("seed first");
        SongRepository::upsert(&database, &second).expect("seed second");
        let playlist = PlaylistId::new();
        PlaylistRepository::create(&database, playlist, root, "Queue").expect("create playlist");
        PlaylistRepository::add_member(&database, playlist, first.id(), 0).expect("add first");
        PlaylistRepository::add_member(&database, playlist, second.id(), 1).expect("add second");

        let resolved = ResolvePlaybackContext::new(&database)
            .run(&PlaybackContextRequest::new(
                ViewRef::Playlist { id: playlist },
                sort(),
                second.id(),
            ))
            .expect("playlist resolves");
        assert_eq!(resolved.songs, vec![second.id(), first.id()]);
        assert_eq!(resolved.selected_index, 0);
    }

    #[test]
    fn unknown_library_view_is_rejected_in_core() {
        let err = PlaybackContextRequest::library_view("unknown", "", sort(), SongId::new())
            .expect_err("unknown view is invalid");
        assert!(matches!(err, Error::Validation { .. }));
    }

    #[test]
    fn playlist_with_query_limits_playback_queue_to_filtered_members() {
        let (database, root) = catalog();
        let matched = song(root, "matched.flac", "Lovely Track");
        let other = song(root, "other.flac", "Other");
        let also_matched = song(root, "also.flac", "Lovely 二");
        for entry in [&matched, &other, &also_matched] {
            SongRepository::upsert(&database, entry).expect("seed song");
        }
        let playlist = PlaylistId::new();
        PlaylistRepository::create(&database, playlist, root, "搜索歌单").expect("create playlist");
        // 成员按追加顺序入库；用户可见顺序是 newest-first。
        for (position, entry) in [&matched, &other, &also_matched].into_iter().enumerate() {
            PlaylistRepository::add_member(&database, playlist, entry.id(), position as u64)
                .expect("add member");
        }

        let resolved = ResolvePlaybackContext::new(&database)
            .run(
                &PlaybackContextRequest::new(
                    ViewRef::Playlist { id: playlist },
                    sort(),
                    matched.id(),
                )
                .with_query("lovely"),
            )
            .expect("playlist search resolves");
        let mut expect = vec![matched.id(), also_matched.id()];
        expect.sort();
        let mut actual = resolved.songs.clone();
        actual.sort();
        assert_eq!(actual, expect, "queue is exactly the filtered members");
        assert!(
            !resolved.songs.contains(&other.id()),
            "unfiltered member is not in the playback queue"
        );
    }

    #[test]
    fn playlist_with_query_selection_outside_filter_conflicts() {
        let (database, root) = catalog();
        let matched = song(root, "matched.flac", "Lovely");
        let other = song(root, "other.flac", "Other");
        for entry in [&matched, &other] {
            SongRepository::upsert(&database, entry).expect("seed song");
        }
        let playlist = PlaylistId::new();
        PlaylistRepository::create(&database, playlist, root, "筛选队列").expect("create playlist");
        PlaylistRepository::add_member(&database, playlist, matched.id(), 0).expect("add first");
        PlaylistRepository::add_member(&database, playlist, other.id(), 1).expect("add second");

        let err = ResolvePlaybackContext::new(&database)
            .run(
                &PlaybackContextRequest::new(
                    ViewRef::Playlist { id: playlist },
                    sort(),
                    other.id(),
                )
                .with_query("lovely"),
            )
            .expect_err("selection outside the filtered queue conflicts");
        assert!(matches!(err, Error::Conflict { .. }));
    }
}
