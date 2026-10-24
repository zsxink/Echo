//! The generation-driven, cancellable library scan pipeline (tasks 4.7/4.8/4.10,
//! design §6).
//!
//! Flow: `Queued → Enumerating → Parsing (bounded workers, batched
//! reconcile) → Reconciling (final missing pass) → Completed`, with
//! `Cancelled`/`Failed` exits. Every run carries a root-scoped, monotonic
//! `generation`; only a *fully completed* run may mark unseen songs missing —
//! a cancelled or failed run never batch-deletes identities. Progress is
//! persisted as throttled snapshots (at most one per 100 ms) plus per-file
//! issues, so a crash never loses the terminal summary.
//!
//! Per-file work (probe → BLAKE3 → tags → lyrics → cover) runs on a bounded
//! worker pool; results are reconciled in small batches so UI queries stay
//! available while a scan is in flight (design §6.5).

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, UNIX_EPOCH};

use crate::application::ports::TxAccess;
use crate::application::ports::{
    CatalogQueryRepository, Clock, ContentHasher, ControlPlanePort, CoverAssetRef, CoverCache,
    CoverRepository, DeviceIdProvider, FileMeta, IdGenerator, LibraryFileSystem, LyricsParser,
    LyricsRepository, MediaProbe, MetadataReader, OperationJournalRepository, PlaylistRepository,
    ProbeOutcome, RootWriteRepository, ScanRunRepository, SongRepository, SyncStateReader,
    UnitOfWork,
};
use crate::application::relink::{ParsedFile, RelinkPlanner, Resolution};
use crate::domain::entities::{
    LyricsCandidate, LyricsSource, MediaDiagnostic, Song, SongAvailability,
};
use crate::domain::ids::{LibraryRootId, RelativeMediaPath, Revision, SongId};
use crate::domain::state::scan::{ScanProgress, ScanState};
use crate::error::Error;

pub(crate) mod durable;

/// The cheap extension filter (design §6.2): enumeration returns every
/// regular file; the scan probes only these. Everything else is a skip. The
/// import pipeline applies the same matrix to user-selected sources (task 5.1).
pub(crate) const SUPPORTED_EXTENSIONS: [&str; 8] =
    ["mp3", "flac", "m4a", "mp4", "ogg", "opus", "wav", "ape"];

/// Scan pipeline tuning.
#[derive(Clone, Copy, Debug)]
pub struct ScanConfig {
    /// Song writes per reconcile transaction (small batches, design §6.5).
    pub batch_size: usize,
    /// Bounded parse workers, ≤ `min(CPU, 4)` by default (design §6.4).
    pub worker_threads: usize,
    /// Minimum distance between persisted progress snapshots (at most one per
    /// 100 ms by default; phase transitions and terminal states always
    /// persist immediately, so the terminal summary is never lost).
    pub progress_interval: Duration,
    /// Sidecar/embedded lyrics input limit (task 4.4: 2 MiB).
    pub lyrics_limit: usize,
}

impl Default for ScanConfig {
    fn default() -> Self {
        Self {
            batch_size: 50,
            worker_threads: std::thread::available_parallelism()
                .map_or(1, std::num::NonZeroUsize::get)
                .min(4),
            progress_interval: Duration::from_millis(100),
            lyrics_limit: 2 * 1024 * 1024,
        }
    }
}

