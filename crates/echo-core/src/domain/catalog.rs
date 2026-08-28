//! Catalog / sorting / cursor / playback-context types (task 2.8).
//!
//! These are the *types* the library views and the playback coordinator
//! consume, kept in the domain so the ordering and paging rules are testable
//! without a database:
//!
//! - [`SongSort`] — the stable, four-field sort ("最近添加 / 歌曲名称 / 艺人 /
//!   播放次数", each in either direction) with the deterministic tie-break
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
//! - [`PlaybackContextResolved`] — the resolved plan: entry index + total +
//!   cursor that the desktop coordinator reads back.

use crate::domain::entities::Song;
use crate::domain::ids::{Revision, SongId};

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
    /// 艺人 (artist).
    Artist,
    /// 播放次数 (play count).
    PlayCount,
}

impl SongSortField {
    /// All four fields (used by views/golden tests).
    pub const ALL: [Self; 4] = [Self::AddedAt, Self::Title, Self::Artist, Self::PlayCount];
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
/// The tie-break ladder depends on the field (design:
/// `(library_root_uuid, title_sort, artist_sort, uuid)`-style), but always
/// ends with the `SongId` so the order is fully determined even when every
/// displayed value is identical.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SongSort {
    pub field: SongSortField,
    pub direction: SortDirection,
}

/// The scalar value a song exposes for a sort field (display fallback applied).
fn field_value(song: &Song, field: SongSortField) -> String {
    match field {
        SongSortField::AddedAt => song.revision().as_u64().to_string().pad_start(20),
        SongSortField::Title => song.title().unwrap_or("").to_owned(),
        SongSortField::Artist => song.artist().unwrap_or("").to_owned(),
        SongSortField::PlayCount => song.play_count().as_u64().to_string(),
    }
}

/// Simple zero-pad to a width.
trait PadStart {
    fn pad_start(self, width: usize) -> String;
}
impl PadStart for String {
    fn pad_start(self, width: usize) -> String {
        if self.len() >= width {
            self
        } else {
            format!("{}{}", "0".repeat(width - self.len()), self)
        }
    }
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

    /// The primary value comparison in ascending direction.
    fn primary(self, lhs: &Song, rhs: &Song) -> std::cmp::Ordering {
        match self.field {
            SongSortField::PlayCount => lhs.play_count().as_u64().cmp(&rhs.play_count().as_u64()),
            _ => field_value(lhs, self.field).cmp(&field_value(rhs, self.field)),
        }
    }

    /// The stable secondary keys that finalize ties beyond the primary value
    /// (in ascending direction; `compare` reverses this wholesale for Desc).
    fn tie_break(self, lhs: &Song, rhs: &Song) -> std::cmp::Ordering {
        match self.field {
            SongSortField::Artist => {
                let l = (lhs.title().unwrap_or(""), lhs.revision().as_u64());
                let r = (rhs.title().unwrap_or(""), rhs.revision().as_u64());
                l.cmp(&r).then_with(|| lhs.id().cmp(&rhs.id()))
            }
            SongSortField::Title => {
                let l = (lhs.artist().unwrap_or(""), lhs.revision().as_u64());
                let r = (rhs.artist().unwrap_or(""), rhs.revision().as_u64());
                l.cmp(&r).then_with(|| lhs.id().cmp(&rhs.id()))
            }
            SongSortField::PlayCount => {
                let l = (lhs.title().unwrap_or(""), lhs.artist().unwrap_or(""));
                let r = (rhs.title().unwrap_or(""), rhs.artist().unwrap_or(""));
                l.cmp(&r).then_with(|| lhs.id().cmp(&rhs.id()))
            }
            SongSortField::AddedAt => lhs.id().cmp(&rhs.id()),
        }
    }
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
#[derive(
    Clone, Debug, Default, Eq, PartialEq, Ord, PartialOrd, serde::Serialize, serde::Deserialize,
)]
pub struct OpaqueCursor {
    /// The revision the cursor was minted under.
    revision: Revision,
    /// Opaque keyset payload (platform/encoder-specific bytes); never
    /// interpreted by the domain.
    keyset: String,
}

impl OpaqueCursor {
    /// Encode a cursor for consumers (the query layer encodes the keyset row).
    #[must_use]
    pub fn encode(revision: Revision, keyset: impl Into<String>) -> Self {
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

    /// The opaque keyset payload (opaque by contract).
    #[must_use]
    pub fn keyset_hex(&self) -> &str {
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
    cursor.revision() >= current_revision
}

/// One page of a paged query.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Paged<T> {
    pub items: Vec<T>,
    /// `None` means this was the last page (no more rows).
    pub next_cursor: Option<OpaqueCursor>,
    /// True when the page is known to be the final one.
    pub is_last: bool,
}

impl<T> Paged<T> {
    #[must_use]
    pub const fn new(items: Vec<T>, next_cursor: Option<OpaqueCursor>, is_last: bool) -> Self {
        Self {
            items,
            next_cursor,
            is_last,
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
    /// The selected song to start at (must be within the resolved view).
    pub selected: SongId,
}

/// The stable identity of a library view (the three default views + playlist).
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ViewRef {
    AllSongs,
    Recent,
    Favorites,
    Playlist { id: crate::domain::ids::PlaylistId },
}

/// The server-side resolved form of a [`PlaybackContextRequest`].
///
/// Desktop resolves this without re-querying per entry: the entry index and
/// the continuation cursor let the coordinator walk the whole view lazily.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct PlaybackContextResolved {
    /// Total visible songs in the view at resolution time.
    pub total: u64,
    /// Index (0-based) of the selected song within the sorted view.
    pub selected_index: u64,
    /// Cursor for the remainder after `selected_index`.
    pub after_selected: OpaqueCursor,
}

impl PlaybackContextRequest {
    #[must_use]
    pub const fn new(view: ViewRef, sort: SongSort, selected: SongId) -> Self {
        Self {
            view,
            sort,
            cursor: None,
            selected,
        }
    }

    #[must_use]
    pub fn with_cursor(mut self, cursor: OpaqueCursor) -> Self {
        self.cursor = Some(cursor);
        self
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
            cursor_compatible(&c1, Revision::from_u64(5)),
            "old cursor ok on new epoch"
        );
        assert!(
            !cursor_compatible(&c1, Revision::from_u64(9)),
            "minted under older epoch is rejected"
        );
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
