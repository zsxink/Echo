//! Per-file parsing/staging pipeline helpers: bounded worker pool, single
//! file parse with content-change checks, lyrics rewrap and progress
//! emission (tasks 4.4/4.8, design §6.4/§6.5).

#[allow(clippy::wildcard_imports)]
use super::*;

pub fn transition(progress: &mut ScanProgress, next: ScanState) -> Result<(), Error> {
    progress
        .transition(next)
        .map_err(|error| Error::InvariantViolation {
            why: error.to_string(),
        })
}

/// One parsed file plus the side assets the reconcile must persist.
#[derive(Debug)]
pub struct ParsedOutcome {
    pub file: ParsedFile,
    pub embedded_lyrics: Option<LyricsCandidate>,
    pub sidecar_lyrics: Option<LyricsCandidate>,
    pub cover: Option<CoverAssetRef>,
}

/// One worker-produced outcome for a candidate file.
#[derive(Debug)]
pub enum FileOutcome {
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
pub fn parse_bounded(
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

pub fn order_by_batch(outcomes: Vec<FileOutcome>, batch: &[RelativeMediaPath]) -> Vec<FileOutcome> {
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
pub fn parse_single_file(
    deps: &ScanDeps,
    root: LibraryRootId,
    path: &RelativeMediaPath,
) -> FileOutcome {
    parse_single_file_with_content_check(deps, root, path, false)
}

/// Parse a file event without trusting its size/mtime pair as a content
/// fingerprint. External tag editors can preserve both while changing the
/// embedded artwork or lyrics, so watcher events must still hash the bytes.
pub fn parse_file_event(
    deps: &ScanDeps,
    root: LibraryRootId,
    path: &RelativeMediaPath,
) -> FileOutcome {
    parse_single_file_with_content_check(deps, root, path, true)
}

pub fn parse_single_file_with_content_check(
    deps: &ScanDeps,
    root: LibraryRootId,
    path: &RelativeMediaPath,
    force_content_check: bool,
) -> FileOutcome {
    let file_meta: FileMeta = match deps.fs.file_meta(root, path) {
        Ok(meta) => meta,
        Err(error) => return FileOutcome::Diagnostic(diagnostic_of(path, &error)),
    };
    if !force_content_check {
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
pub fn rewrap(candidate: &LyricsCandidate, source: LyricsSource) -> LyricsCandidate {
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
pub struct ProgressEmitter<'a> {
    deps: &'a ScanDeps,
    root: LibraryRootId,
    generation: u64,
    last: Option<Duration>,
}

impl<'a> ProgressEmitter<'a> {
    pub const fn new(deps: &'a ScanDeps, root: LibraryRootId, generation: u64) -> Self {
        Self {
            deps,
            root,
            generation,
            last: None,
        }
    }

    pub fn emit(&mut self, progress: &ScanProgress, force: bool) -> Result<(), Error> {
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