/// Every collaborator a scan needs. The composition root assembles it once
/// (`SQLite` repositories + real adapters); tests assemble fakes.
#[derive(Clone)]
pub struct ScanDeps {
    /// Runtime-owned admission shared by all local writers.
    pub activity: Arc<super::local_activity::LocalActivity>,
    /// Optional historical rescue and ordered projection capability.
    pub history: Option<Arc<dyn crate::application::ports::LibraryHistoryPort>>,
    pub roots: Arc<dyn RootWriteRepository>,
    pub songs: Arc<dyn SongRepository>,
    /// The paginated catalog read-model (`AllSongs`/`Favorites`/search/recents)
    /// — the same backing store as `songs`, exposed under its read trait so the
    /// desktop command surface can run catalog queries (task 7.3).
    pub catalog: Arc<dyn CatalogQueryRepository>,
    pub playlists: Arc<dyn PlaylistRepository>,
    pub lyrics: Arc<dyn LyricsRepository>,
    pub covers: Arc<dyn CoverRepository>,
    pub runs: Arc<dyn ScanRunRepository>,
    pub uow: Arc<dyn UnitOfWork>,
    pub fs: Arc<dyn LibraryFileSystem>,
    pub probe: Arc<dyn MediaProbe>,
    pub metadata: Arc<dyn MetadataReader>,
    pub hasher: Arc<dyn ContentHasher>,
    pub lyrics_parser: Arc<dyn LyricsParser>,
    pub cover_cache: Arc<dyn CoverCache>,
    pub journal: Arc<dyn OperationJournalRepository>,
    /// The portable `echo/` control surface (manifest + records). Used by the
    /// library-initialization, materialization and restore use cases (tasks
    /// 2.2/2.3/3.2/4.x); scan and watcher rely on it only to keep the control
    /// surface out of media.
    pub control: Arc<dyn ControlPlanePort>,
    /// The single stable device identity (migration 0006 `device_state`).
    pub device_id: Arc<dyn DeviceIdProvider>,
    /// Read-back of a committed object's outbox revision + HLC (sync shape).
    pub sync: Arc<dyn SyncStateReader>,
    pub ids: Arc<dyn IdGenerator>,
    pub clock: Arc<dyn Clock>,
    pub config: ScanConfig,
}

/// A cancel token: `CancelScan` sets it, the pipeline polls it.
#[derive(Clone, Debug, Default)]
pub struct ScanCancelToken(Arc<AtomicBool>);

impl ScanCancelToken {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
    // `cancel` and `is_cancelled` are effects/predicates used by tests and
    // the runtime; they need no `must_use`.

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Relaxed);
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}

/// Tracks in-flight scans per root (one scan at a time per root).
#[derive(Clone, Debug, Default)]
pub struct ScanSupervisor {
    active: Arc<Mutex<HashMap<LibraryRootId, ScanCancelToken>>>,
}

type ActiveScans = HashMap<LibraryRootId, ScanCancelToken>;

impl ScanSupervisor {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> MutexGuard<'_, ActiveScans> {
        self.active
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(crate) fn register(
        &self,
        root: LibraryRootId,
        token: ScanCancelToken,
    ) -> Result<(), Error> {
        let mut active = self.lock();
        if active.contains_key(&root) {
            drop(active);
            return Err(Error::conflict("a scan is already running for this root"));
        }
        active.insert(root, token);
        drop(active);
        Ok(())
    }

    pub(crate) fn unregister(&self, root: LibraryRootId) {
        self.lock().remove(&root);
    }

    /// Whether a scan is in flight for `root` (the watch coordinator buffers
    /// events while this is true).
    #[must_use]
    pub fn is_scanning(&self, root: LibraryRootId) -> bool {
        self.lock().contains_key(&root)
    }

    /// Cancel the active scan of `root`; `true` when one was cancelled.
    #[must_use]
    pub fn cancel(&self, root: LibraryRootId) -> bool {
        let token = self.lock().get(&root).cloned();
        if let Some(token) = token {
            token.cancel();
            return true;
        }
        false
    }

    /// Cancel every active scan (root-switch quiesce). Returns the roots that
    /// had scans registered.
    #[must_use]
    pub fn cancel_all(&self) -> Vec<LibraryRootId> {
        let active = self.lock();
        for token in active.values() {
            token.cancel();
        }
        active.keys().copied().collect()
    }
}

/// The terminal summary of one scan run.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScanSummary {
    pub generation: u64,
    pub cancelled: bool,
    pub progress: ScanProgress,
}

/// Starts (and runs to completion) one scan generation for a root. Blocking:
/// the desktop runtime calls it on a worker thread.
pub struct StartScan<'a> {
    deps: &'a ScanDeps,
    supervisor: &'a ScanSupervisor,
}

impl<'a> StartScan<'a> {
    #[must_use]
    pub const fn new(deps: &'a ScanDeps, supervisor: &'a ScanSupervisor) -> Self {
        Self { deps, supervisor }
    }

