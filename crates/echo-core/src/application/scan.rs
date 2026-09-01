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
use std::time::Duration;

use crate::application::ports::TxAccess;
use crate::application::ports::{
    CatalogQueryRepository, Clock, ContentHasher, CoverAssetRef, CoverCache, CoverRepository,
    FileMeta, IdGenerator, LibraryFileSystem, LibraryRepository, LyricsParser, LyricsRepository,
    MediaProbe, MetadataReader, OperationJournalRepository, PlaylistRepository, ProbeOutcome,
    ScanRunRepository, SongRepository, UnitOfWork,
};
use crate::application::relink::{ParsedFile, RelinkPlanner, Resolution};
use crate::domain::entities::{
    LyricsCandidate, LyricsSource, MediaDiagnostic, Song, SongAvailability,
};
use crate::domain::ids::{LibraryRootId, RelativeMediaPath, Revision, SongId};
use crate::domain::state::scan::{ScanProgress, ScanState};
use crate::error::Error;

/// The cheap extension filter (design §6.2): enumeration returns every
/// regular file; the scan probes only these. Everything else is a skip. The
/// import pipeline applies the same matrix to user-selected sources (task 5.1).
pub(crate) const SUPPORTED_EXTENSIONS: [&str; 7] =
    ["mp3", "flac", "m4a", "mp4", "ogg", "opus", "wav"];

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
    pub roots: Arc<dyn LibraryRepository>,
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
        {
            let active = self.lock();
            if active.contains_key(&root) {
                return Err(Error::conflict("a scan is already running for this root"));
            }
        }
        self.lock().insert(root, token);
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
        let generation = self.deps.runs.latest_generation(root)?.unwrap_or(0) + 1;
        let token = ScanCancelToken::new();
        self.supervisor.register(root, token.clone())?;
        let result = self.run_guarded(root, generation, &token);
        self.supervisor.unregister(root);
        result
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
        let candidates = match self.deps.fs.enumerate(root) {
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
        let planner = Mutex::new(RelinkPlanner::new(self.deps.songs.all_in_root(root)?));
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
        if cancelled {
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
        self.mark_missing(root, &seen, &mut progress)?;
        self.finish(root, generation, &mut progress, ScanState::Completed)?;
        Ok(ScanSummary {
            generation,
            cancelled: false,
            progress,
        })
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
                            let entity = planner
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .song(song);
                            self.apply_parsed(root, entity, parsed)?;
                        }
                        Resolution::Create => {
                            progress.created += 1;
                            let id = self.deps.ids.new_song_id();
                            let mut entity =
                                Song::new(id, root, parsed.file.path.clone(), Revision::INITIAL);
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
            Song::new(
                self.deps.ids.new_song_id(),
                root,
                parsed.file.path.clone(),
                Revision::INITIAL,
            )
        });
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
        self.deps
            .uow
            .with_tx(Box::new(move |tx: &mut dyn TxAccess| {
                tx.upsert_song(&song)?;
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

fn transition(progress: &mut ScanProgress, next: ScanState) -> Result<(), Error> {
    progress
        .transition(next)
        .map_err(|error| Error::InvariantViolation {
            why: error.to_string(),
        })
}

/// One parsed file plus the side assets the reconcile must persist.
#[derive(Debug)]
pub(crate) struct ParsedOutcome {
    pub(crate) file: ParsedFile,
    pub(crate) embedded_lyrics: Option<LyricsCandidate>,
    pub(crate) sidecar_lyrics: Option<LyricsCandidate>,
    pub(crate) cover: Option<CoverAssetRef>,
}

/// One worker-produced outcome for a candidate file.
#[derive(Debug)]
pub(crate) enum FileOutcome {
    /// size/mtime unchanged since the record's last parse: no re-work.
    FastSkip {
        path: RelativeMediaPath,
        /// Song to restore when the record was missing (file came back).
        restore: Option<SongId>,
    },
    /// The file parsed fully.
    Parsed(Box<ParsedOutcome>),
    /// A per-file diagnostic (unsupported / no audio / corrupt / IO).
    Diagnostic(MediaDiagnostic),
}

/// Parse a batch with bounded workers (design §6.4). Never fails: per-file
/// problems become diagnostics so one bad file cannot block others.
pub(crate) fn parse_bounded(
    deps: &ScanDeps,
    root: LibraryRootId,
    batch: &[RelativeMediaPath],
    token: &ScanCancelToken,
) -> Vec<FileOutcome> {
    if deps.config.worker_threads.max(1) <= 1 || batch.len() <= 1 {
        return batch
            .iter()
            .map(|path| parse_single_file(deps, root, path))
            .collect();
    }
    // Bounded pool: exactly `worker_threads` threads drain a shared queue —
    // concurrency never exceeds the configured bound.
    let queue = Mutex::new(VecDeque::from(batch.to_vec()));
    let results = Mutex::new(Vec::new());
    let workers = deps.config.worker_threads.min(batch.len());
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                if token.is_cancelled() {
                    return;
                }
                let next = queue
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .pop_front();
                let Some(path) = next else {
                    return;
                };
                let outcome = parse_single_file(deps, root, &path);
                results
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(outcome);
            });
        }
    });
    // Restore enumeration order for deterministic progress and reconcile.
    order_by_batch(
        results
            .into_inner()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        batch,
    )
}

