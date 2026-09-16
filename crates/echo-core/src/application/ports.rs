//! Application-layer Ports (task 2.6).
//!
//! These are the small, business-shaped interfaces that `echo-core`'s use cases
//! depend on. Infrastructure adapters (`SQLite`, file system, metadata, lyrics,
//! cover cache, hashing, file events) implement them; the desktop layer
//! implements `SystemTrashPort` and playback-related boundaries.
//!
//! Every method returns a [`crate::error::Error`]; the *semantics* of that
//! `Result` are uniform across the interface (the caller handles the failure,
//! often by mapping to an IPC error). Repeating a `# Errors` line on each of
//! the ~two dozen trait methods would be pure noise, so the pedantic lint is
//! scoped out here with that rationale.
//!
//! Design rules honoured here (`openspec/CODE_STANDARDS.md` §3):

#![allow(clippy::missing_errors_doc)]
//!
//! - **Small and focused**: each trait has one job; no "Manager" mega-interface.
//! - **No infrastructure types leak**: no `rusqlite::Connection`, no Tauri, no
//!   mpv, no `PathBuf` carrying absolute-paths into comparisons where identity
//!   requires `RelativeMediaPath`. Paths cross these boundaries as validated
//!   [`RelativeMediaPath`]s or as opaque source locations.
//! - **Boundary conversions are explicit**: the traits speak domain and
//!   application vocabulary; concrete rows/DTOs are mapped by the adapters.
//!
//! Every trait is `Send + Sync` so use cases can run behind the desktop
//! actor/thread boundary.

use std::io::Read;
use std::time::Duration;

use crate::domain::catalog::{CatalogCounts, OpaqueCursor, Paged, SongSort};
use crate::domain::entities::{
    LibraryRoot, LyricsCandidate, LyricsSource, MediaDiagnostic, PlaylistMember, Song,
    SongAvailability,
};
use crate::domain::ids::{
    LibraryRootId, OperationId, PlaylistId, RelativeMediaPath, Revision, SongId,
};
use crate::domain::library::{
    DeviceId, HybridLogicalClock, LibraryManifest, PortableRecord, RecordKind,
};
use crate::domain::media::{AudioFormat, ParsedMetadata};
use crate::domain::state::scan::{ScanProgress, ScanState};
use crate::error::Error;

/// Root-wide rescan sentinel path for [`FileEventKind::RescanNeeded`] events
/// that have no single triggering file (queue overflow, root replacement).
pub const RESCAN_SENTINEL: &str = ".echo-rescan";

// ---------------------------------------------------------------------------
// Repository ports
// ---------------------------------------------------------------------------

/// Query/store library roots.
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

// ---------------------------------------------------------------------------
// Transaction / Unit of Work
// ---------------------------------------------------------------------------

/// Opaque transaction context — concrete adapters own the connection, use cases
/// never see it. The unit of work guarantees the closure commits atomically.
///
/// The closure returns `Result<(), Error>` and communicates results through
/// its captured state; keeping the signature free of generic parameters makes
/// the port usable as a trait object from the application layer (a use case
/// must not name a concrete adapter type to run one transaction).
pub trait UnitOfWork: Send + Sync {
    /// Run `f` inside one `SQLite` transaction, committing on success and
    /// rolling back on error. The transaction never crosses an `.await`:
    /// `f` is a plain synchronous closure.
    fn with_tx(&self, f: TxWork) -> Result<(), Error>;
}

/// The boxed, `Send` transaction body [`UnitOfWork::with_tx`] runs.
pub type TxWork = Box<dyn FnOnce(&mut dyn TxAccess) -> Result<(), Error> + Send + 'static>;