    /// Run the scan. `Err` means a root-level failure (enumeration abort or
    /// persistence failure); per-file problems are diagnostics, not errors.
    ///
    /// # Errors
    ///
    /// Root-level failures propagate; the run is persisted as `Failed` first
    /// and nothing is marked missing.
    pub fn run(&self, root: LibraryRootId) -> Result<ScanSummary, Error> {
        let _activity = self.deps.activity.enter()?;
        if self.deps.roots.replica_enrollment_pending(root)? {
            return Ok(ScanSummary {
                generation: self.deps.runs.latest_generation(root)?.unwrap_or(0),
                cancelled: true,
                progress: ScanProgress {
                    state: ScanState::Cancelled,
                    ..ScanProgress::default()
                },
            });
        }
        let generation = self.deps.runs.latest_generation(root)?.unwrap_or(0) + 1;
        let token = ScanCancelToken::new();
        self.supervisor.register(root, token.clone())?;
        if self.deps.activity.is_closed() {
            token.cancel();
        }
        let result = self.run_guarded(root, generation, &token);
        self.supervisor.unregister(root);
        result
    }

    /// Index the tags and embedded assets of one already-published sync song.
    /// The publication owns its identity and path. This deliberately skips
    /// enumeration and the missing pass, including for isolated resource paths
    /// outside the normal media tree.
    ///
    /// # Errors
    /// Returns a retryable error while enrollment or another scan owns the
    /// root. Parse and persistence failures leave the publication unindexed.
    pub fn refresh_published_song(&self, root: LibraryRootId, id: SongId) -> Result<(), Error> {
        let _activity = self.deps.activity.enter()?;
        if self.deps.roots.replica_enrollment_pending(root)? {
            return Err(Error::unavailable(
                "library",
                "replica enrollment is pending",
            ));
        }
        self.supervisor.register(root, ScanCancelToken::new())?;
        let result = self.refresh_published_song_guarded(root, id);
        self.supervisor.unregister(root);
        result
    }

    fn refresh_published_song_guarded(&self, root: LibraryRootId, id: SongId) -> Result<(), Error> {
        let Some(mut song) = self.deps.songs.by_id(id)? else {
            return Ok(());
        };
        if song.root() != root
            || !matches!(
                song.availability(),
                SongAvailability::Available | SongAvailability::Missing
            )
        {
            return Err(Error::conflict(
                "published song is no longer available in this root",
            ));
        }
        // A historical missing row can retain its old size/mtime stamp. Only
        // a fresh full digest can authorize restoring the completed association.
        let parsed = match parse_single_file_with_content_check(
            self.deps,
            root,
            song.path(),
            song.availability() == SongAvailability::Missing,
        ) {
            FileOutcome::Parsed(parsed) => parsed,
            FileOutcome::FastSkip { .. } => return Ok(()),
            FileOutcome::Diagnostic(diagnostic) => {
                return Err(Error::unavailable(
                    "published song metadata",
                    diagnostic.code(),
                ));
            }
        };
        if song.blake3_hash() != Some(parsed.file.hash.as_str())
            || song.file_size() != Some(parsed.file.size)
        {
            return Err(Error::conflict(
                "published song changed before metadata indexing",
            ));
        }
        song.apply_scan_facts_with_params(
            parsed.file.hash.clone(),
            parsed.file.size,
            parsed.file.mtime_ns,
            parsed.file.meta.format,
            parsed.file.meta.parameters,
        );
        // Check that the file stayed stable throughout extraction, but retain
        // the host's logical resource declarations and shared metadata.
        super::scan_resources::verify_parsed_audio(self.deps, root, &parsed)?;
        self.persist_parsed_index(song, &parsed, None)
    }

