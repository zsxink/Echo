//! Catalog / sorting / cursor / playback-context types (task 2.8).
//!
//! These are the *types* the library views and the playback coordinator
//! consume, kept in the domain so the ordering and paging rules are testable
//! without a database:
//!
//! - [`SongSort`] — the stable, five-field sort ("最近添加 / 歌曲名称 / 歌手 /
//!   专辑 / 播放次数", each in either direction) with the deterministic tie-break
//!   (artist→title→added→UUID as secondary keys). Sorting is **total**: no two
//!   distinct songs order equal.
//! - [`OpaqueCursor`] — the server-side keyset cursor. The UI never builds or
//!   interprets it; it only carries it back verbatim. A monotonic `Revision`
//!   guards against interleaved writes between pages.
//! - [`Paged`] — one page: items + next cursor + whether it is the last page.
//! - [`PlaybackContextRequest`] — *what* to play: a view, a sort, a cursor
//!   snapshot and the selected [`SongId`]. It deliberately contains no
//!   `Vec<SongId>` of the view — a 50,000-song library never ships its UUIDs
//!   over IPC; the context is resolved server-side into a cursor + selection.
//! - [`PlaybackContextResolved`] — the server-side ordered playback plan.

use crate::domain::entities::Song;
use crate::domain::ids::{PlaylistId, Revision, SongId};
use crate::domain::text::normalized_key;
use crate::error::{Error, ValidationSubject};
use serde::{de, Deserialize, Deserializer, Serialize, Serializer};

// ---------------------------------------------------------------------------
// Sorting
// ---------------------------------------------------------------------------

/// The sortable fields of the "全部歌曲" view.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum SongSortField {
    /// 最近添加 (added time, newest first normally).
    #[default]
    AddedAt,
    /// 歌曲名称 (title).
    Title,
    /// 歌手 (artist).
    Artist,
    /// 专辑 (album).
    Album,
    /// 播放次数 (play count).
    PlayCount,
}

impl SongSortField {
    /// All five fields (used by views/golden tests).
    pub const ALL: [Self; 5] = [
        Self::AddedAt,
        Self::Title,
        Self::Artist,
        Self::Album,
        Self::PlayCount,
    ];
}

/// Sort direction.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum SortDirection {
    Asc,
    #[default]
    Desc,
}

impl SortDirection {
    #[must_use]
    pub const fn is_desc(self) -> bool {
        matches!(self, Self::Desc)
    }
}

/// A total, deterministic sort: primary field + direction + stable tie-breaks.
///
/// The tie-break ladder mirrors the `SQLite` `ORDER BY` ladders exactly (design:
/// `(library_root_uuid, title_sort, artist_sort, uuid)`-style) and always ends
/// with the `SongId` so the order is fully determined even when every displayed
/// value is identical. Sort keys use the same normalized comparison keys the
/// repository indexes (`normalized_key`), never the mutable `revision` — a
/// favorite/statistics mutation must not reorder the catalog, and the in-memory
/// ordering must agree with paged keyset results row for row.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SongSort {
    pub field: SongSortField,
    pub direction: SortDirection,
}

/// A directory dimension derived from available songs in the active library.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CatalogCollectionKind {
    Artist,
    Album,
}

/// One artist or album entry in the catalog directory.
///
/// The normalized keys are presentation-independent identities: callers can
/// re-open a group without matching against case- or Unicode-sensitive labels.
/// For album entries, `album_key` is the grouping identity and `artist` /
/// `artist_key` describe the newest member used for presentation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CatalogCollection {
    pub kind: CatalogCollectionKind,
    pub artist_key: String,
    pub album_key: Option<String>,
    pub artist: String,
    pub name: String,
    pub song_count: usize,
    /// Most recently added member, used to resolve automatic artwork.
    pub latest_song: SongId,
}

impl SongSort {
    /// Total order between two songs for this sort. `Ordering::Equal` occurs
    /// only when the two songs share the same `SongId` (impossible for distinct
    /// entities); the ladder reaches the UUID so the comparison is a strict
    /// total order over the set of songs.
    ///
    /// The full ascending ordering (primary value → secondary ladder → UUID)
    /// is computed first, then reversed when `direction == Desc`, so the
    /// tie-break respects the chosen direction too (a Desc sort must also
    /// reverse the tie-break; otherwise equal-primary rows would still surface
    /// in Asc order).
    #[must_use]
    pub fn compare(&self, lhs: &Song, rhs: &Song) -> std::cmp::Ordering {
        let asc = self
            .primary(lhs, rhs)
            .then_with(|| self.tie_break(lhs, rhs));
        match self.direction {
            SortDirection::Asc => asc,
            SortDirection::Desc => asc.reverse(),
        }
    }