/// The narrow, repository-like surface visible *inside* a transaction.
/// Concrete implementations map these onto the same connection as the outer
/// repositories, so atomicity is real. (Marker-ish by design: use cases that
/// need cross-repository atomic writes pass `&dyn TxAccess` to repos.)
// A transaction is deliberately thread-confined. `UnitOfWork::with_tx` takes
// a synchronous closure, so this context never crosses an async/thread
// boundary; requiring `Send + Sync` here would make a real SQLite transaction
// impossible while providing no safety benefit.
pub trait TxAccess {
    /// Insert/update a song within the open transaction.
    fn upsert_song(&mut self, song: &Song) -> Result<(), Error>;
    /// Permanently remove a song after the platform has durably accepted its
    /// staged files. Foreign-key cascades remove its lyrics, overrides, play
    /// sessions and playlist memberships in this same authority snapshot.
    fn delete_song(&mut self, id: SongId) -> Result<(), Error>;
    /// Update a song's availability in the same transaction as journal state.
    fn set_song_availability(
        &mut self,
        id: SongId,
        availability: SongAvailability,
    ) -> Result<(), Error>;
    /// Update a favorite without splitting its mutation snapshot.
    fn set_song_favorite(&mut self, id: SongId, favorite: bool) -> Result<(), Error>;
    /// Record an idempotence-protected playback update.
    fn increment_song_play_count(&mut self, id: SongId) -> Result<(), Error>;
    /// Insert/update a root record.
    fn upsert_root(&mut self, root: &LibraryRoot) -> Result<(), Error>;
    /// Atomically isolate destructive writes after an indeterminate system
    /// trash result. `available` records whether the root was still readable
    /// while evaluating the staged evidence.
    fn isolate_root_writes(&mut self, id: LibraryRootId, available: bool) -> Result<(), Error>;
    /// Create a playlist in the same transaction as initial membership writes.
    fn create_playlist(
        &mut self,
        id: PlaylistId,
        root: LibraryRootId,
        name: &str,
    ) -> Result<(), Error>;
    /// Insert a playlist membership within the open transaction.
    fn insert_member(&mut self, member: &PlaylistMember) -> Result<(), Error>;
    /// Remove a playlist membership during a delete finalization.
    fn remove_member(&mut self, playlist: PlaylistId, song: SongId) -> Result<(), Error>;
    /// Persist operation-item intent/result alongside affected records.
    fn upsert_operation_item(
        &mut self,
        operation: OperationId,
        item: OperationItem,
    ) -> Result<(), Error>;
    /// Release active target claims as part of an operation's terminal
    /// transaction. This prevents a crash after finalization from leaving a
    /// path permanently reserved.
    fn release_operation_claims(&mut self, operation: OperationId) -> Result<(), Error>;
    /// Persist the operation's `undo_deadline` (epoch millis) in the same
    /// transaction as the song's pending-delete hide (design §9: the deadline
    /// and the hide are one atomic step, so a crash cannot split them).
    fn set_undo_deadline(&mut self, operation: OperationId, deadline_ms: i64) -> Result<(), Error>;
    /// Write one parsed lyrics candidate row (`(song, source)`) inside the
    /// open transaction, keyed by the candidate's source. A rescan upserts
    /// embedded/sidecar rows with fresh text; override rows are only ever
    /// written by the override use cases, never by a scan.
    fn set_lyrics_candidate(
        &mut self,
        song: SongId,
        candidate: &LyricsCandidate,
    ) -> Result<(), Error>;
    /// Remove one source's candidate row inside the open transaction (a
    /// rescan must not leave stale embedded/sidecar text behind). Scans only
    /// ever clear `Embedded`/`Sidecar`; an `Override` clear is a user action
    /// (no such entry point in 0.1.0).
    fn clear_lyrics_candidate(&mut self, song: SongId, source: LyricsSource) -> Result<(), Error>;
    /// Attach a cover asset to a song (and upsert the `cover_assets` row) in
    /// the same transaction as the song write, keeping the cache key and the
    /// database reference consistent (task 4.6).
    fn attach_cover(&mut self, song: SongId, cover: &CoverAssetRef) -> Result<(), Error>;
    /// Persist a runtime key/value (root epoch) inside the same transaction
    /// as the activation commit (design §5).
    fn set_runtime_state(&mut self, key: &str, value: &str) -> Result<(), Error>;
}

// ---------------------------------------------------------------------------
// File-system boundary
// ---------------------------------------------------------------------------

/// Result of a root-boundary read/write operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FileMeta {
    pub size: u64,
    pub modified_ns: i64,
}

/// Opaque handle to one user-selected external import source (design §8: the
/// "受桌面可信边界保护的外部源定位"). The desktop layer resolves the handle to
/// the real file it offered the user to pick; the handle itself is a logical,
/// non-path key so Core results and errors never carry an absolute location.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ImportSource {
    key: String,
}

