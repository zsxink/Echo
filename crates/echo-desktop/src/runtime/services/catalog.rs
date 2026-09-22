//! Read-only catalog / detail queries (task 7.3).
//!
//! Every method re-reads the committed row through the shared `ScanDeps` and
//! returns the authoritative DTO the UI renders. A read never consults the
//! write gate.

use echo_core::application::catalog::CatalogQuery;
use echo_core::application::detail::{GetSongDetail, GetSongLyrics};
use echo_core::application::playlist::PlaylistMembers;
use echo_core::application::ports::PlaylistRepository;
use echo_core::domain::catalog::{OpaqueCursor, SongSort};
use echo_core::domain::entities::SongAvailability;
use echo_core::domain::ids::{PlaylistId, SongId};
use echo_core::error::Error;

use crate::ipc::dto::{LibraryCountsDto, PagedSongs, PlaylistView, SongDetailView, SongView};

impl super::AppServices {
    /// 全部歌曲, keyset-paginated, mapped to `PagedSongs`.
    ///
    /// # Errors
    ///
    /// `Unavailable` when there is no active root; `Conflict` for a stale
    /// cursor; storage errors propagate.
    pub fn all_songs(
        &self,
        sort: SongSort,
        cursor: Option<&OpaqueCursor>,
        limit: usize,
    ) -> Result<PagedSongs, Error> {
        let page = CatalogQuery::new(self.deps.catalog.as_ref()).all_songs(sort, cursor, limit)?;
        Ok(PagedSongs::from(page))
    }

    /// Search overlay on the active root.
    ///
    /// # Errors
    ///
    /// `Unavailable` when there is no active root; `Conflict` for a stale
    /// cursor; storage errors propagate.
    pub fn search(
        &self,
        query: &str,
        in_favorites: bool,
        sort: SongSort,
        cursor: Option<&OpaqueCursor>,
        limit: usize,
    ) -> Result<PagedSongs, Error> {
        let page = CatalogQuery::new(self.deps.catalog.as_ref()).search(
            query,
            in_favorites,
            sort,
            cursor,
            limit,
        )?;
        Ok(PagedSongs::from(page))
    }

    /// 喜欢的音乐 (keyset-paginated).
    ///
    /// # Errors
    ///
    /// `Unavailable` when there is no active root; `Conflict` for a stale
    /// cursor; storage errors propagate.
    pub fn favorites(
        &self,
        sort: SongSort,
        cursor: Option<&OpaqueCursor>,
        limit: usize,
    ) -> Result<PagedSongs, Error> {
        let page = CatalogQuery::new(self.deps.catalog.as_ref()).favorites(sort, cursor, limit)?;
        Ok(PagedSongs::from(page))
    }

    /// 资料库导航计数: one authoritative total per library view.
    ///
    /// The sidebar prints these next to views the user may never have opened,
    /// so they are answered by Core directly — never assembled from rows the
    /// desktop layer happens to have paged through.
    ///
    /// # Errors
    ///
    /// `Unavailable` when there is no active root; storage errors propagate.
    pub fn library_counts(&self) -> Result<LibraryCountsDto, Error> {
        let counts = CatalogQuery::new(self.deps.catalog.as_ref()).counts()?;
        Ok(LibraryCountsDto::from(counts))
    }

    /// 最近添加.
    ///
    /// # Errors
    ///
    /// `Unavailable` when there is no active root; storage errors propagate.
    pub fn recent(&self, query: &str) -> Result<Vec<SongView>, Error> {
        let songs = CatalogQuery::new(self.deps.catalog.as_ref()).recent_100()?;
        let needle = query.trim().to_lowercase();
        Ok(songs
            .iter()
            .map(SongView::from)
            .filter(|song| {
                needle.is_empty()
                    || [
                        song.title.as_deref(),
                        song.artist.as_deref(),
                        song.album.as_deref(),
                    ]
                    .into_iter()
                    .flatten()
                    .any(|field| field.to_lowercase().contains(&needle))
            })
            .collect())
    }

    /// Return the newest available song in the active root for initial
    /// playback selection. The ordering remains owned by Core rather than
    /// being reconstructed from a desktop page.
    ///
    /// # Errors
    ///
    /// `Unavailable` when there is no active root; storage errors propagate.
    pub fn latest_available_song(&self) -> Result<Option<SongId>, Error> {
        CatalogQuery::new(self.deps.catalog.as_ref()).latest_available_song()
    }

