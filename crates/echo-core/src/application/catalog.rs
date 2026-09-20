//! Catalog query use case (task 6.1, spec "资料库视图" / "全部歌曲排序").
//!
//! A thin, testable wrapper over [`CatalogQueryRepository`] that exposes the
//! four default library views — 全部歌曲, 最近添加 (recent 100), 喜欢的音乐,
//! and 歌单 — with their defining semantics. `echo-core` stays the sole
//! authority for which songs a view contains (active root only, pending-delete
//! hidden, deterministic ordering); the desktop layer renders the results.
//!
//! Views never reach into another root, never surface a pending-delete song,
//! and keep a deterministic order, so the UI can page through them with stable
//! keyset cursors and rely on identical results across repeat requests.

use crate::application::ports::CatalogQueryRepository;
use crate::domain::catalog::{CatalogCounts, OpaqueCursor, Paged, SongSort};
use crate::domain::entities::Song;
use crate::domain::ids::PlaylistId;
use crate::error::Error;

/// The catalog read-model the UI consumes (task 6.1).
pub struct CatalogQuery<'a> {
    repo: &'a dyn CatalogQueryRepository,
}

impl<'a> CatalogQuery<'a> {
    #[must_use]
    pub const fn new(repo: &'a dyn CatalogQueryRepository) -> Self {
        Self { repo }
    }

    /// 全部歌曲: the active root's available songs, keyset-paginated.
    ///
    /// # Errors
    ///
    /// `Unavailable` when there is no active root; a stale cursor is
    /// `Conflict` (restart pagination); the repo propagates storage errors.
    pub fn all_songs(
        &self,
        sort: SongSort,
        cursor: Option<&OpaqueCursor>,
        limit: usize,
    ) -> Result<Paged<Song>, Error> {
        self.repo.all_songs(sort, cursor, limit)
    }

    /// 喜欢的音乐: the active root's favorited, available songs.
    ///
    /// # Errors
    ///
    /// Same surface as [`Self::all_songs`].
    pub fn favorites(
        &self,
        sort: SongSort,
        cursor: Option<&OpaqueCursor>,
        limit: usize,
    ) -> Result<Paged<Song>, Error> {
        self.repo.favorites(sort, cursor, limit)
    }

    /// 资料库搜索: case-insensitive full-query contains overlay over title,
    /// artist and album of the active root. `in_favorites` narrows to the
    /// favorites view; an empty `query` restores the full underlying view.
    ///
    /// # Errors
    ///
    /// `Unavailable` when there is no active root; a stale cursor is
    /// `Conflict` (restart pagination); storage errors propagate.
    pub fn search(
        &self,
        query: &str,
        in_favorites: bool,
        sort: SongSort,
        cursor: Option<&OpaqueCursor>,
        limit: usize,
    ) -> Result<Paged<Song>, Error> {
        self.repo.search(query, in_favorites, sort, cursor, limit)
    }

    /// 最近添加: the active root's most recently added available songs, at
    /// most 100.
    ///
    /// # Errors
    ///
    /// `Unavailable` when there is no active root; storage errors propagate.
    pub fn recent_100(&self) -> Result<Vec<Song>, Error> {
        self.repo.recent_100()
    }

    /// The newest available song in the active root, using the same stable
    /// ordering as the 最近添加 view. The recent view is already ordered and
    /// capped after the newest rows, so its first item is the exact candidate
    /// needed for initial playback selection.
    ///
    /// # Errors
    ///
    /// `Unavailable` when there is no active root; storage errors propagate.
    pub fn latest_available_song(&self) -> Result<Option<crate::domain::ids::SongId>, Error> {
        Ok(self.recent_100()?.into_iter().next().map(|song| song.id()))
    }

    /// 资料库导航计数: one total per library view (task: 侧边栏在打开视图前
    /// 就显示其歌曲数).
    ///
    /// The counts are a *view-membership* question, so Core stays the sole
    /// authority here — the desktop layer must never derive a total from rows
    /// it has already paged through.
    ///
    /// # Errors
    ///
    /// `Unavailable` when there is no active root; storage errors propagate.
    pub fn counts(&self) -> Result<CatalogCounts, Error> {
        self.repo.counts()
    }