impl ImportSource {
    /// Construct a source handle from the desktop-side logical key.
    ///
    /// # Errors
    ///
    /// [`crate::error::Error::Validation`] when the key is empty or carries
    /// path syntax — sources are identified logically, never by location.
    pub fn new(key: impl Into<String>) -> Result<Self, Error> {
        let key = key.into();
        if key.is_empty() || key.contains(['/', '\\', '\0']) || key == "." || key == ".." {
            return Err(Error::validation(
                crate::error::Subject::Other,
                "ImportSource",
                "source key must be a non-path logical identifier",
            ));
        }
        Ok(Self { key })
    }

    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }
}

/// What the reader knows about a source without reading its content.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportSourceInfo {
    /// The file's display name (e.g. `晴天.flac`) — a name, never a path.
    pub display_name: String,
    /// Content size in bytes as observed by the reader.
    pub size: u64,
}

/// A same-basename `.lrc` sidecar beside an import source (task 5.4). The
/// reader resolves the sibling by the real path it owns (extension matching
/// is the filesystem's job — case-insensitive on macOS/Windows); Core only
/// ever sees the name and size.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SidecarInfo {
    /// The sidecar's display name (e.g. `晴天.lrc`) — a name, never a path.
    pub display_name: String,
    /// Content size in bytes as observed by the reader.
    pub size: u64,
}

/// Reads user-selected external import sources. Implemented by the layer that
/// owns the file-selection result (the desktop trusted boundary) and by test
/// doubles; Core only ever sees handles and content streams, never locations.
pub trait ImportSourceReader: Send + Sync {
    /// Describe a source (display name + size) without reading its content.
    fn describe(&self, source: &ImportSource) -> Result<ImportSourceInfo, Error>;
    /// A reader over the source's full content (design §8: 逐资源源定位).
    /// The import streams this reader straight into the controlled staging
    /// directory, so implementations must serve the content from its
    /// beginning to its end; a vanished or truncated source surfaces as a
    /// short stream and is rejected by the size verification.
    fn open<'a>(&'a self, source: &ImportSource) -> Result<Box<dyn Read + 'a>, Error>;
    /// Describe the same-basename `.lrc` beside `source` (spec: 扩展名大小写
    /// 不敏感), if one exists. `Ok(None)` = no sidecar candidate. Errors are
    /// the reader's own unavailability (unreadable library, vanished
    /// directory…).
    fn sidecar(&self, source: &ImportSource) -> Result<Option<SidecarInfo>, Error>;
    /// Open the sidecar's content (design §8: 每个资源有独立源定位). `Ok(None)`
    /// = no sidecar; `Err` = one exists but cannot be read (permission, is a
    /// directory, vanished mid-selection…) and is reported as a lyrics-failed
    /// result, never a fake full success.
    fn open_sidecar<'a>(
        &'a self,
        source: &ImportSource,
    ) -> Result<Option<Box<dyn Read + 'a>>, Error>;
}

/// The verbatim result of streaming one resource into the controlled staging
/// directory: bytes written, the BLAKE3 accumulated during the copy, and the
/// staged file's root-relative location (journal bookkeeping only — never a
/// user-facing path).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StagedCopy {
    /// Bytes actually copied into the staging file.
    pub size: u64,
    /// BLAKE3 hex digest of the staged content.
    pub blake3: String,
    /// The staged file's location relative to the library root.
    pub staged_path: RelativeMediaPath,
}

/// Opaque handle to a file already placed in Echo's marker-verified staging
/// directory. It intentionally contains no filesystem path: an adapter must
/// resolve it below the operation's owned staging directory and reject unknown
/// or forged handles before publishing.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct StagedResource {
    operation: OperationId,
    resource_key: String,
}

impl StagedResource {
    /// Construct an application-level handle for one operation resource.
    ///
    /// `resource_key` is a logical item key, never a filesystem path. Adapters
    /// map it to their private, marker-verified staging location.
    pub fn new(operation: OperationId, resource_key: impl Into<String>) -> Result<Self, Error> {
        let resource_key = resource_key.into();
        if resource_key.is_empty()
            || resource_key.contains(['/', '\\', '\0'])
            || resource_key == "."
            || resource_key == ".."
        {
            return Err(Error::validation(
                crate::error::Subject::Path,
                "StagedResource",
                "resource key must be a non-path logical identifier",
            ));
        }
        Ok(Self {
            operation,
            resource_key,
        })
    }