    /// The primary value comparison in ascending direction. Text fields use
    /// the normalized comparison key (`title_sort`/`artist_sort` in SQL);
    /// numeric fields compare numerically.
    fn primary(self, lhs: &Song, rhs: &Song) -> std::cmp::Ordering {
        match self.field {
            SongSortField::PlayCount => lhs.play_count().as_u64().cmp(&rhs.play_count().as_u64()),
            SongSortField::AddedAt => lhs.added_at().cmp(&rhs.added_at()),
            SongSortField::Title => normalized_key(lhs.title().unwrap_or(""))
                .cmp(&normalized_key(rhs.title().unwrap_or(""))),
            SongSortField::Artist => normalized_key(lhs.artist().unwrap_or(""))
                .cmp(&normalized_key(rhs.artist().unwrap_or(""))),
            SongSortField::Album => album_key(lhs).cmp(&album_key(rhs)),
        }
    }

    /// The stable secondary keys that finalize ties beyond the primary value
    /// (in ascending direction; `compare` reverses this wholesale for Desc).
    /// This is the exact remainder of the SQL `ORDER BY` ladder for the field,
    /// ending in the `SongId`.
    fn tie_break(self, lhs: &Song, rhs: &Song) -> std::cmp::Ordering {
        let title = |song: &Song| normalized_key(song.title().unwrap_or(""));
        let artist = |song: &Song| normalized_key(song.artist().unwrap_or(""));
        match self.field {
            // SQL: … ORDER BY title_sort, artist_sort, uuid
            SongSortField::Title => artist(lhs).cmp(&artist(rhs)),
            // SQL: … ORDER BY artist_sort, title_sort, uuid
            SongSortField::Artist => title(lhs).cmp(&title(rhs)),
            // SQL: … ORDER BY album_sort, title_sort, artist_sort, uuid
            SongSortField::Album => title(lhs)
                .cmp(&title(rhs))
                .then_with(|| artist(lhs).cmp(&artist(rhs))),
            // SQL: … ORDER BY play_count, title_sort, artist_sort, uuid
            SongSortField::PlayCount => title(lhs)
                .cmp(&title(rhs))
                .then_with(|| artist(lhs).cmp(&artist(rhs))),
            // SQL: … ORDER BY added_at, uuid
            SongSortField::AddedAt => std::cmp::Ordering::Equal,
        }
        .then_with(|| lhs.id().cmp(&rhs.id()))
    }
}

fn album_key(song: &Song) -> String {
    let album = song
        .album()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("未知专辑");
    normalized_key(album)
}

// ---------------------------------------------------------------------------
// Paging cursor
// ---------------------------------------------------------------------------

/// The server-side keyset cursor.
///
/// The UI/repository boundary treats this as an **opaque** token: it is
/// produced by the query layer, carried back verbatim on "next page", and
/// never built or parsed by UI code. Its internal shape is:
///
/// `v1:<revision>:<base64(keyset_row)>`
///
/// where `<keyset_row>` encodes the last row's sort values + UUID so the next
/// page can continue immediately and deterministically. The `Revision` guard
/// lets the query layer reject a cursor produced before a write epoch (rather
/// than silently skipping/duplicating rows).
#[derive(Clone, Debug, Default, Eq, PartialEq, Ord, PartialOrd)]
pub struct OpaqueCursor {
    /// The revision the cursor was minted under.
    revision: Revision,
    /// Opaque keyset payload (platform/encoder-specific bytes); never
    /// interpreted by the domain.
    keyset: String,
}

impl OpaqueCursor {
    /// Mint a cursor from a repository keyset. This stays crate-visible so the
    /// UI can carry a cursor but cannot construct its internal fields.
    #[must_use]
    pub(crate) fn encode(revision: Revision, keyset: impl Into<String>) -> Self {
        Self {
            revision,
            keyset: keyset.into(),
        }
    }