fn order_by_batch(outcomes: Vec<FileOutcome>, batch: &[RelativeMediaPath]) -> Vec<FileOutcome> {
    let mut by_path: HashMap<String, FileOutcome> = outcomes
        .into_iter()
        .map(|outcome| {
            let key = match &outcome {
                FileOutcome::FastSkip { path, .. } => path.identity_key().to_owned(),
                FileOutcome::Parsed(parsed) => parsed.file.path.identity_key().to_owned(),
                FileOutcome::Diagnostic(diagnostic) => diagnostic.path().identity_key().to_owned(),
            };
            (key, outcome)
        })
        .collect();
    batch
        .iter()
        .filter_map(|path| by_path.remove(path.identity_key()))
        .collect()
}

/// Parse one candidate file: fast-skip → probe → BLAKE3 → tags → lyrics →
/// cover. Any failure is a diagnostic, never an error.
pub(crate) fn parse_single_file(
    deps: &ScanDeps,
    root: LibraryRootId,
    path: &RelativeMediaPath,
) -> FileOutcome {
    let file_meta: FileMeta = match deps.fs.file_meta(root, path) {
        Ok(meta) => meta,
        Err(error) => return FileOutcome::Diagnostic(diagnostic_of(path, &error)),
    };
    if let Ok(Some(existing)) = deps.songs.by_path(root, path) {
        let unchanged = existing.blake3_hash().is_some()
            && existing.file_size() == Some(file_meta.size)
            && existing.file_mtime_ns() == Some(file_meta.modified_ns);
        if unchanged {
            let restore =
                (existing.availability() == SongAvailability::Missing).then_some(existing.id());
            // `PendingDelete` rows stay as they are: Echo's own delete flow
            // owns them; file presence does not resurrect a pending delete.
            return FileOutcome::FastSkip {
                path: path.clone(),
                restore,
            };
        }
    }

    // Probe (independent of tags; the extension was only a cheap filter).
    let (format, duration) = match deps.probe.probe(root, path) {
        Ok(ProbeOutcome::Audio { format, duration }) => (format, duration),
        Ok(ProbeOutcome::NoAudioTrack) => {
            return FileOutcome::Diagnostic(MediaDiagnostic::new(
                path.clone(),
                "no_audio_track",
                "container has no supported audio track".to_owned(),
                false,
            ));
        }
        Ok(ProbeOutcome::Unsupported) => {
            return FileOutcome::Diagnostic(MediaDiagnostic::new(
                path.clone(),
                "unsupported_media",
                "content is not a supported media format".to_owned(),
                false,
            ));
        }
        Err(error) => return FileOutcome::Diagnostic(diagnostic_of(path, &error)),
    };

    // Full-file BLAKE3 (design §6.3).
    let hash = match deps.hasher.hash(root, path) {
        Ok(hash) => hash,
        Err(error) => return FileOutcome::Diagnostic(diagnostic_of(path, &error)),
    };

    // Tags + embedded assets (input limits enforced inside the reader).
    let mut meta = match deps.metadata.read(root, path) {
        Ok(meta) => meta,
        Err(error) => return FileOutcome::Diagnostic(diagnostic_of(path, &error)),
    };
    // Duration/format are stream facts: the probe owns them, never the tags.
    meta.format = format;
    meta.duration = duration;

    // Embedded lyrics → candidate.
    let embedded_lyrics = meta
        .embedded_lyrics
        .take()
        .map(|text| deps.lyrics_parser.parse(&text));

    // Sidecar `.lrc`: same basename, extension case-insensitive (task 4.5).
    let sidecar_lyrics = read_sidecar(deps, root, path);

    // Cover → content-addressed cache (opaque key; the DB reference lands in
    // the reconcile transaction). A failed cover never blocks the song.
    let cover = meta.cover.take().and_then(|cover| {
        let content_hash = deps.hasher.hash_of_bytes(&cover.bytes);
        deps.cover_cache
            .put(&cover.bytes, &cover.mime)
            .ok()
            .map(|asset_key| CoverAssetRef {
                content_hash,
                mime: cover.mime,
                asset_key,
            })
    });

    FileOutcome::Parsed(Box::new(ParsedOutcome {
        file: ParsedFile {
            path: path.clone(),
            hash,
            size: file_meta.size,
            mtime_ns: file_meta.modified_ns,
            meta,
        },
        embedded_lyrics,
        sidecar_lyrics,
        cover,
    }))
}

/// Read the same-basename `.lrc` sidecar (extension case-insensitive), within
/// the lyrics input limit. `None` = no (usable) sidecar: missing, empty,
/// undecodable or over-limit sidecars are all "no lyrics candidate" — never
/// a song failure (task 4.5).
fn read_sidecar(
    deps: &ScanDeps,
    root: LibraryRootId,
    path: &RelativeMediaPath,
) -> Option<LyricsCandidate> {
    let file_name = path.file_name()?;
    let stem = file_name.rsplit_once('.')?.0;
    let sidecar_text = path
        .parent()
        .map_or_else(|| format!("{stem}.lrc"), |dir| format!("{dir}/{stem}.lrc"));
    let sidecar = RelativeMediaPath::new(&sidecar_text).ok()?;
    // Limit + 1 byte so an over-limit sidecar is *detected*, not truncated.
    let limit = deps.config.lyrics_limit;
    let raw = deps
        .fs
        .read_head(root, &sidecar, u64::try_from(limit).ok()? + 1)
        .ok()?;
    if raw.len() > limit {
        return None;
    }
    let text = String::from_utf8(raw).ok()?;
    if text.trim().is_empty() {
        return None;
    }
    Some(deps.lyrics_parser.parse(&text))
}