    fn run_guarded(
        &self,
        root: LibraryRootId,
        generation: u64,
        token: &ScanCancelToken,
    ) -> Result<ScanSummary, Error> {
        self.deps.runs.begin_run(root, generation)?;
        let mut progress = ScanProgress {
            state: ScanState::Queued,
            ..ScanProgress::default()
        };
        let mut emitter = ProgressEmitter::new(self.deps, root, generation);

        // ---- Enumerating ----
        transition(&mut progress, ScanState::Enumerating)?;
        emitter.emit(&progress, true)?;
        let candidates = match self.scan_candidates(root) {
            Ok(files) => files,
            Err(error) => {
                // Root-level failure: the run fails; no identity is touched.
                self.finish(root, generation, &mut progress, ScanState::Failed)?;
                return Err(error);
            }
        };
        let (audio, ignored): (Vec<_>, Vec<_>) = candidates.into_iter().partition(|path| {
            path.extension()
                .is_some_and(|ext| SUPPORTED_EXTENSIONS.contains(&ext.as_str()))
        });
        progress.discovered = u64::try_from(audio.len()).unwrap_or(u64::MAX);
        progress.skipped += u64::try_from(ignored.len()).unwrap_or(u64::MAX);

        // ---- Parsing / hashing with batched reconcile ----
        transition(&mut progress, ScanState::Parsing)?;
        emitter.emit(&progress, true)?;
        durable::library_id(self.deps, root)?;
        let planner = Mutex::new(RelinkPlanner::new(durable::song_snapshot(self.deps, root)?));
        let seen: HashSet<String> = audio
            .iter()
            .map(|path| path.identity_key().to_owned())
            .collect();
        let mut cancelled = false;
        for batch in audio.chunks(self.deps.config.batch_size.max(1)) {
            if token.is_cancelled() {
                cancelled = true;
                break;
            }
            let outcomes = parse_bounded(self.deps, root, batch, token);
            cancelled |= token.is_cancelled();
            self.reconcile(root, generation, &outcomes, &planner, &mut progress)?;
            emitter.emit(&progress, false)?;
        }

        // ---- Cancelled: persist the terminal state, no missing pass ----
        if cancelled || token.is_cancelled() {
            self.finish(root, generation, &mut progress, ScanState::Cancelled)?;
            return Ok(ScanSummary {
                generation,
                cancelled: true,
                progress,
            });
        }

        // ---- Final missing pass (fully completed runs only) ----
        transition(&mut progress, ScanState::Reconciling)?;
        emitter.emit(&progress, true)?;
        if token.is_cancelled() {
            self.finish(root, generation, &mut progress, ScanState::Cancelled)?;
            return Ok(ScanSummary {
                generation,
                cancelled: true,
                progress,
            });
        }
        self.mark_missing(root, &seen, &mut progress)?;
        self.finish(root, generation, &mut progress, ScanState::Completed)?;
        Ok(ScanSummary {
            generation,
            cancelled: false,
            progress,
        })
    }

    fn scan_candidates(&self, root: LibraryRootId) -> Result<Vec<RelativeMediaPath>, Error> {
        let mut candidates = self.deps.fs.enumerate(root)?;
        let locations = self.deps.songs.published_file_paths(root)?;
        let owned: HashSet<_> = self
            .deps
            .songs
            .all_in_root(root)?
            .iter()
            .map(|song| song.path().identity_key().to_owned())
            .collect();
        let retained: HashSet<_> = locations
            .iter()
            .filter(|(local, logical)| local != logical && !owned.contains(logical.identity_key()))
            .map(|(_, logical)| logical.identity_key().to_owned())
            .collect();
        candidates.retain(|path| !retained.contains(path.identity_key()));
        let mut known: HashSet<_> = candidates
            .iter()
            .map(|path| path.identity_key().to_owned())
            .collect();
        for (local, _) in locations {
            // A missing current version must still enter the final missing
            // pass. Never discover unrelated historical Echo control files.
            if self.deps.fs.path_exists(root, &local)?
                && known.insert(local.identity_key().to_owned())
            {
                candidates.push(local);
            }
        }
        Ok(candidates)
    }