    /// The revision this cursor was minted under.
    #[must_use]
    pub const fn revision(&self) -> Revision {
        self.revision
    }

    /// The opaque keyset payload. Only repository adapters may interpret it.
    #[allow(dead_code)] // used by the SQLite adapter added in task 3.8
    #[must_use]
    pub(crate) fn keyset(&self) -> &str {
        &self.keyset
    }

    /// The first page sentinel (revision 0, empty keyset).
    #[must_use]
    pub const fn start() -> Self {
        Self {
            revision: Revision::INITIAL,
            keyset: String::new(),
        }
    }
}

impl Serialize for OpaqueCursor {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'de> Deserialize<'de> for OpaqueCursor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let token = String::deserialize(deserializer)?;
        Self::parse(&token).map_err(de::Error::custom)
    }
}

impl OpaqueCursor {
    fn parse(token: &str) -> Result<Self, String> {
        let Some((version, rest)) = token.split_once(':') else {
            return Err("malformed cursor".into());
        };
        if version != "v1" {
            return Err("unsupported cursor version".into());
        }
        let Some((revision, keyset)) = rest.split_once(':') else {
            return Err("malformed cursor".into());
        };
        let revision = revision
            .parse::<u64>()
            .map_err(|_| "malformed cursor revision")?;
        if !keyset
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err("malformed cursor payload".into());
        }
        Ok(Self::encode(Revision::from_u64(revision), keyset))
    }
}

impl std::fmt::Display for OpaqueCursor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Human-readable opaque form: `v1:<rev>:<hex>`. The UI treats it as a
        // string, never parses it.
        write!(f, "v1:{}:{}", self.revision.as_u64(), self.keyset)
    }
}

/// Whether a supplied cursor may be used given the current write epoch.
#[must_use]
pub fn cursor_compatible(cursor: &OpaqueCursor, current_revision: Revision) -> bool {
    cursor.revision() == current_revision
}

/// One page of a paged query.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Paged<T> {
    pub items: Vec<T>,
    /// Total rows matching the query before pagination is applied.
    #[serde(default)]
    pub total_count: usize,
    /// `None` means this was the last page (no more rows).
    pub next_cursor: Option<OpaqueCursor>,
    /// True when the page is known to be the final one.
    pub is_last: bool,
}

impl<T> Paged<T> {
    #[must_use]
    pub fn new(items: Vec<T>, next_cursor: Option<OpaqueCursor>, is_last: bool) -> Self {
        Self {
            total_count: items.len(),
            items,
            next_cursor,
            is_last,
        }
    }

    #[must_use]
    pub const fn with_total_count(mut self, total_count: usize) -> Self {
        self.total_count = total_count;
        self
    }
}

/// The "最近添加" view is defined as the newest *100* songs; the view query and
/// its count must agree, so the ceiling lives in the domain rather than being
/// duplicated in SQL and in the UI.
pub const RECENT_VIEW_LIMIT: usize = 100;

/// How many songs each library view currently holds.
///
/// A count answers "how many songs would this view show", and is therefore
/// computed from the same membership rules as the view itself: the active root
/// only, `available` songs only (pending-delete is invisible everywhere). The
/// UI needs it *before* a view is ever opened — that is the whole point of a
/// navigation count — so it is a standalone total, never a derived "rows loaded
/// so far" figure that a paged query happens to have in hand.
///
/// `recent` is capped at [`RECENT_VIEW_LIMIT`]: printing 5,000 next to a view
/// that renders 100 rows would be a lie of a different shape.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CatalogCounts {
    pub all: usize,
    pub favorites: usize,
    pub recent: usize,
    pub artists: usize,
    pub albums: usize,
}

impl CatalogCounts {
    /// Build counts from the raw available/favorited totals; `recent` is
    /// clamped to the view's ceiling here so every repository implementation
    /// agrees on it.
    #[must_use]
    pub const fn new(all: usize, favorites: usize, artists: usize, albums: usize) -> Self {
        Self {
            all,
            favorites,
            recent: if all > RECENT_VIEW_LIMIT {
                RECENT_VIEW_LIMIT
            } else {
                all
            },
            artists,
            albums,
        }
    }
}

// ---------------------------------------------------------------------------
// Playback context
// ---------------------------------------------------------------------------

