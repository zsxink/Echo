use super::*;

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