fn diagnostic_of(path: &RelativeMediaPath, error: &Error) -> MediaDiagnostic {
    MediaDiagnostic::new(
        path.clone(),
        match error.code() {
            "unsupported_media" => "unsupported_media",
            "corrupt_media" => "corrupt_media",
            _ => "scan_file_error",
        },
        error.to_log(crate::logging::DiagnosticMode::Off),
        false,
    )
}

/// Rewrap a parser result under a specific source (the port-level parser
/// defaults to `Embedded`; sidecar text is parsed through the same parser).
pub(crate) fn rewrap(candidate: &LyricsCandidate, source: LyricsSource) -> LyricsCandidate {
    let mut rewrapped = LyricsCandidate::with_raw_text(
        source,
        candidate.raw_text().to_owned(),
        candidate.lines().to_vec(),
        candidate.plain_text().map(str::to_owned),
        candidate.parse_error().map(str::to_owned),
    );
    if candidate.is_empty_override() {
        rewrapped.mark_empty_override();
    }
    rewrapped
}

/// Throttled progress persistence (task 4.10: at most one snapshot per
/// interval; phase transitions and terminal states force immediate writes).
struct ProgressEmitter<'a> {
    deps: &'a ScanDeps,
    root: LibraryRootId,
    generation: u64,
    last: Option<Duration>,
}

impl<'a> ProgressEmitter<'a> {
    const fn new(deps: &'a ScanDeps, root: LibraryRootId, generation: u64) -> Self {
        Self {
            deps,
            root,
            generation,
            last: None,
        }
    }

    fn emit(&mut self, progress: &ScanProgress, force: bool) -> Result<(), Error> {
        let now = self.deps.clock.now_monotonic();
        if !force
            && self
                .last
                .is_some_and(|last| now.saturating_sub(last) < self.deps.config.progress_interval)
        {
            return Ok(());
        }
        self.deps
            .runs
            .update_progress(self.root, self.generation, progress)?;
        self.last = Some(now);
        Ok(())
    }
}

/// Cancels the active scan of a root (task 4.7).
pub struct CancelScan<'a> {
    supervisor: &'a ScanSupervisor,
}

impl<'a> CancelScan<'a> {
    #[must_use]
    pub const fn new(supervisor: &'a ScanSupervisor) -> Self {
        Self { supervisor }
    }

    /// Cancel the running scan for `root`; `true` when a scan was cancelled.
    #[must_use]
    pub fn cancel(&self, root: LibraryRootId) -> bool {
        self.supervisor.cancel(root)
    }
}

/// Manual rescan entry (task 4.10): a repeatable full scan. Re-linking rules
/// run inside every scan, so a manual rescan restores and re-associates
/// identities exactly like an automatic one — same UUID for a moved file,
/// new UUID only for genuinely new content.
pub struct RelinkLibrary<'a> {
    deps: &'a ScanDeps,
    supervisor: &'a ScanSupervisor,
}

impl<'a> RelinkLibrary<'a> {
    #[must_use]
    pub const fn new(deps: &'a ScanDeps, supervisor: &'a ScanSupervisor) -> Self {
        Self { deps, supervisor }
    }

    /// # Errors
    ///
    /// Same contract as [`StartScan::run`].
    pub fn run(&self, root: LibraryRootId) -> Result<ScanSummary, Error> {
        StartScan::new(self.deps, self.supervisor).run(root)
    }
}