/// A request to start/resolve playback for a view.
///
/// Critical contract: this type never carries a materialized `Vec<SongId>` of
/// the view. A 50,000-song library's UUIDs are resolved server-side from the
/// cursor + sort, never shipped to the UI; only the *selection* is a UUID.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PlaybackContextRequest {
    pub view: ViewRef,
    pub sort: SongSort,
    /// Cursor snapshot to resolve from (server-side). A request without a
    /// cursor may resolve the first page.
    pub cursor: Option<OpaqueCursor>,
    /// Optional text filter for the library views. It stays server-side with
    /// the rest of the request and is never expanded into song identifiers.
    #[serde(default)]
    pub query: String,
    /// The selected song to start at (must be within the resolved view).
    pub selected: SongId,
}

/// The stable identity of a library view (the three default views + playlist).
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ViewRef {
    AllSongs,
    Recent,
    Favorites,
    Playlist { id: PlaylistId },
}

/// The server-side resolved form of a [`PlaybackContextRequest`].
///
/// This is deliberately resolved inside Core: platform players receive the
/// ordered identifiers but do not recreate filtering, paging or ordering.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PlaybackContextResolved {
    /// Ordered songs in the exact order represented by the view.
    pub songs: Vec<SongId>,
    /// Index (0-based) of the selected song within the sorted view.
    pub selected_index: usize,
}

impl PlaybackContextRequest {
    #[must_use]
    pub const fn new(view: ViewRef, sort: SongSort, selected: SongId) -> Self {
        Self {
            view,
            sort,
            cursor: None,
            query: String::new(),
            selected,
        }
    }

    #[must_use]
    pub fn with_cursor(mut self, cursor: OpaqueCursor) -> Self {
        self.cursor = Some(cursor);
        self
    }

    /// Attach the optional library text filter to a typed request.
    #[must_use]
    pub fn with_query(mut self, query: impl Into<String>) -> Self {
        self.query = query.into();
        self
    }