    #[must_use]
    pub const fn operation(&self) -> OperationId {
        self.operation
    }

    #[must_use]
    pub fn resource_key(&self) -> &str {
        &self.resource_key
    }
}

/// The library file system — always root-constrained operations. The adapter
/// resolves a root's absolute path internally; use cases only pass
/// [`RelativeMediaPath`].
pub trait LibraryFileSystem: Send + Sync {
    /// Enumerate supported files under `root`. Follows no symlinks (leaks out
    /// of the root are rejected by the adapter).
    fn enumerate(&self, root: LibraryRootId) -> Result<Vec<RelativeMediaPath>, Error>;
    /// Metadata of one file (size + mtime) for scan fast-skip.
    fn file_meta(&self, root: LibraryRootId, path: &RelativeMediaPath) -> Result<FileMeta, Error>;
    /// Read up to `limit` bytes (metadata/tag reads).
    fn read_head(
        &self,
        root: LibraryRootId,
        path: &RelativeMediaPath,
        limit: u64,
    ) -> Result<Vec<u8>, Error>;
    /// Atomically publish an adapter-owned staging resource into its final
    /// root-relative path. The publication reserves the target exclusively
    /// (create-new: any existing entry — including a symlink — is a conflict,
    /// never a replacement), then renames the staged file onto the reserved
    /// name (design §8: 新建 exclusive 目标 + fsync + rename 且绝不替换).
    /// Callers cannot pass an arbitrary external path.
    fn publish(
        &self,
        root: LibraryRootId,
        staged: &StagedResource,
        target: &RelativeMediaPath,
    ) -> Result<(), Error>;
    /// Place content into the operation's adapter-owned staging area so it
    /// can be published to its target afterwards (the ingestion entry the
    /// import use case drives; task 5.1). The adapter decides the physical
    /// location — use cases only hold the [`StagedResource`] handle.
    fn stage(
        &self,
        root: LibraryRootId,
        staged: &StagedResource,
        content: &[u8],
    ) -> Result<(), Error>;
    /// Stream-copy `content` into the operation's marker-verified staging
    /// directory (task 5.3: 流式复制+BLAKE3). Implementations pump the reader
    /// in bounded chunks into an exclusively created file inside the owned
    /// staging slot, accumulate BLAKE3 while copying, fsync the staged file
    /// and only then report it as staged. The returned [`StagedCopy`] is the
    /// evidence the journal records (per-resource source/staging/hash).
    fn stage_stream(
        &self,
        root: LibraryRootId,
        staged: &StagedResource,
        content: &mut dyn Read,
    ) -> Result<StagedCopy, Error>;
    /// Read a staged resource back through its handle (the import parses the
    /// staged copy's tags before a target name exists). Unknown or forged
    /// handles are rejected.
    fn read_staged(&self, root: LibraryRootId, staged: &StagedResource) -> Result<Vec<u8>, Error>;
    /// Best-effort removal of a staged resource (failure-path cleanup; the
    /// operation's journal keeps the diagnostic). Idempotent: discarding an
    /// unknown handle or an already removed file succeeds.
    fn discard_staged(&self, root: LibraryRootId, staged: &StagedResource) -> Result<(), Error>;
    /// Safely remove one *persisted* staged file by its root-relative path
    /// (task 5.5/5.10 recovery rollback: 无完整暂存且未发布则清理安全残留并回滚).
    /// Like [`Self::publish_from_staging_path`], the adapter must verify the
    /// path resolves inside Echo's own marker-verified staging area — a foreign
    /// path is refused, never deleted. Idempotent: an absent file succeeds.
    fn discard_staging_path(
        &self,
        root: LibraryRootId,
        staging_path: &RelativeMediaPath,
    ) -> Result<(), Error>;
    /// Best-effort removal of an adapter-published *final* file at `target` (a
    /// root-relative library path, never a staging path). Used by the import
    /// pre-commit dedup (task 5.6): when the content hash already belongs to
    /// another song record, the import removes the redundant duplicate file it
    /// just published so identical content never leaves a second library file
    /// (绝不复制/绝无重复文件) while returning the existing record. The caller
    /// has first verified the file's hash equals the duplicate content's hash,
    /// so no unique data is removed. The adapter must refuse symlink/reparse
    /// targets (never follow) and must succeed idempotently whether or not the
    /// file exists; only a real regular file is removed.
    fn discard_published(
        &self,
        root: LibraryRootId,
        target: &RelativeMediaPath,
    ) -> Result<(), Error>;
    /// Whether a root-relative path currently exists (recovery's three-location
    /// check). `Ok(false)` is "not present", distinct from an I/O error in
    /// checking the filesystem itself.
    fn path_exists(&self, root: LibraryRootId, path: &RelativeMediaPath) -> Result<bool, Error>;
    /// Publish a file already staged at the persisted, root-relative
    /// `staging_path` into `target` with the same exclusive create-new +
    /// fsync + rename contract as [`Self::publish`] (task 5.5 recovery: 只有
    /// 暂存正确则重试 exclusive publish). Unlike [`Self::publish`] this does
    /// NOT resolve an in-memory staged handle — it reads the *journal's*
    /// persisted staging location, which is what survives a crash — so the
    /// adapter must verify `staging_path` still resolves inside Echo's own
    /// marker-verified staging area before publishing (never a foreign path).
    fn publish_from_staging_path(
        &self,
        root: LibraryRootId,
        staging_path: &RelativeMediaPath,
        target: &RelativeMediaPath,
    ) -> Result<(), Error>;
    /// Resolve the root-relative trash slot for one delete-operation resource
    /// WITHOUT moving anything (design §9: 专属受控 `trash/<operation-id>`).
    /// The use case persists this location in the `StagePending` journal item
    /// *before* the rename, so a crash between the state write and the move
    /// still leaves a durable, resolvable staged location for recovery.
    /// `resource_key` is a logical per-resource item name (e.g. `audio` /
    /// `lyrics`), never a filesystem path. The adapter owns the slot layout;
    /// the returned path must be stable and match what [`Self::stage_to_trash`]
    /// later fills.
    fn trash_path(
        &self,
        root: LibraryRootId,
        operation: OperationId,
        resource_key: &str,
    ) -> Result<RelativeMediaPath, Error>;
    /// Move a *library-published* file at `source` into Echo's controlled
    /// `trash/<operation-id>` slot (design §9: 同盘 rename 移入专属受控目录的
    /// `trash/<operation-id>`), i.e. the delete stage. Like [`Self::publish`],
    /// the adapter owns the physical slot: `resource_key` is a logical
    /// per-resource item name (e.g. `audio` / `lyrics`), never a filesystem
    /// path, and the resolved trash file is created exclusively (an existing
    /// entry is a conflict, never replaced). `source` must be a real library
    /// file (a symlink/reparse target is refused). The move is a same-volume
    /// rename so the staged copy is atomically visible whole. Returns the
    /// root-relative trash path the journal records and that recovery/undo
    /// later resolve; an implementation must keep it stable for the
    /// operation's lifetime and equal to [`Self::trash_path`].
    fn stage_to_trash(
        &self,
        root: LibraryRootId,
        operation: OperationId,
        source: &RelativeMediaPath,
        resource_key: &str,
    ) -> Result<RelativeMediaPath, Error>;
    /// Move a file back out of the trash slot to a library `target` (design §9
    /// undo: 把文件移回原路径). The same exclusive create-new contract as a
    /// publish: a non-empty existing target is a conflict, never replaced — the
    /// use case retries against a safe numbered path. `trash` must resolve
    /// inside the owned `trash/` subdirectory; a symlink/reparse `target` is
    /// refused.
    fn restore_from_trash(
        &self,
        root: LibraryRootId,
        trash: &RelativeMediaPath,
        target: &RelativeMediaPath,
    ) -> Result<(), Error>;
    /// Whether the root currently permits writes (permissions + marker).
    fn write_capable(&self, root: LibraryRootId) -> Result<bool, Error>;
    /// Acquire write capability for `root` by establishing its owned staging
    /// directory (design §8: 首次获得写能力时 exclusive-create). Idempotent —
    /// a root that already has capability is a no-op. Called by root
    /// activation/prepare when a candidate root is brought writable, so a
    /// freshly-chosen directory is never permanently read-only.
    ///
    /// # Errors
    ///
    /// Propagates the inability to create/verify the staging directory.
    fn establish_write_capability(&self, root: LibraryRootId) -> Result<(), Error>;
}