    /// 歌单: one playlist's song rows ordered by member position (available
    /// and externally-missing members shown, pending-delete hidden).
    ///
    /// # Errors
    ///
    /// `Unavailable` when there is no active root; storage errors propagate.
    pub fn playlist(&self, playlist: PlaylistId) -> Result<Vec<Song>, Error> {
        self.repo.playlist_songs(playlist)
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;
    use crate::application::ports::{LibraryRepository, PlaylistRepository, SongRepository};
    use crate::application::testing::memory_database::MemoryDatabase;
    use crate::domain::catalog::{SongSortField, SortDirection, RECENT_VIEW_LIMIT};
    use crate::domain::entities::{LibraryRoot, SongAvailability};
    use crate::domain::ids::{LibraryRootId, PlaylistId, RelativeMediaPath, Revision, SongId};
    use crate::error::Error;

    fn seed_memory(db: &MemoryDatabase, root: LibraryRootId) {
        let mut song = Song::with_added_at(
            SongId::new(),
            root,
            RelativeMediaPath::new("a.flac").expect("path"),
            Revision::INITIAL,
            10,
        );
        song.apply_metadata(
            Some("alpha".to_owned()),
            Some("阿".to_owned()),
            Some("album".to_owned()),
            Some(Duration::from_secs(1)),
        );
        SongRepository::upsert(db, &song).expect("seed a");

        let mut song = Song::with_added_at(
            SongId::new(),
            root,
            RelativeMediaPath::new("b.flac").expect("path"),
            Revision::INITIAL,
            20,
        );
        song.set_favorite(true);
        song.apply_metadata(
            Some("beta".to_owned()),
            Some("伯".to_owned()),
            Some("album".to_owned()),
            Some(Duration::from_secs(2)),
        );
        SongRepository::upsert(db, &song).expect("seed b");

        let mut pending = Song::with_added_at(
            SongId::new(),
            root,
            RelativeMediaPath::new("gone.flac").expect("path"),
            Revision::INITIAL,
            30,
        );
        pending.begin_pending_delete();
        pending.apply_metadata(
            Some("gone".to_owned()),
            Some("歌".to_owned()),
            Some("album".to_owned()),
            Some(Duration::from_secs(3)),
        );
        SongRepository::upsert(db, &pending).expect("seed pending");
    }

    /// The sidebar prints a count for a view the user has never opened, so
    /// `counts()` must be answerable without paging any view first. It is a
    /// *membership* total, not a "rows loaded so far" figure.
    #[test]
    fn catalog_counts_are_available_before_any_view_is_paged() {
        let db = MemoryDatabase::new();
        let root = LibraryRootId::new();
        LibraryRepository::upsert(&db, &LibraryRoot::new(root, ".".into(), true, true))
            .expect("active root");
        seed_memory(&db, root);
        let query = CatalogQuery::new(&db);

        let counts = query.counts().expect("counts");
        assert_eq!(counts.all, 2, "pending-delete is invisible to 全部歌曲");
        assert_eq!(counts.favorites, 1, "only the favorited song counts");
        assert_eq!(counts.recent, 2, "under the ceiling, recent == all");

        // The counts must agree with what the views actually render.
        let all = query
            .all_songs(SongSort::default(), None, 100)
            .expect("all");
        let favs = query
            .favorites(SongSort::default(), None, 100)
            .expect("favorites");
        assert_eq!(counts.all, all.items.len(), "count matches 全部歌曲 rows");
        assert_eq!(
            counts.favorites,
            favs.items.len(),
            "count matches 喜欢的音乐 rows"
        );

        // An inactive root's songs never inflate any count.
        let other = LibraryRootId::new();
        LibraryRepository::upsert(&db, &LibraryRoot::new(other, "other".into(), false, true))
            .expect("inactive root");
        SongRepository::upsert(
            &db,
            &Song::new(
                SongId::new(),
                other,
                RelativeMediaPath::new("foreign.flac").expect("path"),
                Revision::INITIAL,
            ),
        )
        .expect("foreign");
        let after = query.counts().expect("counts again");
        assert_eq!(after.all, 2, "inactive-root songs are never counted");
        assert_eq!(after.favorites, 1);
    }

    /// 最近添加 renders at most `RECENT_VIEW_LIMIT` rows; printing the whole
    /// library next to it would be a different kind of wrong.
    #[test]
    fn catalog_counts_cap_recent_at_the_view_ceiling() {
        let db = MemoryDatabase::new();
        let root = LibraryRootId::new();
        LibraryRepository::upsert(&db, &LibraryRoot::new(root, ".".into(), true, true))
            .expect("active root");
        let total = RECENT_VIEW_LIMIT + 20;
        for index in 0..total {
            let mut song = Song::with_added_at(
                SongId::new(),
                root,
                RelativeMediaPath::new(&format!("song-{index}.flac")).expect("path"),
                Revision::INITIAL,
                index as u64,
            );
            song.set_favorite(index % 4 == 0);
            SongRepository::upsert(&db, &song).expect("seed");
        }
        let counts = CatalogQuery::new(&db).counts().expect("counts");

        assert_eq!(counts.all, total);
        assert_eq!(counts.recent, RECENT_VIEW_LIMIT, "recent is capped");
        assert_eq!(
            counts.favorites,
            (0..total).filter(|i| i % 4 == 0).count(),
            "every fourth song is a favorite"
        );
    }

    /// No active root is not "an empty library" — a count of zero would be a
    /// fact the app does not have, and the UI would print "0" next to a view it
    /// cannot open.
    #[test]
    fn catalog_counts_are_unavailable_without_an_active_root() {
        let db = MemoryDatabase::new();
        let err = CatalogQuery::new(&db)
            .counts()
            .expect_err("no root to count");
        assert!(
            matches!(err, Error::Unavailable { .. }),
            "expected Unavailable, got {err:?}"
        );
    }

    #[test]
    fn catalog_views_over_memory_repo_agree_with_the_port_contract() {
        let db = MemoryDatabase::new();
        let root = LibraryRootId::new();
        LibraryRepository::upsert(&db, &LibraryRoot::new(root, ".".into(), true, true))
            .expect("active root");
        seed_memory(&db, root);
        let query = CatalogQuery::new(&db);

        // 全部歌曲: available only, both candidates visible, pending hidden.
        let all = query
            .all_songs(
                SongSort {
                    field: SongSortField::AddedAt,
                    direction: SortDirection::Asc,
                },
                None,
                100,
            )
            .expect("all songs");
        assert_eq!(all.items.len(), 2, "pending-delete hidden");
        let titles: Vec<_> = all.items.iter().map(|s| s.title().unwrap_or("")).collect();
        assert_eq!(titles, ["alpha", "beta"], "stable added_at order");

        // 喜欢的音乐: only the favorited candidate.
        let favs = query
            .favorites(
                SongSort {
                    field: SongSortField::Title,
                    direction: SortDirection::Asc,
                },
                None,
                100,
            )
            .expect("favorites");
        assert_eq!(favs.items.len(), 1);
        assert_eq!(favs.items[0].title().unwrap_or(""), "beta");

        // 最近添加: newest available first, pending hidden.
        let recent = query.recent_100().expect("recent");
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].title().unwrap_or(""), "beta", "newest first");
        assert_eq!(recent[1].title().unwrap_or(""), "alpha");