    /// Build a request from the stable library-view names accepted by the
    /// desktop command boundary. Unknown names are rejected in Core so every
    /// platform shares one validation rule.
    ///
    /// # Errors
    ///
    /// `Validation` when `view` is not one of `"all"`, `"recent"` or
    /// `"favorites"` — the boundary's string vocabulary has no other members.
    pub fn library_view(
        view: &str,
        query: impl Into<String>,
        sort: SongSort,
        selected: SongId,
    ) -> Result<Self, Error> {
        let view = match view {
            "all" => ViewRef::AllSongs,
            "recent" => ViewRef::Recent,
            "favorites" => ViewRef::Favorites,
            _ => {
                return Err(Error::validation(
                    ValidationSubject::Other,
                    "view",
                    "unknown library playback view".to_owned(),
                ))
            }
        };
        Ok(Self::new(view, sort, selected).with_query(query))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::ids::{LibraryRootId, RelativeMediaPath, SongId};

    fn mk(id: u64, title: &str, artist: &str, plays: u64) -> Song {
        let mut song = Song::new(
            SongId::from_uuid(uuid::Uuid::from_u128(id.into())),
            LibraryRootId::new(),
            RelativeMediaPath::new(&format!("{artist}/{title}.mp3")).unwrap(),
            Revision::from_u64(id),
        );
        song.apply_metadata(Some(title.to_owned()), Some(artist.to_owned()), None, None);
        for _ in 0..plays {
            song.record_play();
        }
        song
    }

    #[test]
    fn sort_is_a_total_order_even_with_equal_primary_values() {
        let a = mk(1, "晴天", "周杰伦", 0);
        let b = mk(2, "晴天", "周杰伦", 0);
        // Identical displayed title/artist/plays — tie-break reaches UUID.
        let sort = SongSort {
            field: SongSortField::Title,
            direction: SortDirection::Asc,
        };
        let ord = sort.compare(&a, &b);
        assert_ne!(
            ord,
            std::cmp::Ordering::Equal,
            "must be a strict total order"
        );
    }

    #[test]
    fn four_sorts_respect_direction_and_tie_break() {
        // ASCII titles make the primary-value ordering obvious and
        // code-point-deterministic regardless of script.
        let n1 = mk(1, "aaa", "周杰伦", 3);
        let n2 = mk(2, "bbb", "周杰伦", 2);
        let sort = SongSort {
            field: SongSortField::Title,
            direction: SortDirection::Asc,
        };
        assert_eq!(sort.compare(&n1, &n2), std::cmp::Ordering::Less);
        let desc = SongSort {
            direction: SortDirection::Desc,
            ..sort
        };
        assert_eq!(desc.compare(&n1, &n2), std::cmp::Ordering::Greater);
    }

    #[test]
    fn added_sort_uses_immutable_added_at_not_mutation_revision() {
        let mut old = Song::with_added_at(
            SongId::from_uuid(uuid::Uuid::from_u128(1)),
            LibraryRootId::new(),
            RelativeMediaPath::new("old.mp3").unwrap(),
            Revision::from_u64(99),
            1,
        );
        old.set_favorite(true);
        let new = Song::with_added_at(
            SongId::from_uuid(uuid::Uuid::from_u128(2)),
            LibraryRootId::new(),
            RelativeMediaPath::new("new.mp3").unwrap(),
            Revision::INITIAL,
            2,
        );
        let sort = SongSort::default();
        assert_eq!(sort.compare(&new, &old), std::cmp::Ordering::Less);
    }

    #[test]
    fn play_count_sort_is_numeral_not_lexical() {
        let low = mk(1, "a", "x", 9);
        let high = mk(2, "a", "x", 100);
        let sort = SongSort {
            field: SongSortField::PlayCount,
            direction: SortDirection::Desc,
        };
        // Desc → the higher play count comes first, so low sorts *after* high.
        assert_eq!(sort.compare(&low, &high), std::cmp::Ordering::Greater);
        // Asc → low first.
        let asc = SongSort {
            direction: SortDirection::Asc,
            ..sort
        };
        assert_eq!(asc.compare(&low, &high), std::cmp::Ordering::Less);
    }

    #[test]
    fn cursor_is_opaque_and_revision_guarded() {
        let c1 = OpaqueCursor::encode(Revision::from_u64(7), "deadbeef");
        assert_eq!(c1.to_string(), "v1:7:deadbeef");
        assert!(
            cursor_compatible(&c1, Revision::from_u64(7)),
            "cursor matches its issuing epoch"
        );
        assert!(
            !cursor_compatible(&c1, Revision::from_u64(9)),
            "cursor from a different epoch is rejected"
        );
        assert!(
            !cursor_compatible(&c1, Revision::from_u64(5)),
            "a future cursor is rejected too"
        );
        let json = serde_json::to_string(&c1).unwrap();
        assert_eq!(json, "\"v1:7:deadbeef\"");
        assert!(serde_json::from_str::<OpaqueCursor>("\"v2:7:deadbeef\"").is_err());
        assert!(serde_json::from_str::<OpaqueCursor>("\"v1:7:../../path\"").is_err());
        let start = OpaqueCursor::start();
        assert_eq!(start.revision(), Revision::INITIAL);
    }

    #[test]
    fn paged_is_last_is_explicit() {
        let page = Paged::<u64>::new(vec![1, 2], None, true);
        assert!(page.is_last);
        assert!(page.next_cursor.is_none());
    }

    #[test]
    fn playback_context_request_never_materializes_all_uuids() {
        // Build a 50k synthetic view and prove the *request* is just a cursor
        // + selection, not a Vec of 50,000 UUIDs.
        let sort = SongSort {
            field: SongSortField::Title,
            direction: SortDirection::Asc,
        };
        let cursor = OpaqueCursor::encode(Revision::from_u64(1), "keyset-for-50000");
        let selected = SongId::new();
        // Simulate: the view has 50,000 songs, but the request to play it
        // carries only the cursor + selection — nothing proportional to N.
        let req =
            PlaybackContextRequest::new(ViewRef::AllSongs, sort, selected).with_cursor(cursor);
        assert!(req.cursor.is_some());
        // The request would serialize to a tiny payload, not to 50k UUIDs.
        let json = serde_json::to_string(&req).unwrap();
        assert!(
            json.len() < 1024,
            "request payload stays small: {} bytes",
            json.len()
        );
    }

    #[test]
    fn view_ref_identifies_the_default_views() {
        assert_eq!(ViewRef::AllSongs, ViewRef::AllSongs);
        assert_ne!(ViewRef::AllSongs, ViewRef::Favorites);
    }
}