// ---------------------------------------------------------------------------
// Media, metadata & hashing
// ---------------------------------------------------------------------------

/// Container-level media probe: format + duration + audio parameters.
/// Deliberately separate from tag reading so probing can live on its own
/// actor / thread budget.
pub trait MediaProbe: Send + Sync {
    fn probe(&self, root: LibraryRootId, path: &RelativeMediaPath) -> Result<ProbeOutcome, Error>;
}

/// Probe result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProbeOutcome {
    Audio {
        format: AudioFormat,
        duration: Option<Duration>,
    },
    /// The file is a supported container but has no playable audio track.
    NoAudioTrack,
    /// Not a supported media file at all.
    Unsupported,
}

/// Metadata (tags) reader — returns parsed fields and optionally embedded
/// cover/lyrics handles through [`CoverCache`]/[`LyricsParser`].
pub trait MetadataReader: Send + Sync {
    fn read(&self, root: LibraryRootId, path: &RelativeMediaPath) -> Result<ParsedMetadata, Error>;
    /// Read the tags of in-memory content. Import sources are planned from
    /// their bytes (task 5.2: the `歌手/歌手 - 歌曲名.扩展名` target must be
    /// known before anything is written into the library), so the reader must
    /// accept content directly, not only files under a library root.
    ///
    /// # Errors
    ///
    /// Unreadable or unrecognized content (corrupt container) — the caller
    /// reports a failed input instead of guessing a name.
    fn read_bytes(&self, content: &[u8]) -> Result<ParsedMetadata, Error>;
}