        // A second root's songs never leak into the views.
        let other = LibraryRootId::new();
        LibraryRepository::upsert(&db, &LibraryRoot::new(other, "other".into(), false, true))
            .expect("inactive root");
        let mut foreign = Song::new(
            SongId::new(),
            other,
            RelativeMediaPath::new("foreign.flac").expect("path"),
            Revision::INITIAL,
        );
        foreign.apply_metadata(
            Some("foreign".to_owned()),
            Some("外".to_owned()),
            Some("album".to_owned()),
            Some(Duration::from_secs(4)),
        );
        SongRepository::upsert(&db, &foreign).expect("foreign");
        assert_eq!(
            query
                .all_songs(SongSort::default(), None, 100)
                .expect("all again")
                .items
                .len(),
            2,
            "inactive-root songs are never visible"
        );
        // Playlist: available + missing shown, pending-delete hidden.
        let playlist = PlaylistId::new();
        let mut missing = Song::new(
            SongId::new(),
            root,
            RelativeMediaPath::new("missing.flac").expect("path"),
            Revision::INITIAL,
        );
        missing.mark_missing();
        missing.apply_metadata(
            Some("missing".to_owned()),
            Some("没".to_owned()),
            Some("album".to_owned()),
            Some(Duration::from_secs(5)),
        );
        SongRepository::upsert(&db, &missing).expect("missing");
        // The pending-delete song from the seed is hidden from every view.
        let pending: Vec<Song> = db
            .songs()
            .into_iter()
            .filter(|s| s.availability() == SongAvailability::PendingDelete)
            .collect();
        assert_eq!(pending.len(), 1, "seeding produced one pending song");
        let pending_id = pending[0].id();

        PlaylistRepository::create(&db, playlist, root, "playlist").expect("create");
        for (i, id) in [all.items[0].id(), missing.id(), pending_id]
            .into_iter()
            .enumerate()
        {
            PlaylistRepository::add_member(&db, playlist, id, i as u64).expect("member");
        }
        let songs = query.playlist(playlist).expect("playlist");
        assert_eq!(songs.len(), 2, "available + missing, pending hidden");
        assert_eq!(songs[0].id(), all.items[0].id(), "position order");
        assert_eq!(songs[1].id(), missing.id());
    }

    #[test]
    fn latest_available_song_uses_recent_order() {
        let db = MemoryDatabase::new();
        let root = LibraryRootId::new();
        LibraryRepository::upsert(&db, &LibraryRoot::new(root, ".".into(), true, true))
            .expect("active root");
        seed_memory(&db, root);
        let query = CatalogQuery::new(&db);
        let recent = query.recent_100().expect("recent");
        assert_eq!(
            query.latest_available_song().expect("latest"),
            Some(recent[0].id())
        );
    }
}
