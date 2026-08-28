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

use std::time::Duration;

use crate::domain::entities::{
    LibraryRoot, LyricsCandidate, LyricsSource, MediaDiagnostic, PlaylistMember, Song,
    SongAvailability,
};
use crate::domain::ids::{LibraryRootId, OperationId, PlaylistId, RelativeMediaPath, SongId};
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

/// Query/store playlists and their members.
pub trait PlaylistRepository: Send + Sync {
    fn by_id(&self, id: PlaylistId) -> Result<Option<PlaylistId>, Error>;
    fn by_name(
        &self,
        root: LibraryRootId,
        normalized_name: &str,
    ) -> Result<Option<PlaylistId>, Error>;
    fn list(&self, root: LibraryRootId) -> Result<Vec<PlaylistId>, Error>;
    fn create(&self, id: PlaylistId, root: LibraryRootId, name: &str) -> Result<(), Error>;
    fn rename(&self, id: PlaylistId, to_normalized_name: &str) -> Result<(), Error>;
    fn delete(&self, id: PlaylistId) -> Result<(), Error>;
    fn members(&self, id: PlaylistId) -> Result<Vec<PlaylistMember>, Error>;
    fn add_member(&self, playlist: PlaylistId, song: SongId, position: u64) -> Result<(), Error>;
    fn remove_member(&self, playlist: PlaylistId, song: SongId) -> Result<(), Error>;
}

/// Per-resource operation journal (import / delete / restore). The journal's
/// states are the domain [`crate::domain::state::OperationState`] states; the
/// repository persists per-item rows keyed by `(operation, item)`.
pub trait OperationJournalRepository: Send + Sync {
    /// The state of a concrete journal item.
    fn item_state(
        &self,
        operation: OperationId,
        item: &str,
    ) -> Result<Option<OperationItem>, Error>;
    fn upsert_item(&self, operation: OperationId, item: OperationItem) -> Result<(), Error>;
    fn items(&self, operation: OperationId) -> Result<Vec<OperationItem>, Error>;
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
    /// Relative path in the root the final file targets.
    pub target_path: RelativeMediaPath,
    /// Expected full-file hash (BLAKE3) as hex.
    pub expected_hash: String,
    /// The `target claim` uniqueness key (`(root, normalized_target_path)`).
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
    /// root-relative path. Callers cannot pass an arbitrary external path.
    fn publish(
        &self,
        root: LibraryRootId,
        staged: &StagedResource,
        target: &RelativeMediaPath,
    ) -> Result<(), Error>;
    /// Whether the root currently permits writes (permissions + marker).
    fn write_capable(&self, root: LibraryRootId) -> Result<bool, Error>;
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