/// Full-file content hashing (BLAKE3). Returns the hex digest.
pub trait ContentHasher: Send + Sync {
    fn hash(&self, root: LibraryRootId, path: &RelativeMediaPath) -> Result<String, Error>;
    fn hash_of_bytes(&self, bytes: &[u8]) -> String;
}

/// Cover asset store — keyed by content hash, returns an opaque asset key
/// (never a raw filesystem path to the UI).
pub trait CoverCache: Send + Sync {
    /// Persist cover bytes under their content hash; returns the asset key.
    fn put(&self, bytes: &[u8], mime: &str) -> Result<String, Error>;
    /// Resolve an asset key to its byte ranges for the read-only protocol;
    /// unknown/malformed keys are rejected.
    fn get(&self, asset_key: &str) -> Result<Option<Vec<u8>>, Error>;
    /// Delete unreferenced assets (GC entry point).
    fn gc(&self, referenced_keys: &[String]) -> Result<(), Error>;
}

/// Lyrics parser — turns raw text into typed, timestamp-sorted lines.
pub trait LyricsParser: Send + Sync {
    fn parse(&self, raw: &str) -> LyricsCandidate;
}

/// Lyrics candidates persisted for one song (task 4.5). Writes go through
/// [`TxAccess::set_lyrics_candidate`] so a reconcile batch stays atomic;
/// reads are standalone because selection (`Override > Embedded > Sidecar`,
/// corrupt falls back) must not re-read media files.
pub trait LyricsRepository: Send + Sync {
    /// Every stored candidate of the song (all sources, valid or not).
    fn candidates(&self, song: SongId) -> Result<Vec<LyricsCandidate>, Error>;
}

/// Cover-asset references between songs and the [`CoverCache`] (task 4.6).
pub trait CoverRepository: Send + Sync {
    /// The cover reference of one song, if any.
    fn cover_of(&self, song: SongId) -> Result<Option<CoverAssetRef>, Error>;
    /// Every asset key still referenced by any song of the root — the GC
    /// keep-set (`CoverCache::gc` must never delete a referenced asset).
    fn referenced_asset_keys(&self, root: LibraryRootId) -> Result<Vec<String>, Error>;
}

