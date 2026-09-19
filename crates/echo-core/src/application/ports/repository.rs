use crate::domain::catalog::{CatalogCounts, OpaqueCursor, Paged, SongSort};
use crate::domain::entities::{LibraryRoot, PlaylistMember, Song, SongAvailability};
use crate::domain::ids::{LibraryRootId, OperationId, PlaylistId, RelativeMediaPath, SongId};
use crate::error::Error;

pub trait LibraryRepository: Send + Sync {
    /// The single active root, if any.
    fn active_root(&self) -> Result<Option<LibraryRoot>, Error>;
    /// A root by id.
    fn by_id(&self, id: LibraryRootId) -> Result<Option<LibraryRoot>, Error>;
    /// Upsert a root (records the canonical path key).
    fn upsert(&self, root: &LibraryRoot) -> Result<(), Error>;
    /// Deactivate the active root record (kept, not deleted).
    fn deactivate(&self, id: LibraryRootId) -> Result<(), Error>;
    /// Set the write capability and availability of a root.
    fn set_write_and_availability(
        &self,
        id: LibraryRootId,
        write_capable: bool,
        available: bool,
    ) -> Result<(), Error>;
    /// Persist a safety isolation for destructive operations. It is separate
    /// from filesystem capability so a later permission probe cannot clear an
    /// indeterminate trash outcome.
    fn set_write_safety_locked(&self, id: LibraryRootId, locked: bool) -> Result<(), Error>;
}

/// Query/store songs.
pub trait SongRepository: Send + Sync {
    fn by_id(&self, id: SongId) -> Result<Option<Song>, Error>;
    fn by_path(&self, root: LibraryRootId, path: &RelativeMediaPath)
        -> Result<Option<Song>, Error>;
    /// Every song of one root — the scan's identity snapshot for relinking
    /// and the final missing pass. Bounded by the library size, never by a
    /// page limit: scans need the whole picture.
    fn all_in_root(&self, root: LibraryRootId) -> Result<Vec<Song>, Error>;
    fn upsert(&self, song: &Song) -> Result<(), Error>;
    fn set_availability(&self, id: SongId, availability: SongAvailability) -> Result<(), Error>;
    fn set_favorite(&self, id: SongId, favorite: bool) -> Result<(), Error>;
    fn increment_play_count(&self, id: SongId) -> Result<(), Error>;
    /// Restore a play count merged from portable `play-stats` records. The
    /// counter only ever rises, so a repeated continuation is idempotent (and
    /// never erases plays this device recorded locally).
    fn set_play_count(&self, id: SongId, count: u64) -> Result<(), Error>;
}

/// Keyset-paginated catalog queries over the **active root** (task 6.1,
/// design §6). Every view hides pending-delete songs and only ever returns
/// songs of the active root, so the UI can render all/favorite/playlist views
/// from one stable, pageable contract.
pub trait CatalogQueryRepository: Send + Sync {
    /// The `AllSongs` view over the active root: available songs only,
    /// keyset-paginated, pending-delete hidden.
    fn all_songs(
        &self,
        sort: SongSort,
        cursor: Option<&OpaqueCursor>,
        limit: usize,
    ) -> Result<Paged<Song>, Error>;
    /// The `Favorites` view over the active root: only favorited, available
    /// songs, keyset-paginated, pending-delete hidden.
    fn favorites(
        &self,
        sort: SongSort,
        cursor: Option<&OpaqueCursor>,
        limit: usize,
    ) -> Result<Paged<Song>, Error>;
    /// The active root's newest 100 available songs (`added_at` desc, stable
    /// UUID tie-break).
    fn recent_100(&self) -> Result<Vec<Song>, Error>;
    /// One playlist's song rows ordered by member position (available +
    /// missing shown, pending-delete hidden, active root only).
    fn playlist_songs(&self, playlist: PlaylistId) -> Result<Vec<Song>, Error>;
    /// How many songs each library view holds, as one total per view.
    ///
    /// This exists because a navigation count is needed *before* a view is
    /// opened: paging to the end of a 50,000-song view to learn its size is
    /// not an acceptable way to render a sidebar. Implementations must apply
    /// the same membership rules as [`Self::all_songs`] / [`Self::favorites`]
    /// (active root, available only) so a count never disagrees with the list
    /// it advertises, and must fail with `Unavailable` — never return zero —
    /// when there is no active root: zero means "empty library", which is a
    /// different fact.
    fn counts(&self) -> Result<CatalogCounts, Error>;
    /// Search overlay over the active root (task 6.2). `query` is matched
    /// case-insensitively as a full-query contains across title, artist and
    /// album of the *normalized* keys (FTS for ≥3 scalars, escaped LIKE for
    /// short queries). `in_favorites` restricts results to favorited songs so
    /// search can combine with either the all-songs or the favorites view. The
    /// result is keyset-paginated identically to [`Self::all_songs`].
    fn search(
        &self,
        query: &str,
        in_favorites: bool,
        sort: SongSort,
        cursor: Option<&OpaqueCursor>,
        limit: usize,
    ) -> Result<Paged<Song>, Error>;
}