    /// List playlists of the active root with member counts.
    ///
    /// # Errors
    ///
    /// Storage errors propagate. Returns `Ok(vec![])` when no root is active.
    pub fn playlists(&self) -> Result<Vec<PlaylistView>, Error> {
        let root = self.deps.roots.active_root()?.map(|r| r.id());
        let Some(root) = root else {
            return Ok(Vec::new());
        };
        let repos = self.deps.playlists.as_ref();
        let ids = PlaylistRepository::list(repos, root)?;
        let mut views = Vec::with_capacity(ids.len());
        for id in ids {
            let name = PlaylistRepository::name(repos, id)?.unwrap_or_default();
            let members = repos.members(id)?;
            // Pending-delete members are hidden from the playlist view
            // (`playlist_songs_query` filters `availability <> 'pending_delete'`),
            // so the sidebar count and auto-cover must agree: a just-deleted
            // song stops counting within its undo window without removing the
            // membership row undo may still restore.
            let count = members
                .iter()
                .filter(|m| m.song_availability() != SongAvailability::PendingDelete)
                .count();
            // A manual choice always wins. Otherwise the newest member's
            // embedded artwork is the playlist cover; a brand-new playlist
            // deliberately has no cover at all.
            let custom_cover_key = PlaylistRepository::cover_key(repos, id)?;
            let has_custom_cover = custom_cover_key.is_some();
            // Work backwards so adding a song without artwork never clears a
            // previously resolved automatic cover.
            let mut automatic_cover_key = None;
            for member in members.iter().rev() {
                if member.song_availability() == SongAvailability::PendingDelete {
                    continue;
                }
                if let Some(cover) = self.deps.covers.cover_of(member.song())? {
                    automatic_cover_key = Some(cover.asset_key);
                    break;
                }
            }
            let cover_key = custom_cover_key.or_else(|| automatic_cover_key.clone());
            views.push(PlaylistView::from((
                id,
                name,
                count,
                cover_key,
                automatic_cover_key,
                has_custom_cover,
            )));
        }
        Ok(views)
    }

    /// One playlist's members (available + missing shown, pending-delete hidden).
    ///
    /// # Errors
    ///
    /// Storage errors propagate.
    pub fn playlist_members(&self, playlist: PlaylistId) -> Result<Vec<SongView>, Error> {
        let rows = PlaylistMembers::new(self.deps.playlists.as_ref()).execute(playlist)?;
        let mut out = Vec::with_capacity(rows.len());
        // Repository membership order is append order (oldest first). The
        // playlist view is chronological in the other direction: the song
        // added last is the first row.
        for member in rows.into_iter().rev() {
            if let Some(song) = self.deps.songs.by_id(member.song())? {
                out.push(SongView::from(&song));
            }
        }
        Ok(out)
    }

    /// Read-only song detail.
    ///
    /// # Errors
    ///
    /// `Unavailable` when the song is unknown; storage errors propagate.
    pub fn song_detail(&self, song: SongId) -> Result<SongDetailView, Error> {
        let detail = GetSongDetail::new(
            self.deps.songs.as_ref(),
            self.deps.lyrics.as_ref(),
            self.deps.covers.as_ref(),
        )
        .execute(song)?;
        Ok(SongDetailView::from(&detail))
    }

    /// Effective lyrics of a song (task 11.4–11.6): source, timed/plain lines,
    /// plain text and parse diagnostic. Line/path-free; the UI receives only
    /// the strongest non-corrupt candidate.
    ///
    /// # Errors
    ///
    /// `Unavailable` when the song is not in the library; storage errors propagate.
    pub fn get_lyrics(
        &self,
        song: SongId,
    ) -> Result<echo_core::application::detail::SongLyrics, Error> {
        GetSongLyrics::new(self.deps.lyrics.as_ref()).execute(song)
    }

    /// The opaque cover-asset keys of the given songs (design §16).
    ///
    /// Design §115 resolves artwork **内置优先**: the scan persists the artwork
    /// embedded in the audio file (and the `.lrc`-sidecar equivalent for
    /// lyrics), and that asset is what this returns. A song without embedded
    /// artwork is simply **absent** from the map — never a fabricated key — and
    /// the list keeps the prototype's palette placeholder for it.
    ///
    /// The values are the [`echo_core`] cover cache's opaque `cv1-…` identifiers,
    /// so the caller composes `cover://<key>` and never sees a filesystem path.
    /// Unknown ids are skipped rather than failing the batch: a stale row on
    /// screen must not break artwork for the rows that are still valid.
    ///
    /// # Errors
    ///
    /// Storage errors propagate; a read never consults the write gate.
    pub fn cover_keys(
        &self,
        song_ids: &[SongId],
    ) -> Result<std::collections::BTreeMap<String, String>, Error> {
        let mut keys = std::collections::BTreeMap::new();
        for song in song_ids {
            if let Some(cover) = self.deps.covers.cover_of(*song)? {
                keys.insert(song.to_string(), cover.asset_key);
            }
        }
        Ok(keys)
    }
}