/// Persistence of one scan generation's progress, summary and per-file
/// issues (`scan_runs` / `scan_issues`, task 4.10). A run is identified by
/// `(root, generation)`; generations are root-scoped and monotonic.
pub trait ScanRunRepository: Send + Sync {
    /// Open a new run row for `(root, generation)` in the `Queued` state.
    fn begin_run(&self, root: LibraryRootId, generation: u64) -> Result<(), Error>;
    /// Persist a throttled progress snapshot (at most one per 100 ms).
    fn update_progress(
        &self,
        root: LibraryRootId,
        generation: u64,
        progress: &ScanProgress,
    ) -> Result<(), Error>;
    /// Record one per-file diagnostic; bad files never abort the run.
    fn record_issue(
        &self,
        root: LibraryRootId,
        generation: u64,
        issue: &MediaDiagnostic,
    ) -> Result<(), Error>;
    /// Persist the terminal state and the final summary. Called exactly once
    /// per run; terminal snapshots must never be lost.
    fn finish_run(
        &self,
        root: LibraryRootId,
        generation: u64,
        state: ScanState,
        progress: &ScanProgress,
    ) -> Result<(), Error>;
    /// The newest generation recorded for the root, if any.
    fn latest_generation(&self, root: LibraryRootId) -> Result<Option<u64>, Error>;
}

/// Small key/value store for runtime-persisted state (the `root_epoch`
/// counter today). Read side of [`TxAccess::set_runtime_state`].
pub trait RuntimeStateStore: Send + Sync {
    fn load(&self, key: &str) -> Result<Option<String>, Error>;
}

// ---------------------------------------------------------------------------
// Events, clock & identity
// ---------------------------------------------------------------------------

/// A normalized file-system change event.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileEvent {
    pub root: LibraryRootId,
    /// Relative path that changed (always root-space).
    pub path: RelativeMediaPath,
    /// Kind of change.
    pub kind: FileEventKind,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FileEventKind {
    Created,
    Modified,
    Removed,
    /// Renamed from a (root-relative) prior path.
    Renamed {
        from: RelativeMediaPath,
    },
    /// The event stream degraded: watcher queue overflow, root replacement or
    /// an unclassifiable rename. Reconciling must downgrade to an incremental
    /// or full rescan instead of guessing (task 4.9). The path is the
    /// triggering file when one exists, otherwise the `.echo-rescan` sentinel
    /// (a valid relative path reserved as a signal — it never names a file).
    RescanNeeded,
}

/// The file-event stream (desktop adapter = `notify`, tests = scripted).
pub trait FileEventSource: Send + Sync {
    /// Subscribe to normalized, debounced events for a root.
    ///
    /// Adapters buffer/coalesce; use cases reconcile at their own pace. The
    /// returned handle is cancel-safe (dropping it unsubscribes).
    fn subscribe(&self, root: LibraryRootId) -> Result<Box<dyn FileEventSubscription>, Error>;
}

/// A cancel-safe subscription.
pub trait FileEventSubscription: Send + Sync {
    /// Blocking read of the next ready event (the actor loop owns timing).
    fn recv(&mut self) -> Result<Option<FileEvent>, Error>;
}

/// Monotonic clock for playback statistics, undo deadlines and journaling.
pub trait Clock: Send + Sync {
    /// Monotonic elapsed (never goes backward; safe for duration math).
    fn now_monotonic(&self) -> Duration;
    /// Wall-clock for persistence (undo deadlines, timestamps).
    fn now_wall(&self) -> std::time::SystemTime;
}

/// Identity generator (deterministic in tests).
pub trait IdGenerator: Send + Sync {
    fn new_song_id(&self) -> SongId;
    fn new_playlist_id(&self) -> PlaylistId;
    fn new_operation_id(&self) -> OperationId;
    fn new_library_root_id(&self) -> LibraryRootId;
}

/// The single, stable logical identity of this device (migration 0006
/// `device_state`). Every portable record's `updated_by_device_id` comes from
/// here; it is generated once and never regenerated, and is local-only (never
/// part of the `echo/` control surface).
pub trait DeviceIdProvider: Send + Sync {
    /// The device id portable records must be stamped with.
    fn current_device_id(&self) -> DeviceId;
}