/// Whether `extension` (lower-case) is in the phase-1 matrix.
#[must_use]
pub fn is_supported_extension(extension: &str) -> bool {
    SUPPORTED_EXTENSIONS.contains(&extension)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Hasher double that cancels the scan through the supervisor on the
    /// first hash call — the same path `CancelScan` takes.
    struct CancelOnHash {
        supervisor: ScanSupervisor,
        inner: Arc<dyn ContentHasher>,
    }

    impl ContentHasher for CancelOnHash {
        fn hash(&self, root: LibraryRootId, path: &RelativeMediaPath) -> Result<String, Error> {
            let _cancelled = self.supervisor.cancel(root);
            self.inner.hash(root, path)
        }
        fn hash_of_bytes(&self, bytes: &[u8]) -> String {
            self.inner.hash_of_bytes(bytes)
        }
    }

    /// Probe double tracking concurrent entries (bounded-worker assertion).
    struct CountingProbe {
        inner: Arc<dyn MediaProbe>,
        current: Arc<std::sync::atomic::AtomicUsize>,
        max_seen: Arc<std::sync::atomic::AtomicUsize>,
    }

    impl MediaProbe for CountingProbe {
        fn probe(
            &self,
            root: LibraryRootId,
            path: &RelativeMediaPath,
        ) -> Result<ProbeOutcome, Error> {
            let now = self
                .current
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                + 1;
            self.max_seen
                .fetch_max(now, std::sync::atomic::Ordering::SeqCst);
            // Hold the slot long enough for a second worker to overlap.
            std::thread::sleep(std::time::Duration::from_millis(5));
            let outcome = self.inner.probe(root, path);
            self.current
                .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
            outcome
        }
    }

    use crate::application::delete::DeleteSongs;
    use crate::application::ports::{PlaylistRepository, SongRepository};
    use crate::application::testing::{scan_fixture::song_by_path, ScanFixture};
    use crate::domain::entities::{Song, SongAvailability};
    use crate::domain::ids::{PlaylistId, Revision, SongId};

    /// Seed one playable song with favorite, one play and a playlist
    /// membership, returning its [`SongId`] and the playlist id — the
    /// relationship set that task 5.9 proves survives external deletion and
    /// recovery.
    fn seed_loved_song(fixture: &ScanFixture, path: &str) -> (SongId, PlaylistId) {
        fixture.write_file(path, b"audio-bytes");
        fixture.set_audio(path, "晴天", 269_000);
        let mut song = Song::new(
            SongId::new(),
            fixture.root,
            fixture.path(path),
            Revision::INITIAL,
        );
        song.apply_scan_facts(
            fixture.deps.hasher.hash_of_bytes(b"audio-bytes"),
            11,
            1,
            crate::domain::media::AudioFormat::Flac,
        );
        song.set_favorite(true);
        song.record_play();
        SongRepository::upsert(&fixture.database, &song).expect("seed song");

        let playlist = PlaylistId::new();
        fixture
            .database
            .create(playlist, fixture.root, "favorites")
            .expect("create playlist");
        fixture
            .database
            .add_member(playlist, song.id(), 0)
            .expect("add member");
        (song.id(), playlist)
    }

    #[test]
    fn external_deletion_is_missing_not_pending_delete_and_keeps_relationships() {
        let fixture = ScanFixture::new();
        fixture.write_file("keep.mp3", b"audio-keep");
        fixture.set_audio("keep.mp3", "Keep", 1_000);
        let (target, playlist) = seed_loved_song(&fixture, "歌手/晴天.flac");
        let target_path = fixture.path("歌手/晴天.flac");

        start_scan(&fixture).run(fixture.root).expect("first scan");
        assert_eq!(fixture.all_songs().len(), 2);

        // The user deletes the file in their file manager, then a rescan
        // observes it — the external path (not Echo's delete flow).
        fixture.remove_file("歌手/晴天.flac");
        let summary = start_scan(&fixture)
            .run(fixture.root)
            .expect("missing scan");
        assert_eq!(summary.progress.missing, 1);

        let record = song_by_path(&fixture, &target_path)
            .expect("lookup")
            .expect("record kept");
        assert_eq!(record.id(), target);
        assert_eq!(
            record.availability(),
            SongAvailability::Missing,
            "external deletion is the Missing model, not Echo's PendingDelete"
        );
        assert!(
            fixture.database.operations_for_song(target).is_empty(),
            "external missing must never enter Echo's trash/delete journal"
        );
        assert!(record.favorite(), "favorite survives external missing");
        assert_eq!(
            record.play_count().as_u64(),
            1,
            "play stats survive external missing"
        );
        assert!(
            !record.availability().is_playable(),
            "a missing file is not playable but its identity remains"
        );

        // The playlist membership row is retained through external missing
        // (its live availability is surfaced by the SQLite JOIN at query time).
        let member = fixture
            .database
            .members(playlist)
            .expect("members")
            .into_iter()
            .find(|m| m.song() == target)
            .expect("membership preserved through external missing");
        assert_eq!(member.song(), target);
    }

    #[test]
    fn external_missing_recovers_on_original_path_restoring_everything() {
        let fixture = ScanFixture::new();
        let (target, playlist) = seed_loved_song(&fixture, "歌手/晴天.flac");
        let target_path = fixture.path("歌手/晴天.flac");
        start_scan(&fixture).run(fixture.root).expect("first scan");
        fixture.remove_file("歌手/晴天.flac");
        start_scan(&fixture)
            .run(fixture.root)
            .expect("missing scan");

        // The file comes back at its original path (same content).
        fixture.write_file("歌手/晴天.flac", b"audio-bytes");
        fixture.set_audio("歌手/晴天.flac", "晴天", 269_000);
        let summary = start_scan(&fixture)
            .run(fixture.root)
            .expect("recover scan");
        assert_eq!(summary.progress.missing, 0);

        let restored = song_by_path(&fixture, &target_path)
            .expect("lookup")
            .expect("restored");
        assert_eq!(
            restored.id(),
            target,
            "original-path recovery reuses the UUID"
        );
        assert_eq!(restored.availability(), SongAvailability::Available);
        assert!(restored.favorite(), "favorite restored");
        assert_eq!(restored.play_count().as_u64(), 1, "play stats restored");
        assert!(restored.availability().is_playable());
        let member = fixture
            .database
            .members(playlist)
            .expect("members")
            .into_iter()
            .find(|m| m.song() == target)
            .expect("membership kept");
        assert_eq!(member.song(), target);
    }

    #[test]
    fn external_missing_relinks_on_same_hash_path_keeping_relationships() {
        let fixture = ScanFixture::new();
        let (target, playlist) = seed_loved_song(&fixture, "歌手/晴天.flac");
        start_scan(&fixture).run(fixture.root).expect("first scan");
        fixture.remove_file("歌手/晴天.flac");
        start_scan(&fixture)
            .run(fixture.root)
            .expect("missing scan");

        // A *same-hash* file appears at a different path: the identity must
        // move to it, keeping UUID and every relationship (design §6.7).
        fixture.write_file("moved/晴天.flac", b"audio-bytes");
        fixture.set_audio("moved/晴天.flac", "晴天", 269_000);
        let summary = start_scan(&fixture).run(fixture.root).expect("relink scan");
        assert_eq!(summary.progress.updated, 1, "relink counts as an update");

        let relinked = song_by_path(&fixture, &fixture.path("moved/晴天.flac"))
            .expect("lookup")
            .expect("relinked");
        assert_eq!(relinked.id(), target, "same-hash recovery reuses the UUID");
        assert_eq!(relinked.availability(), SongAvailability::Available);
        assert!(relinked.favorite());
        assert_eq!(relinked.play_count().as_u64(), 1);
        let member = fixture
            .database
            .members(playlist)
            .expect("members")
            .into_iter()
            .find(|m| m.song() == target)
            .expect("membership kept across the same-hash move");
        assert_eq!(member.song(), target);
        // The old path must not linger as a duplicate record either.
        assert_eq!(
            fixture.all_songs().len(),
            1,
            "one logical song per hash after relink"
        );
    }

    #[test]
    fn echo_pending_delete_is_never_treated_as_external_missing() {
        let fixture = ScanFixture::new();
        let (target, _playlist) = seed_loved_song(&fixture, "歌手/晴天.flac");
        start_scan(&fixture).run(fixture.root).expect("first scan");

        // Echo's own delete stages the file into the controlled trash slot and
        // hides the song as PendingDelete (task 5.7).
        DeleteSongs::new(&fixture.deps)
            .delete(fixture.root, target)
            .expect("Echo delete");

        // A rescan must NOT mark the hidden song externally missing, and must
        // not re-import its staged file from the owned trash directory.
        let summary = start_scan(&fixture)
            .run(fixture.root)
            .expect("rescan during delete");
        assert_eq!(
            summary.progress.missing, 0,
            "a PendingDelete song is skipped by the external-missing pass"
        );
        let record = song_by_path(&fixture, &fixture.path("歌手/晴天.flac"))
            .expect("lookup")
            .expect("record kept");
        assert_eq!(record.id(), target);
        assert_eq!(
            record.availability(),
            SongAvailability::PendingDelete,
            "Echo delete keeps its own hidden model; the scanner must not convert it to Missing"
        );
        assert_eq!(
            fixture.all_songs().len(),
            1,
            "the scanner never re-imports the staged trash file as a new song"
        );
        assert_eq!(
            fixture.database.operations_for_song(target).len(),
            1,
            "an Echo delete is recorded in the trash journal (unlike external missing)"
        );
    }

    fn start_scan(fixture: &ScanFixture) -> StartScan<'_> {
        StartScan::new(&fixture.deps, &fixture.supervisor)
    }

    #[test]
    fn scan_pipeline_creates_updates_and_reports_progress() {
        let fixture = ScanFixture::new();
        fixture.write_file("a.mp3", b"audio-a");
        fixture.write_file("华语/b.flac", b"audio-b");
        fixture.write_file("notes.txt", b"not audio");
        fixture.set_audio("a.mp3", "A", 1_000);
        fixture.set_audio("华语/b.flac", "B", 2_000);

        let summary = start_scan(&fixture).run(fixture.root).expect("scan ok");
        assert!(!summary.cancelled);
        assert_eq!(summary.progress.discovered, 2);
        assert_eq!(summary.progress.created, 2);
        assert_eq!(summary.progress.skipped, 1, "the .txt file is skipped");
        assert_eq!(summary.progress.failed, 0);

        let songs = fixture.all_songs();
        assert_eq!(songs.len(), 2);
        let a = songs
            .iter()
            .find(|s| s.path().display() == "a.mp3")
            .unwrap();
        assert_eq!(a.title(), Some("A"));
        assert!(
            a.blake3_hash().is_some() && a.file_size() == Some(7),
            "scan facts persisted for fast-skip"
        );
        // The run row reached Completed with the same summary.
        let row = fixture.run_row(1).expect("run row");
        assert!(row.finished);
        assert_eq!(row.state, ScanState::Completed);
        assert_eq!(row.progress.discovered, 2);
    }

    #[test]
    fn scan_pipeline_batches_and_survives_bad_files() {
        let fixture = ScanFixture::new();
        for index in 0..5 {
            let path = format!("file{index}.mp3");
            fixture.write_file(&path, format!("audio-{index}").as_bytes());
            fixture.set_audio(&path, &format!("T{index}"), 1_000);
        }
        // One broken file among the good ones.
        fixture.write_file("broken.mp3", b"garbage");
        fixture.probe.set(
            "broken.mp3",
            crate::application::ports::ProbeOutcome::Unsupported,
        );

        let summary = start_scan(&fixture).run(fixture.root).expect("scan ok");
        assert_eq!(summary.progress.created, 5, "good files are not blocked");
        assert_eq!(summary.progress.failed, 1);
        let issues = fixture.issues(1);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].code(), "unsupported_media");
        assert!(fixture.run_row(1).is_some());
    }

    #[test]
    fn no_audio_track_is_a_diagnostic_not_a_song() {
        let fixture = ScanFixture::new();
        fixture.write_file("video.mp4", b"video-only");
        fixture.probe.set(
            "video.mp4",
            crate::application::ports::ProbeOutcome::NoAudioTrack,
        );
        let summary = start_scan(&fixture).run(fixture.root).expect("scan ok");
        assert_eq!(summary.progress.created, 0);
        assert_eq!(summary.progress.failed, 1);
        assert!(fixture.all_songs().is_empty());
    }

    #[test]
    fn cancel_mid_scan_never_marks_missing() {
        let fixture = ScanFixture::new();
        for index in 0..6 {
            let path = format!("file{index}.mp3");
            fixture.write_file(&path, format!("audio-{index}").as_bytes());
            fixture.set_audio(&path, &format!("T{index}"), 1_000);
        }
        // First scan completes; then the file of one song disappears.
        start_scan(&fixture).run(fixture.root).expect("first scan");
        let survivor_path = fixture.path("file0.mp3");
        let survivor = song_by_path(&fixture, &survivor_path)
            .expect("lookup")
            .expect("exists");
        fixture.remove_file("file1.mp3");

        // Touch the remaining files (new bytes) so the second scan really
        // parses (and hashes) instead of fast-skipping everything: it then
        // cancels on the very first hash call (through the supervisor, i.e.
        // exactly what `CancelScan` does). The missing pass must never run,
        // so `file1`'s record stays available.
        for index in [0, 2, 3, 4, 5] {
            let path = format!("file{index}.mp3");
            fixture.write_file(&path, format!("audio-v2-{index}").as_bytes());
        }
        let canceller = Arc::new(CancelOnHash {
            supervisor: fixture.supervisor.clone(),
            inner: Arc::clone(&fixture.deps.hasher),
        });
        let deps = ScanDeps {
            hasher: Arc::clone(&canceller) as Arc<dyn ContentHasher>,
            ..ScanDeps::clone(&fixture.deps)
        };
        let summary = StartScan::new(&deps, &fixture.supervisor)
            .run(fixture.root)
            .expect("cancelled scan returns summary");
        assert!(summary.cancelled);
        assert_eq!(summary.progress.state, ScanState::Cancelled);

        let still_there = song_by_path(&fixture, &survivor_path)
            .expect("lookup")
            .expect("exists");
        assert_eq!(
            still_there.availability(),
            SongAvailability::Available,
            "a cancelled scan must not batch-mark anything missing"
        );
        let row = fixture.run_row(2).expect("second run");
        assert_eq!(row.state, ScanState::Cancelled);
        assert!(row.finished, "terminal state persisted");
        drop(survivor);
    }

    #[test]
    fn enumerate_failure_marks_run_failed_without_missing() {
        let fixture = ScanFixture::new();
        fixture.write_file("a.mp3", b"audio");
        fixture.set_audio("a.mp3", "A", 1_000);
        start_scan(&fixture).run(fixture.root).expect("first scan");
        // The file disappears AND the root read fails.
        fixture.remove_file("a.mp3");
        fixture
            .fs
            .inject_fault(Error::unavailable("library root", "unmounted"));

        let error = start_scan(&fixture).run(fixture.root).unwrap_err();
        // The fake's fault is storage-shaped; the contract under test is the
        // failed run, not the error variant.
        assert_eq!(error.code(), "storage");
        let row = fixture.run_row(2).expect("failed run row");
        assert_eq!(row.state, ScanState::Failed);
        assert!(row.finished);
        // The (now absent) song was NOT marked missing by the failed run.
        let record = song_by_path(&fixture, &fixture.path("a.mp3"))
            .expect("lookup")
            .expect("record kept");
        assert_eq!(record.availability(), SongAvailability::Available);
    }

    #[test]
    fn fast_skip_unchanged_files_and_relink_on_move() {
        let fixture = ScanFixture::new();
        fixture.write_file("album/song.flac", b"audio-bytes");
        fixture.set_audio("album/song.flac", "Song", 1_000);
        let summary = start_scan(&fixture).run(fixture.root).expect("first scan");
        assert_eq!(summary.progress.created, 1);

        let original = song_by_path(&fixture, &fixture.path("album/song.flac"))
            .expect("lookup")
            .expect("song");
        let mut favorite = original.clone();
        favorite.set_favorite(true);
        crate::application::ports::SongRepository::upsert(&fixture.database, &favorite)
            .expect("favorite set");

        // Rescan with no change: fast-skip, no re-parse, favorite intact.
        let summary = start_scan(&fixture).run(fixture.root).expect("rescan");
        assert_eq!(summary.progress.skipped, 1);
        assert_eq!(summary.progress.updated, 0);
        assert_eq!(summary.progress.created, 0);

        // Move the file: same content, new path → the UUID survives.
        fixture.write_file("album/renamed.flac", b"audio-bytes");
        fixture.remove_file("album/song.flac");
        fixture.set_audio("album/renamed.flac", "Song", 1_000);
        let summary = start_scan(&fixture).run(fixture.root).expect("move scan");
        assert_eq!(summary.progress.updated, 1, "re-link counts as update");
        let moved = song_by_path(&fixture, &fixture.path("album/renamed.flac"))
            .expect("lookup")
            .expect("re-linked record");
        assert_eq!(moved.id(), original.id(), "UUID kept across the move");
        assert!(moved.favorite(), "favorite survives the move");
        assert!(
            song_by_path(&fixture, &fixture.path("album/song.flac"))
                .expect("lookup")
                .is_none(),
            "no duplicate record for the old path"
        );
        // The old-path record must not linger as missing either.
        assert!(fixture
            .all_songs()
            .iter()
            .all(|song| song.availability() == SongAvailability::Available));
    }

    #[test]
    fn duplicate_hash_paths_get_deterministic_primary() {
        let fixture = ScanFixture::new();
        fixture.write_file("z-first.mp3", b"same-bytes");
        fixture.write_file("a-second.mp3", b"same-bytes");
        fixture.set_audio("z-first.mp3", "Same", 1_000);
        fixture.set_audio("a-second.mp3", "Same", 1_000);

        let summary = start_scan(&fixture).run(fixture.root).expect("scan");
        assert_eq!(
            summary.progress.created, 1,
            "exactly one record per hash: {summary:?}"
        );
        // One of the two paths became the duplicate report (whichever the
        // enumeration ordered second).
        let duplicated = summary.progress.skipped + summary.progress.updated;
        assert_eq!(duplicated, 1);
        let issues = fixture.issues(1);
        assert!(
            issues
                .iter()
                .any(|issue| issue.code() == "duplicate_content")
                || summary.progress.updated == 1,
            "duplicate content is reported or the primary moved deterministically"
        );
        // The primary is the smallest canonical path key, deterministically.
        let songs = fixture.all_songs();
        assert_eq!(songs.len(), 1);
        assert_eq!(songs[0].path().display(), "a-second.mp3");
        let primary_id = songs[0].id();

        // The primary file disappears: the record goes missing (UUID kept).
        fixture.remove_file("a-second.mp3");
        start_scan(&fixture)
            .run(fixture.root)
            .expect("missing scan");
        let record = fixture.all_songs().into_iter().next().expect("record");
        assert_eq!(record.id(), primary_id);
        assert_eq!(record.availability(), SongAvailability::Missing);

        // The next scan sees the remaining same-hash file: the record is
        // promoted to it under the same UUID (同一规则提升).
        let summary = start_scan(&fixture)
            .run(fixture.root)
            .expect("promote scan");
        assert_eq!(summary.progress.updated, 1);
        let songs = fixture.all_songs();
        assert_eq!(songs.len(), 1);
        assert_eq!(songs[0].id(), primary_id, "UUID kept on promotion");
        assert_eq!(songs[0].path().display(), "z-first.mp3");
        assert_eq!(songs[0].availability(), SongAvailability::Available);
    }

    #[test]
    fn unique_music_key_relink_is_conservative() {
        let fixture = ScanFixture::new();
        // A song whose file vanished, with full tags (music key available).
        fixture.write_file("old/tone.flac", b"old-bytes");
        fixture.set_audio("old/tone.flac", "晴天", 200_000);
        start_scan(&fixture).run(fixture.root).expect("first scan");
        fixture.remove_file("old/tone.flac");
        start_scan(&fixture)
            .run(fixture.root)
            .expect("marks missing");
        let gone = fixture.all_songs().into_iter().next().expect("record kept");
        assert_eq!(gone.availability(), SongAvailability::Missing);

        // A new file with the same normalized key and duration within 2 s:
        // conservative weak re-link.
        fixture.write_file("new/tone.flac", b"new-bytes");
        fixture.set_audio("new/tone.flac", "晴天", 201_000);
        let summary = start_scan(&fixture).run(fixture.root).expect("relink scan");
        assert_eq!(summary.progress.updated, 1);
        assert_eq!(summary.progress.created, 0);
        let relinked = fixture.all_songs().into_iter().next().unwrap();
        assert_eq!(relinked.id(), gone.id(), "weak re-link keeps the UUID");
        assert_eq!(relinked.path().display(), "new/tone.flac");
    }

    #[test]
    fn progress_persisted_throttled_and_terminal_state_kept() {
        let fixture = ScanFixture::new();
        for index in 0..7 {
            let path = format!("file{index}.mp3");
            fixture.write_file(&path, format!("audio-{index}").as_bytes());
            fixture.set_audio(&path, &format!("T{index}"), 1_000);
        }
        // batch_size 2 → 4 parse batches; the clock steps 60 ms per read with
        // a 100 ms interval, so batch snapshots are throttled while phase
        // transitions persist immediately.
        let deps = ScanDeps {
            config: ScanConfig {
                batch_size: 2,
                worker_threads: 1,
                progress_interval: Duration::from_millis(100),
                lyrics_limit: 2 * 1024 * 1024,
            },
            clock: Arc::new(crate::application::testing::clock::SteppingClock::new(60)),
            ..ScanDeps::clone(&fixture.deps)
        };
        let summary = StartScan::new(&deps, &fixture.supervisor)
            .run(fixture.root)
            .expect("scan ok");
        assert_eq!(summary.progress.state, ScanState::Completed);
        let row = fixture.run_row(1).expect("run row");
        assert!(row.finished, "terminal state never lost");
        assert_eq!(row.state, ScanState::Completed);
        assert_eq!(row.progress.failed, 0);
        // Throttle: at most one snapshot per interval. Phase transitions (5)
        // always persist; the 4 batch snapshots are throttled by the clock.
        assert!(
            row.updates <= 5 + 2,
            "batch snapshots must be throttled: {}",
            row.updates
        );
    }

    #[test]
    fn manual_rescan_is_repeatable_and_converges() {
        let fixture = ScanFixture::new();
        fixture.write_file("a.mp3", b"audio");
        fixture.set_audio("a.mp3", "A", 1_000);
        let relink = RelinkLibrary::new(&fixture.deps, &fixture.supervisor);
        let first = relink.run(fixture.root).expect("first rescan");
        let second = relink.run(fixture.root).expect("second rescan");
        assert_eq!(first.generation, 1);
        assert_eq!(second.generation, 2);
        // Convergence: the second scan changes nothing (all fast-skipped).
        assert_eq!(second.progress.created, 0);
        assert_eq!(second.progress.updated, 0);
        assert_eq!(second.progress.skipped, 1);
        assert_eq!(fixture.all_songs().len(), 1);
    }

    #[test]
    fn bounded_workers_never_exceed_the_configured_bound() {
        // A parse pool of 2 must never run more than 2 files concurrently.
        let fixture = ScanFixture::new();
        for index in 0..8 {
            let path = format!("file{index}.mp3");
            fixture.write_file(&path, format!("audio-{index}").as_bytes());
            fixture.set_audio(&path, &format!("T{index}"), 1_000);
        }
        let current = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let max_seen = Arc::new(std::sync::atomic::AtomicUsize::new(0));

        let deps = ScanDeps {
            config: ScanConfig {
                batch_size: 8,
                worker_threads: 2,
                ..ScanConfig::default()
            },
            probe: Arc::new(CountingProbe {
                inner: Arc::clone(&fixture.deps.probe),
                current: Arc::clone(&current),
                max_seen: Arc::clone(&max_seen),
            }),
            ..ScanDeps::clone(&fixture.deps)
        };
        StartScan::new(&deps, &fixture.supervisor)
            .run(fixture.root)
            .expect("scan ok");
        assert_eq!(
            max_seen.load(std::sync::atomic::Ordering::SeqCst),
            2,
            "concurrency must be bounded by worker_threads"
        );
    }

    #[test]
    fn embedded_and_sidecar_lyrics_are_persisted_per_source() {
        let fixture = ScanFixture::new();
        fixture.write_file("song.mp3", b"audio");
        fixture.write_file("song.lrc", b"[00:01.00]sidecar line");
        fixture.set_audio("song.mp3", "Song", 1_000);
        // The reader reports embedded lyrics; the parser turns them into lines.
        fixture.metadata.set(
            "song.mp3",
            crate::domain::media::ParsedMetadata {
                title: Some("Song".to_owned()),
                artist: Some("歌手".to_owned()),
                album: Some("专辑".to_owned()),
                duration: Some(std::time::Duration::from_secs(1)),
                format: crate::domain::media::AudioFormat::Flac,
                embedded_lyrics: Some("[00:01.00]embedded line".to_owned()),
                ..crate::domain::media::ParsedMetadata::default()
            },
        );
        start_scan(&fixture).run(fixture.root).expect("scan");
        let song = fixture.all_songs().into_iter().next().expect("song");
        let candidates =
            crate::application::ports::LyricsRepository::candidates(&fixture.database, song.id())
                .expect("candidates");
        let sources: Vec<_> = candidates
            .iter()
            .map(crate::domain::entities::LyricsCandidate::source)
            .collect();
        assert!(sources.contains(&crate::domain::entities::LyricsSource::Embedded));
        assert!(sources.contains(&crate::domain::entities::LyricsSource::Sidecar));
        // No stale candidates survive a rescan: the audio changes (so the
        // file is re-parsed, not fast-skipped) and the sidecar is gone.
        fixture.write_file("song.mp3", b"audio-v2");
        fixture.remove_file("song.lrc");
        fixture.set_audio("song.mp3", "Song", 1_000);
        start_scan(&fixture).run(fixture.root).expect("rescan");
        let candidates =
            crate::application::ports::LyricsRepository::candidates(&fixture.database, song.id())
                .expect("candidates");
        assert!(
            !candidates
                .iter()
                .any(|c| c.source() == crate::domain::entities::LyricsSource::Sidecar),
            "cleared sidecar must not linger: {candidates:?}"
        );
    }

    #[test]
    fn cover_cache_key_and_db_reference_stay_consistent() {
        let fixture = ScanFixture::new();
        fixture.write_file("song.mp3", b"audio");
        fixture.set_audio("song.mp3", "Song", 1_000);
        fixture.metadata.set(
            "song.mp3",
            crate::domain::media::ParsedMetadata {
                title: Some("Song".to_owned()),
                cover: Some(crate::domain::media::EmbeddedCover {
                    bytes: b"cover-bytes".to_vec(),
                    mime: "image/png".to_owned(),
                }),
                ..crate::domain::media::ParsedMetadata::default()
            },
        );
        start_scan(&fixture).run(fixture.root).expect("scan");
        let song = fixture.all_songs().into_iter().next().expect("song");
        let cover =
            crate::application::ports::CoverRepository::cover_of(&fixture.database, song.id())
                .expect("cover ref")
                .expect("cover attached");
        // The asset key round-trips through the cache: opaque, content keyed.
        assert_eq!(
            fixture
                .deps
                .cover_cache
                .get(&cover.asset_key)
                .expect("get")
                .as_deref(),
            Some(b"cover-bytes".as_slice())
        );
        assert_eq!(cover.mime, "image/png");
        // And the referenced key is exactly the GC keep-set.
        let keys = crate::application::ports::CoverRepository::referenced_asset_keys(
            &fixture.database,
            fixture.root,
        )
        .expect("keys");
        assert_eq!(keys, vec![cover.asset_key]);
    }

    #[test]
    fn revision_and_added_at_survive_rescans() {
        let fixture = ScanFixture::new();
        fixture.write_file("a.mp3", b"audio");
        fixture.set_audio("a.mp3", "A", 1_000);
        start_scan(&fixture).run(fixture.root).expect("scan");
        let song = fixture.all_songs().into_iter().next().unwrap();
        let added_at = song.added_at();
        // Change the file content and its tags → the rescan re-parses and
        // updates, not re-creates (an unchanged file is a fast-skip by
        // design).
        fixture.write_file("a.mp3", b"audio-v2-longer");
        fixture.set_audio("a.mp3", "A2", 1_000);
        start_scan(&fixture).run(fixture.root).expect("rescan");
        let updated = fixture.all_songs().into_iter().next().unwrap();
        assert_eq!(updated.id(), song.id(), "identity stable across rescans");
        assert_eq!(updated.title(), Some("A2"));
        assert_eq!(updated.added_at(), added_at, "added_at never changes");
        assert!(updated.revision() >= Revision::INITIAL);
    }
}