    /// Reconcile one parsed batch. Song writes go through the unit of work in
    /// small transactions; diagnostics are recorded per file.
    fn reconcile(
        &self,
        root: LibraryRootId,
        generation: u64,
        outcomes: &[FileOutcome],
        planner: &Mutex<RelinkPlanner>,
        progress: &mut ScanProgress,
    ) -> Result<(), Error> {
        for outcome in outcomes {
            match outcome {
                FileOutcome::FastSkip { path, restore } => {
                    progress.processed += 1;
                    progress.skipped += 1;
                    if let Some(id) = *restore {
                        // A fast-skipped file that came back: the fast-skip
                        // proved content parity, so restore without re-parse.
                        self.deps
                            .songs
                            .set_availability(id, SongAvailability::Available)?;
                        planner
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .restore_available(id);
                    }
                    let _ = path;
                }
                FileOutcome::Diagnostic(diagnostic) => {
                    progress.processed += 1;
                    progress.failed += 1;
                    // A bad file is a recorded issue, never a scan abort.
                    self.deps.runs.record_issue(root, generation, diagnostic)?;
                }
                FileOutcome::Parsed(parsed) => {
                    progress.processed += 1;
                    let resolution = planner
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .resolve(&parsed.file);
                    match resolution {
                        Resolution::Keep { song } | Resolution::Relink { song } => {
                            progress.updated += 1;
                            let revived = planner
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .revived(song);
                            let entity = planner
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .song(song);
                            self.apply_parsed(root, entity, parsed)?;
                            // The file was just parsed at this record's path, so
                            // presence is proven: the Missing -> Available flip
                            // must reach the row. `upsert_song` deliberately
                            // never writes `availability` (a stale snapshot must
                            // not roll it backward), so the revival goes through
                            // the dedicated mutation — the same shape the
                            // fast-skip branch uses. Without it, a
                            // continuation-projected library stays `missing`
                            // and therefore invisible forever: portable records
                            // carry no size/mtime, so those rows can never
                            // fast-skip, only land here.
                            if revived {
                                self.deps
                                    .songs
                                    .set_availability(song, SongAvailability::Available)?;
                            }
                        }
                        Resolution::Create => {
                            progress.created += 1;
                            let id = self.deps.ids.new_song_id();
                            let mut entity = Song::with_added_at(
                                id,
                                root,
                                parsed.file.path.clone(),
                                Revision::INITIAL,
                                self.wall_clock_ms(),
                            );
                            entity.apply_metadata(
                                parsed.file.meta.title.clone(),
                                parsed.file.meta.artist.clone(),
                                parsed.file.meta.album.clone(),
                                parsed.file.meta.duration,
                            );
                            entity.apply_scan_facts_with_params(
                                parsed.file.hash.clone(),
                                parsed.file.size,
                                parsed.file.mtime_ns,
                                parsed.file.meta.format,
                                parsed.file.meta.parameters,
                            );
                            planner
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .register_created(entity.clone());
                            self.apply_parsed(root, Some(entity), parsed)?;
                        }
                        Resolution::Duplicate { .. } => {
                            // Duplicate content: one record per hash; report
                            // the path, never mint a second UUID.
                            self.deps.runs.record_issue(
                                root,
                                generation,
                                &MediaDiagnostic::new(
                                    parsed.file.path.clone(),
                                    "duplicate_content",
                                    format!(
                                        "content hash already owned by another path: {}",
                                        parsed.file.hash
                                    ),
                                    false,
                                ),
                            )?;
                            progress.skipped += 1;
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Persist one parsed file's full state (song + lyrics + cover) in one
    /// transaction: the cache key and the database reference stay consistent
    /// (task 4.6).
    fn apply_parsed(
        &self,
        root: LibraryRootId,
        entity: Option<Song>,
        parsed: &ParsedOutcome,
    ) -> Result<(), Error> {
        let song = entity.unwrap_or_else(|| {
            Song::with_added_at(
                self.deps.ids.new_song_id(),
                root,
                parsed.file.path.clone(),
                Revision::INITIAL,
                self.wall_clock_ms(),
            )
        });
        if let Some(original) = durable::published_index_refresh(self.deps, &song, parsed)? {
            return self.persist_parsed_index(original, parsed, None);
        }
        let resources =
            super::scan_resources::verified_song_resources(self.deps, root, parsed, true)?;
        self.persist_parsed_index(song, parsed, Some(resources))
    }

    fn persist_parsed_index(
        &self,
        song: Song,
        parsed: &ParsedOutcome,
        resources: Option<Vec<crate::domain::sync::SharedResourceManifest>>,
    ) -> Result<(), Error> {
        // Lyrics candidates: embedded + sidecar. Absent sources are cleared —
        // a rescan must not leave stale text behind. Override rows are never
        // touched by scans (no override candidate is ever constructed here).
        let embedded = parsed
            .embedded_lyrics
            .clone()
            .map(|candidate| rewrap(&candidate, LyricsSource::Embedded));
        let sidecar = parsed
            .sidecar_lyrics
            .clone()
            .map(|candidate| rewrap(&candidate, LyricsSource::Sidecar));
        let cover = parsed.cover.clone();
        let durable_resources = resources.as_ref().map_or_else(Vec::new, |_| {
            vec![crate::domain::library::durability::DurableResource {
                relative_path: song.path().display().to_owned(),
                content_hash: parsed.file.hash.clone(),
                byte_length: parsed.file.size,
            }]
        });
        let entity = song.clone();
        let project = || {
            self.deps
                .uow
                .with_tx(Box::new(move |tx: &mut dyn TxAccess| {
                    if let Some(resources) = resources {
                        tx.set_song_file_resources(&song, &resources)?;
                        tx.upsert_song(&song)?;
                    } else {
                        tx.refresh_song_file_index(&song)?;
                    }
                    match embedded {
                        Some(candidate) => tx.set_lyrics_candidate(song.id(), &candidate)?,
                        None => tx.clear_lyrics_candidate(song.id(), LyricsSource::Embedded)?,
                    }
                    match sidecar {
                        Some(candidate) => tx.set_lyrics_candidate(song.id(), &candidate)?,
                        None => tx.clear_lyrics_candidate(song.id(), LyricsSource::Sidecar)?,
                    }
                    if let Some(cover) = cover {
                        tx.attach_cover(song.id(), &cover)?;
                    }
                    Ok(())
                }))
        };
        // Published remote metadata is already authoritative; refreshing its
        // derived index must not create a local mutation or rewrite its path.
        if durable_resources.is_empty() {
            project()
        } else {
            durable::commit_song(
                self.deps,
                &entity,
                self.deps.ids.new_operation_id(),
                crate::domain::library::durability::ChangeOrigin::Scan,
                durable_resources,
                project,
            )
        }
    }

    /// A new song's event fact is read from the injected backend clock at the
    /// persistence boundary, never derived from a revision or supplied by UI.
    fn wall_clock_ms(&self) -> u64 {
        self.deps
            .clock
            .now_wall()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX)
    }

    /// The final missing pass: available songs whose path a *fully completed*
    /// enumeration did not see are marked missing. Cancelled and failed runs
    /// never reach this point (design §6.5: 取消/枚举失败不得批量误删).
    fn mark_missing(
        &self,
        root: LibraryRootId,
        seen: &HashSet<String>,
        progress: &mut ScanProgress,
    ) -> Result<(), Error> {
        for song in self.deps.songs.all_in_root(root)? {
            if song.availability() != SongAvailability::Available {
                continue;
            }
            if !seen.contains(song.path().identity_key()) {
                self.deps
                    .songs
                    .set_availability(song.id(), SongAvailability::Missing)?;
                progress.missing += 1;
            }
        }
        Ok(())
    }

    fn finish(
        &self,
        root: LibraryRootId,
        generation: u64,
        progress: &mut ScanProgress,
        state: ScanState,
    ) -> Result<(), Error> {
        transition(progress, state)?;
        // Terminal snapshots persist unthrottled: never lost (task 4.10).
        self.deps.runs.finish_run(root, generation, state, progress)
    }
}

#[path = "parse.rs"]
mod parse;

pub use parse::{is_supported_extension, CancelScan, RelinkLibrary};
pub(crate) use parse::{
    parse_bounded, parse_file_event, parse_single_file, parse_single_file_with_content_check,
    rewrap, transition, FileOutcome, ParsedOutcome, ProgressEmitter,
};

#[cfg(test)]
mod tests;