/// Read-back of the sync-foundation shape for a *committed* object: the
/// outbox-derived monotone revision and the HLC the object's last write
/// stamped. Used by the materializers to build portable records that share the
/// exact revision/HLC the `SQLite` row and outbox row carry.
pub trait SyncStateReader: Send + Sync {
    /// The object's current outbox revision (`MAX(revision)`), or `Revision(0)`
    /// when nothing has been enqueued for it yet.
    fn outbox_revision(&self, object_type: &str, object_uuid: &str) -> Result<Revision, Error>;
    /// The HLC last stamped into the object's canonical row (`songs`,
    /// `song_overrides`, `playlists`), or `None` before the first stamp.
    fn object_hlc(
        &self,
        object_type: &str,
        object_uuid: &str,
    ) -> Result<Option<HybridLogicalClock>, Error>;
}

// ---------------------------------------------------------------------------
// Portable library control plane
// ---------------------------------------------------------------------------

/// The portable `echo/` control surface of a library.
///
/// This port abstracts the on-disk layout of the portable library — the
/// versioned [`LibraryManifest`] at `echo/manifest.json` and the per-object
/// [`PortableRecord`] files under `echo/records/<kind>/<prefix>/<uuid>.json`.
/// The control surface is what a future sync connector uploads/downloads (and
/// what a new device reads to restore the library), while `echo/tmp/` stays
/// a local-only recovery workspace that never enters this port.
///
/// Implementations must:
///
/// - Read/write each file **atomically** (write-to-temp + fsync + rename, never
///   in-place), so a filesystem-synced copy never observes a half-written JSON.
/// - Ignore temporaries (a same-dir temp file) and incomplete/version-unsupported
///   records, reporting them as absent so the caller retries on the next write.
/// - Never resolve an absolute path or a `..`-escaping path: [`ControlPath`]
///   values are already validated to stay inside `echo/` and outside
///   `echo/tmp/`.
///
/// The library's logical identity is the [`LibraryId`] carried by the manifest;
/// a device verifies it before treating a directory as its sync source.
pub trait ControlPlanePort: Send + Sync {
    /// Write (create or replace) the manifest at `echo/manifest.json`.
    fn write_manifest(&self, root: LibraryRootId, manifest: &LibraryManifest) -> Result<(), Error>;
    /// Read the manifest. `Ok(None)` when `echo/manifest.json` is absent or
    /// not yet initialized.
    fn read_manifest(&self, root: LibraryRootId) -> Result<Option<LibraryManifest>, Error>;
    /// Write (create or replace) one object record. The path is derived from
    /// the record kind + UUID prefix.
    fn write_record(&self, root: LibraryRootId, record: &PortableRecord) -> Result<(), Error>;
    /// Read one object record by kind + UUID. `Ok(None)` when the record is
    /// absent or is a malformed/unsupported temporary.
    fn read_record(
        &self,
        root: LibraryRootId,
        kind: RecordKind,
        object_uuid: &str,
    ) -> Result<Option<PortableRecord>, Error>;
    /// Remove one object record. Missing records are a no-op.
    fn delete_record(
        &self,
        root: LibraryRootId,
        kind: RecordKind,
        object_uuid: &str,
    ) -> Result<(), Error>;
    /// Enumerate every record of one kind as their raw portable JSON (paths,
    /// not parsed content — for restore, which re-projects). Sorted by UUID.
    fn list_records(&self, root: LibraryRootId, kind: RecordKind) -> Result<Vec<String>, Error>;
    /// Whether the library's control surface is usable (the `echo/` directory
    /// can be created/updated). `Ok(false)` when the control plane is not
    /// usable — the library must not be enabled for sync/logic changes.
    fn control_plane_usable(&self, root: LibraryRootId) -> Result<bool, Error>;
}

// ---------------------------------------------------------------------------
// System trash
// ---------------------------------------------------------------------------

/// Outcome of moving a delete-operation's staging directory to the system
/// trash. This is the "explicitly confirmable, cross-restart" contract the
/// delete design requires (`docs/DESIGN.md` §9): the command only reports
/// success if the OS call returned success; otherwise the journal stays in
/// `TrashPending`/`TrashOutcomeUnknown`.
pub trait SystemTrashPort: Send + Sync {
    /// Move the whole `trash/<operation-id>` directory to the system trash.
    ///
    /// Returns `Ok(())` ONLY when the platform call unambiguously succeeded.
    /// Any other outcome is a failure the journal must preserve.
    fn send_to_trash(&self, root: LibraryRootId, operation: OperationId) -> Result<(), Error>;
}