/// Query/store playlists and their members.
pub trait PlaylistRepository: Send + Sync {
    fn by_id(&self, id: PlaylistId) -> Result<Option<PlaylistId>, Error>;
    /// The display name of one playlist (used by the desktop to render the
    /// playlist list; task 7.3).
    fn name(&self, id: PlaylistId) -> Result<Option<String>, Error>;
    /// A user-selected cover asset key. `None` means the playlist follows its
    /// most recently added song's embedded artwork.
    fn cover_key(&self, id: PlaylistId) -> Result<Option<String>, Error>;
    fn by_name(
        &self,
        root: LibraryRootId,
        normalized_name: &str,
    ) -> Result<Option<PlaylistId>, Error>;
    fn list(&self, root: LibraryRootId) -> Result<Vec<PlaylistId>, Error>;
    fn create(&self, id: PlaylistId, root: LibraryRootId, name: &str) -> Result<(), Error>;
    fn rename(&self, id: PlaylistId, to_normalized_name: &str) -> Result<(), Error>;
    /// Set (or clear) the user-selected cover. Clearing restores auto-cover.
    fn set_cover_key(&self, id: PlaylistId, key: Option<&str>) -> Result<(), Error>;
    fn delete(&self, id: PlaylistId) -> Result<(), Error>;
    fn members(&self, id: PlaylistId) -> Result<Vec<PlaylistMember>, Error>;
    fn add_member(&self, playlist: PlaylistId, song: SongId, position: u64) -> Result<(), Error>;
    /// Insert or refresh a membership **by its stable member identity** — the
    /// continuation path must reuse the UUID carried by the portable
    /// `playlist-items` record instead of minting a fresh one on every open.
    fn upsert_member(&self, member: &PlaylistMember) -> Result<(), Error>;
    fn remove_member(&self, playlist: PlaylistId, song: SongId) -> Result<(), Error>;
}

/// Per-resource operation journal (import / delete / restore). The journal's
/// states are the domain [`crate::domain::state::OperationState`] states; the
/// repository persists per-item rows keyed by `(operation, item)`.
pub trait OperationJournalRepository: Send + Sync {
    /// Idempotently create the operation's durable envelope — the journal's
    /// total-state row every per-resource item attaches to (design §8: 每个
    /// operation 由总状态和逐资源 `operation_items` 组成). Repeating the call
    /// for a known operation is a no-op so recovery can re-run freely.
    fn ensure_operation(
        &self,
        operation: OperationId,
        root: LibraryRootId,
        kind: &str,
        reserved_song: Option<SongId>,
    ) -> Result<(), Error>;
    /// The state of a concrete journal item.
    fn item_state(
        &self,
        operation: OperationId,
        item: &str,
    ) -> Result<Option<OperationItem>, Error>;
    fn upsert_item(&self, operation: OperationId, item: OperationItem) -> Result<(), Error>;
    fn items(&self, operation: OperationId) -> Result<Vec<OperationItem>, Error>;
    /// Every journal item of `root` that has not yet reached a terminal state
    /// — the recovery-input set (task 5.5 / 5.10). Returns `(operation, kind,
    /// item)` so recovery can group resources under one operation and decide
    /// how to finish each item from its persisted intent. Only the states the
    /// application has durably written (`Completed`, `RolledBack`,
    /// `DatabaseFinalized`) are excluded; everything else still has work to
    /// do (or a conflict to surface).
    fn incomplete_items(
        &self,
        root: LibraryRootId,
    ) -> Result<Vec<(OperationId, String, OperationItem)>, Error>;
    /// Persist the operation-level `undo_deadline` (epoch millis) on the
    /// envelope row. A delete operation records it in the same transaction
    /// that hides the song (design §9: `HiddenInDatabase (undo_deadline =
    /// now + 10s)`), so the undo window survives a crash/restart.
    fn set_undo_deadline(&self, operation: OperationId, deadline_ms: i64) -> Result<(), Error>;
    /// The persisted `undo_deadline` (epoch millis) of `operation`, if any.
    fn undo_deadline(&self, operation: OperationId) -> Result<Option<i64>, Error>;
    /// Release every active target claim of the operation. Must be called when
    /// the operation reaches a terminal state (completed, rolled back, delete
    /// finalized); until then the conditional unique index keeps the target
    /// path reserved. After release the same path may be claimed again.
    fn release_claims(&self, operation: OperationId) -> Result<(), Error>;
}

/// A single journal item record (the domain shape, not the SQL row).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationItem {
    pub kind: OperationResourceKind,
    pub state: crate::domain::state::OperationState,
    /// Reserved `SongId` (import) / the subject `SongId` (delete/restore).
    pub song: Option<SongId>,
    /// The logical external source locator (design §8: 受桌面可信边界保护的
    /// 外部源定位) — an [`ImportSource`] key, never a filesystem path.
    pub source: Option<String>,
    /// Root-relative location of the staged resource while the operation runs
    /// (design §8: item 固定保存暂存/目标相对路径); recovery resolves it.
    pub staging_path: Option<RelativeMediaPath>,
    /// Relative path in the root the final file targets.
    pub target_path: RelativeMediaPath,
    /// Expected full-file hash (BLAKE3) as hex.
    pub expected_hash: String,
    /// Stable operation-local resource identity (for example `audio` or
    /// `lyrics`). State changes must always upsert this same journal row.
    pub item_key: String,
    /// The *current* normalized target claim (`(root, normalized_target_path)`).
    /// This can change when recovery selects a numbered restore target.
    pub claim_key: String,
}

/// Kind of a journal resource.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperationResourceKind {
    Audio,
    Lyrics,
}

/// The cover-asset reference a song carries (task 4.6). `asset_key` is the
/// opaque [`CoverCache`] key — never a filesystem path — and `content_hash`
/// is the deduplicated cache identity persisted in `cover_assets`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CoverAssetRef {
    pub content_hash: String,
    pub mime: String,
    pub asset_key: String,
}
