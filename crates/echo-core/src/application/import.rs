//! Per-input multi-select import planning (`PlanImport`, tasks 5.1–5.3,
//! design §8).
//!
//! A user-selected batch is processed **one input at a time**: each input is
//! classified (imported / duplicate content / unsupported / failed), and the
//! inputs that proceed are planned as their own operation — a fresh
//! `OperationId` and a **reserved** `SongId`, persisted in the operation
//! journal together with the target-path claim *before* the first external
//! side effect (design §8: 先持久化意图，再执行调用，再持久化结果).
//!
//! The per-resource pipeline (task 5.3) streams every source straight into
//! Echo's **exclusive, marker-verified staging directory**
//! (`<root>/.echo-staging-…/import/<operation-id>`): the copy runs in bounded
//! chunks with BLAKE3 accumulated during the copy, the staged file is fsynced
//! before it counts as staged, and the source file is read exactly once and
//! never modified. The target `歌手/歌手 - 歌曲名.原扩展名` (task 5.2) is
//! planned from the tags of the staged copy, then the conditional unique
//! target claim plus the reserved identity are persisted before the publish;
//! the publish itself reserves the target exclusively (create-new) and swaps
//! the verified staged copy in with one same-filesystem rename, so the song
//! record — committed only afterwards — never points at a half file.
//!
//! Isolation is the contract: every input commits (or fails) on its own, so a
//! single failed input never rolls back the inputs already imported, and the
//! caller receives exactly one result per input, index-aligned with the batch.
//! A root that cannot accept writes refuses the whole batch *before* any copy
//! — every input then reports `LibraryUnavailable` (spec: 导入时根目录断开).
//!
//! Scope note: the full per-step journal chain
//! (`CopyPending → … → PublishApplied`, fault injection and the crash
//! recovery matrix) is task 5.5; this module persists the two points it owns —
//! the plan reservation and the terminal result — and verifies size/hash at
//! the published location before the database commit.

use std::collections::{HashMap, HashSet};

use crate::application::ports::{
    ImportSource, ImportSourceInfo, ImportSourceReader, OperationItem, OperationResourceKind,
    StagedResource, TxAccess,
};
use crate::application::relink::song_from_parsed;
use crate::application::scan::{
    parse_single_file, rewrap, FileOutcome, ParsedOutcome, ScanDeps, SUPPORTED_EXTENSIONS,
};
use crate::domain::entities::LyricsSource;
use crate::domain::ids::{LibraryRootId, OperationId, RelativeMediaPath, SongId};
use crate::domain::state::OperationState;
use crate::domain::text::{
    target_artist_component, target_file_stem, truncate_component_with_extension,
};
use crate::error::{Error, Subject};

/// The one staging resource every import audio operation owns.
const IMPORT_AUDIO_RESOURCE: &str = "audio";
/// The `operation` classifier used by import verification errors.
const IMPORT_OPERATION: &str = "import";
/// Conservative single-component byte cap for planned targets (the OS allows
/// 255 on the major desktop filesystems). Both the artist directory and the
/// `artist - title` file component are bounded by it; over-long components
/// keep the extension plus the short-hash suffix (domain truncation rule).
const TARGET_COMPONENT_BYTES: usize = 200;
/// Upper bound for the minimal ` (n)` conflict numbering search.
const MAX_NUMBERING: u64 = 4_096;

/// The result of one batch input, index-aligned with the batch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ImportOutcome {
    /// Copied, verified, published and committed under the reserved identity.
    Imported {
        /// The journal operation that performed this import.
        operation: OperationId,
        /// The reserved `SongId` the record was committed with.
        song: SongId,
        /// The final root-relative path of the published audio.
        target: RelativeMediaPath,
    },
    /// The content hash already belongs to a library record — no copy, no
    /// second UUID; the caller points the user at the existing song.
    Duplicate { existing: SongId },
    /// The input is not an importable audio type (extension matrix).
    Unsupported,
    /// The library root could not accept writes; the whole batch was refused
    /// before any copy (spec: 资料库不可用 → 开始复制前拒绝整批).
    LibraryUnavailable,
    /// This input failed. The message is user-safe: Core errors redact
    /// absolute paths before display.
    Failed {
        /// The stable machine code of the underlying error.
        code: &'static str,
        /// The redacted, user-presentable explanation.
        message: String,
    },
}

/// One batch's per-input results, index-aligned with the inputs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportBatchReport {
    pub results: Vec<ImportOutcome>,
}

/// Plans and executes one multi-select import batch, one input at a time.
///
/// Blocking (file copies, hashing, parse): the runtime calls it on a worker
/// thread, exactly like [`crate::application::scan::StartScan`].
pub struct PlanImport<'a> {
    deps: &'a ScanDeps,
    sources: &'a dyn ImportSourceReader,
}

/// Mutable per-batch identity state: hashes and target keys claimed by the
/// library snapshot plus the inputs committed so far in this batch.
struct BatchState {
    holders: HashMap<String, SongId>,
    taken_targets: HashSet<String>,
}

/// One input's reserved plan: the identity, staged resource and target every
/// later step (and any recovery, in task 5.5) must agree on.
struct PlannedInput {
    root: LibraryRootId,
    target: RelativeMediaPath,
    operation: OperationId,
    reserved: SongId,
    /// The logical source locator recorded in the journal item (never a path).
    source: Option<String>,
    /// The staged resource handle driving the controlled staging directory.
    staged: StagedResource,
    /// The staged file's root-relative location (journal bookkeeping).
    staged_path: RelativeMediaPath,
    hash: String,
    size: u64,
}

impl<'a> PlanImport<'a> {
    #[must_use]
    pub const fn new(deps: &'a ScanDeps, sources: &'a dyn ImportSourceReader) -> Self {
        Self { deps, sources }
    }

    /// Run one batch against `root`.
    ///
    /// # Errors
    ///
    /// Only batch-level infrastructure failures (root-capability or library
    /// snapshot reads) propagate; per-input problems become `Failed` results
    /// so one bad input can never abort the batch.
    pub fn run(
        &self,
        root: LibraryRootId,
        sources: &[ImportSource],
    ) -> Result<ImportBatchReport, Error> {
        // Refuse the whole batch before any copy when the root cannot take
        // writes (spec: 资料库根目录不可用时在开始复制前拒绝整批导入).
        if !self.deps.fs.write_capable(root)? {
            return Ok(ImportBatchReport {
                results: vec![ImportOutcome::LibraryUnavailable; sources.len()],
            });
        }
        let mut state = self.batch_state(root)?;
        let mut results = Vec::with_capacity(sources.len());
        for source in sources {
            results.push(self.import_one(root, source, &mut state));
        }
        Ok(ImportBatchReport { results })
    }

    /// The batch-start identity snapshot: every hash holder, plus every
    /// occupied target path — library records *and* files that exist only on
    /// disk (external placement, dropped database), so numbering continues
    /// until the target truly does not exist (绝不覆盖既有文件).
    fn batch_state(&self, root: LibraryRootId) -> Result<BatchState, Error> {
        let mut holders = HashMap::new();
        let mut taken_targets = HashSet::new();
        for song in self.deps.songs.all_in_root(root)? {
            if let Some(hash) = song.blake3_hash() {
                holders.entry(hash.to_owned()).or_insert_with(|| song.id());
            }
            taken_targets.insert(song.path().identity_key().to_owned());
        }
        for path in self.deps.fs.enumerate(root)? {
            taken_targets.insert(path.identity_key().to_owned());
        }
        Ok(BatchState {
            holders,
            taken_targets,
        })
    }

    /// Classify, plan and import one input. Never fails the batch: every
    /// failure mode becomes this input's `Failed` result (and rolls back its
    /// own journal claim and staged copy) while the other inputs continue.
    fn import_one(
        &self,
        root: LibraryRootId,
        source: &ImportSource,
        state: &mut BatchState,
    ) -> ImportOutcome {
        let info: ImportSourceInfo = match self.sources.describe(source) {
            Ok(info) => info,
            Err(error) => return failed_of(&error),
        };
        // The extension is both the type filter and the target's 原扩展名
        // (normalized to lowercase for a deterministic target name).
        let Some(ext) = supported_extension_of(&info.display_name) else {
            return ImportOutcome::Unsupported;
        };
        let planned = match self.stage_and_plan(root, source, &info, &ext, state) {
            Ok(planned) => planned,
            Err(outcome) => return outcome,
        };
        // Persist the intent (reserved SongId + per-resource source/staging/
        // target/hash + the conditional unique target claim) before the first
        // external side effect; the claim keeps the target path reserved until
        // the operation reaches a terminal state.
        if let Err(error) = self.deps.journal.ensure_operation(
            planned.operation,
            root,
            IMPORT_OPERATION,
            Some(planned.reserved),
        ) {
            let _ = self.deps.fs.discard_staged(root, &planned.staged);
            return failed_of(&error);
        }
        if let Err(error) = self.deps.journal.upsert_item(
            planned.operation,
            journal_item(OperationState::Planned, &planned),
        ) {
            let _ = self.deps.fs.discard_staged(root, &planned.staged);
            return failed_of(&error);
        }
        state
            .taken_targets
            .insert(planned.target.identity_key().to_owned());

        match self.execute(&planned) {
            Ok(()) => {
                state.holders.insert(planned.hash.clone(), planned.reserved);
                ImportOutcome::Imported {
                    operation: planned.operation,
                    song: planned.reserved,
                    target: planned.target.clone(),
                }
            }
            Err(error) => {
                // Per-input rollback: the terminal journal state releases this
                // input's claim; committed inputs and the remaining batch are
                // untouched. The staged copy is cleaned up best-effort.
                let _ = self.deps.fs.discard_staged(root, &planned.staged);
                let rolled_back = journal_item(OperationState::RolledBack, &planned);
                let _ = self
                    .deps
                    .journal
                    .upsert_item(planned.operation, rolled_back);
                let _ = self.deps.journal.release_claims(planned.operation);
                failed_of(&error)
            }
        }
    }

    /// The per-resource pipeline up to the target claim (task 5.3): open the
    /// source once, stream it into the operation's controlled staging slot
    /// (BLAKE3 accumulated during the copy, staged file fsynced), verify the
    /// size against the description, dedup on the copied hash, parse the
    /// staged copy's tags and plan the target name. `Err` carries this
    /// input's terminal outcome (the staged copy is already discarded).
    fn stage_and_plan(
        &self,
        root: LibraryRootId,
        source: &ImportSource,
        info: &ImportSourceInfo,
        ext: &str,
        state: &BatchState,
    ) -> Result<PlannedInput, ImportOutcome> {
        let discard = |staged: &StagedResource| {
            let _ = self.deps.fs.discard_staged(root, staged);
        };
        let operation = self.deps.ids.new_operation_id();
        let reserved = self.deps.ids.new_song_id();
        let staged = match StagedResource::new(operation, IMPORT_AUDIO_RESOURCE) {
            Ok(staged) => staged,
            Err(error) => return Err(failed_of(&error)),
        };
        // 流式复制+BLAKE3 (task 5.3): the source is opened once and streamed
        // straight into the operation's controlled staging slot; the hash is
        // computed while the copy runs and the staged file is fsynced.
        let copy = self
            .sources
            .open(source)
            .and_then(|mut reader| self.deps.fs.stage_stream(root, &staged, reader.as_mut()))
            .map_err(|error| {
                discard(&staged);
                failed_of(&error)
            })?;
        // A source that does not match its description (vanished or truncated
        // mid-selection) never enters the library.
        if copy.size != info.size {
            discard(&staged);
            return Err(failed_of(&Error::CorruptMedia {
                operation: IMPORT_OPERATION.to_owned(),
                reason: "source content does not match the described size".to_owned(),
            }));
        }
        // Plan-time BLAKE3 dedup (task 5.6 adds the pre-commit re-check):
        // identical content returns the existing record, never a second UUID.
        if let Some(existing) = state.holders.get(&copy.blake3) {
            discard(&staged);
            return Err(ImportOutcome::Duplicate {
                existing: *existing,
            });
        }
        // The target is named from the tags of the staged copy (task 5.2);
        // unparseable content fails this input instead of guessing a name.
        let outcome_on_error = |error: &Error| {
            discard(&staged);
            failed_of(error)
        };
        let bytes = self
            .deps
            .fs
            .read_staged(root, &staged)
            .map_err(|error| outcome_on_error(&error))?;
        let meta = self
            .deps
            .metadata
            .read_bytes(&bytes)
            .map_err(|error| outcome_on_error(&error))?;
        let target = plan_named_target(
            meta.artist.as_deref(),
            meta.title.as_deref(),
            ext,
            &mut |key| state.taken_targets.contains(key),
        )
        .ok_or_else(|| {
            outcome_on_error(&Error::validation(
                Subject::Path,
                "import target",
                "no safe unique target name for the parsed tags",
            ))
        })?;
        Ok(PlannedInput {
            root,
            target,
            operation,
            reserved,
            source: Some(source.key().to_owned()),
            staged,
            staged_path: copy.staged_path,
            hash: copy.blake3.clone(),
            size: copy.size,
        })
    }

    /// Publish → verify → parse → commit one planned input.
    fn execute(&self, planned: &PlannedInput) -> Result<(), Error> {
        // Publish reserves the target exclusively (create-new) and swaps the
        // verified staged copy in with one same-filesystem rename: an occupied
        // target is a conflict, never a replacement (绝不覆盖既有文件).
        self.deps
            .fs
            .publish(planned.root, &planned.staged, &planned.target)?;
        // `*Applied` only counts once the final location is verified: size and
        // hash must match the staged copy (design §8).
        let meta = self.deps.fs.file_meta(planned.root, &planned.target)?;
        if meta.size != planned.size {
            return Err(Error::CorruptMedia {
                operation: IMPORT_OPERATION.to_owned(),
                reason: "published size differs from the staged content".to_owned(),
            });
        }
        let published_hash = self.deps.hasher.hash(planned.root, &planned.target)?;
        if published_hash != planned.hash {
            return Err(Error::CorruptMedia {
                operation: IMPORT_OPERATION.to_owned(),
                reason: "published hash differs from the staged content".to_owned(),
            });
        }
        // Parse the published file with the shared scan pipeline (probe, tags,
        // sidecar, cover) and commit the record under the RESERVED identity.
        match parse_single_file(self.deps, planned.root, &planned.target) {
            FileOutcome::Parsed(parsed) => self.commit(planned, &parsed),
            FileOutcome::Diagnostic(diagnostic) => Err(Error::CorruptMedia {
                operation: IMPORT_OPERATION.to_owned(),
                reason: diagnostic.reason().to_owned(),
            }),
            FileOutcome::FastSkip { .. } => Err(Error::InvariantViolation {
                why: "import target is already owned by another record".to_owned(),
            }),
        }
    }

    /// Commit the song record and the journal's `DatabaseCommitted` state in
    /// one transaction, then finalize the operation and release the claim.
    fn commit(&self, planned: &PlannedInput, parsed: &ParsedOutcome) -> Result<(), Error> {
        let entity = song_from_parsed(planned.reserved, planned.root, &parsed.file);
        let embedded = parsed
            .embedded_lyrics
            .clone()
            .map(|candidate| rewrap(&candidate, LyricsSource::Embedded));
        let sidecar = parsed
            .sidecar_lyrics
            .clone()
            .map(|candidate| rewrap(&candidate, LyricsSource::Sidecar));
        let cover = parsed.cover.clone();
        // Built before the transaction body: the closure owns the item.
        let committed = journal_item(OperationState::DatabaseCommitted, planned);
        let song_id = planned.reserved;
        let operation = planned.operation;
        self.deps
            .uow
            .with_tx(Box::new(move |tx: &mut dyn TxAccess| {
                tx.upsert_song(&entity)?;
                match embedded {
                    Some(candidate) => tx.set_lyrics_candidate(song_id, &candidate)?,
                    None => tx.clear_lyrics_candidate(song_id, LyricsSource::Embedded)?,
                }
                match sidecar {
                    Some(candidate) => tx.set_lyrics_candidate(song_id, &candidate)?,
                    None => tx.clear_lyrics_candidate(song_id, LyricsSource::Sidecar)?,
                }
                if let Some(cover) = cover {
                    tx.attach_cover(song_id, &cover)?;
                }
                // The record and the journal's DatabaseCommitted state land in
                // one transaction: a song is never visible without its journal.
                tx.upsert_operation_item(operation, committed)?;
                Ok(())
            }))?;
        // Terminal state releases the target claim (port contract) so retries
        // and other operations can claim the same path again.
        self.deps.journal.upsert_item(
            planned.operation,
            journal_item(OperationState::Completed, planned),
        )?;
        self.deps.journal.release_claims(planned.operation)?;
        Ok(())
    }
}

/// The supported-audio extension of a source display name, or `None` when the
/// name carries no usable `stem.extension` tail or the extension is outside
/// the scan's supported matrix.
fn supported_extension_of(display_name: &str) -> Option<String> {
    let file_name = display_name.rsplit(['/', '\\']).next()?;
    let (stem, ext) = file_name.rsplit_once('.')?;
    if stem.is_empty() || ext.is_empty() {
        return None;
    }
    let ext = ext.to_ascii_lowercase();
    SUPPORTED_EXTENSIONS.contains(&ext.as_str()).then_some(ext)
}

/// Plan the tag-driven import target `歌手/歌手 - 歌曲名.扩展名` (task 5.2).
///
/// - Missing/blank tags take the visible fallbacks 未知艺人 / 未命名歌曲
///   (domain rule [`target_file_stem`]);
/// - components are platform-safe (NFKC, forbidden/control chars, Windows
///   reserved names) and bounded by [`TARGET_COMPONENT_BYTES`], over-long
///   names keep the extension plus a stable short hash
///   ([`truncate_component_with_extension`]);
/// - on a conflict — a library record, an earlier batch input or an existing
///   on-disk file owning the identity — the minimal ` (n)` is appended to the
///   stem, so an occupied name is never claimed and an existing file is never
///   replaced.
///
/// `None` = no safe unique name within the numbering bound.
fn plan_named_target(
    artist: Option<&str>,
    title: Option<&str>,
    extension: &str,
    is_taken: &mut dyn FnMut(&str) -> bool,
) -> Option<RelativeMediaPath> {
    let dir = target_artist_component(artist, TARGET_COMPONENT_BYTES);
    let base_stem = target_file_stem(artist, title);
    for suffix in 1..=MAX_NUMBERING {
        let stem = if suffix == 1 {
            base_stem.clone()
        } else {
            format!("{base_stem} ({suffix})")
        };
        let file = truncate_component_with_extension(&stem, extension, TARGET_COMPONENT_BYTES);
        if let Ok(path) = RelativeMediaPath::new(&format!("{dir}/{file}")) {
            if !is_taken(path.identity_key()) {
                return Some(path);
            }
        }
    }
    None
}

/// One journal item row for the import audio resource: per-resource source
/// locator, staged location, target and expected hash (design §8: item 固定
/// 保存 kind、外部源定位、暂存/目标相对路径、预期 BLAKE3 和 target claim).
fn journal_item(state: OperationState, planned: &PlannedInput) -> OperationItem {
    OperationItem {
        kind: OperationResourceKind::Audio,
        state,
        song: Some(planned.reserved),
        source: planned.source.clone(),
        staging_path: Some(planned.staged_path.clone()),
        target_path: planned.target.clone(),
        expected_hash: planned.hash.clone(),
        claim_key: planned.target.identity_key().to_owned(),
    }
}

/// A per-input failure result from any Core error (already path-redacted by
/// the error's own `Display`).
fn failed_of(error: &Error) -> ImportOutcome {
    ImportOutcome::Failed {
        code: error.code(),
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::io::Read;
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::application::ports::{
        FileMeta, LibraryFileSystem, OperationJournalRepository, SongRepository as _, StagedCopy,
    };
    use crate::application::scan::ScanConfig;
    use crate::application::testing::clock::{FakeIdGenerator, ManualClock};
    use crate::application::testing::small_fakes::{
        FakeFileHasher, FakeLyricsParser, FakeMediaProbe, FakeMetadataReader, MemoryCoverCache,
    };
    use crate::application::testing::ScanFixture;
    use crate::application::testing::{FakeImportSources, FakeLibraryFileSystem, MemoryDatabase};
    use crate::domain::entities::Song;
    use crate::domain::ids::Revision;
    use crate::domain::media::{AudioFormat, ParsedMetadata};

    /// Register the tags an import source's *content* carries — the input the
    /// task-5.2 naming step parses before planning the target.
    fn tagged(g: &Gated, bytes: &[u8], artist: Option<&str>, title: Option<&str>) {
        g.fixture.metadata.set_bytes(
            bytes,
            ParsedMetadata {
                artist: artist.map(ToOwned::to_owned),
                title: title.map(ToOwned::to_owned),
                ..ParsedMetadata::default()
            },
        );
    }

    /// A fs wrapper that refuses every *external* side effect unless the
    /// operation's journal reservation (`Planned` item with a reserved
    /// `SongId`) is already persisted — the test double that proves
    /// 先持久化意图，再执行调用 for the publish. Staging itself is internal
    /// scratch inside Echo's marker-verified directory (invisible to the
    /// library), so the gate guards the publication boundary.
    struct ReservationGate {
        inner: FakeLibraryFileSystem,
        journal: MemoryDatabase,
        violations: Arc<Mutex<Vec<String>>>,
    }

    impl ReservationGate {
        fn verify_reserved(&self, operation: OperationId) {
            let reserved = OperationJournalRepository::items(&self.journal, operation)
                .unwrap_or_default()
                .into_iter()
                .any(|item| item.state == OperationState::Planned && item.song.is_some());
            if !reserved {
                self.violations
                    .lock()
                    .unwrap()
                    .push(format!("side effect before reservation: {operation}"));
            }
        }
    }

    impl LibraryFileSystem for ReservationGate {
        fn enumerate(&self, root: LibraryRootId) -> Result<Vec<RelativeMediaPath>, Error> {
            self.inner.enumerate(root)
        }
        fn file_meta(
            &self,
            root: LibraryRootId,
            path: &RelativeMediaPath,
        ) -> Result<FileMeta, Error> {
            self.inner.file_meta(root, path)
        }
        fn read_head(
            &self,
            root: LibraryRootId,
            path: &RelativeMediaPath,
            limit: u64,
        ) -> Result<Vec<u8>, Error> {
            self.inner.read_head(root, path, limit)
        }
        fn publish(
            &self,
            root: LibraryRootId,
            staged: &StagedResource,
            target: &RelativeMediaPath,
        ) -> Result<(), Error> {
            self.verify_reserved(staged.operation());
            self.inner.publish(root, staged, target)
        }
        fn stage(
            &self,
            root: LibraryRootId,
            staged: &StagedResource,
            content: &[u8],
        ) -> Result<(), Error> {
            self.inner.stage(root, staged, content)
        }
        fn stage_stream(
            &self,
            root: LibraryRootId,
            staged: &StagedResource,
            content: &mut dyn Read,
        ) -> Result<StagedCopy, Error> {
            self.inner.stage_stream(root, staged, content)
        }
        fn read_staged(
            &self,
            root: LibraryRootId,
            staged: &StagedResource,
        ) -> Result<Vec<u8>, Error> {
            self.inner.read_staged(root, staged)
        }
        fn discard_staged(
            &self,
            root: LibraryRootId,
            staged: &StagedResource,
        ) -> Result<(), Error> {
            self.inner.discard_staged(root, staged)
        }
        fn write_capable(&self, root: LibraryRootId) -> Result<bool, Error> {
            self.inner.write_capable(root)
        }
    }

    /// The gated test composition: the use case sees the gate as its file
    /// system, so every stage/publish proves the reservation ordering.
    struct Gated {
        fixture: ScanFixture,
        gate: Arc<ReservationGate>,
        deps: ScanDeps,
        sources: FakeImportSources,
    }

    fn gated() -> Gated {
        let fixture = ScanFixture::new();
        let gate = Arc::new(ReservationGate {
            inner: fixture.fs.clone(),
            journal: fixture.database.clone(),
            violations: Arc::new(Mutex::new(Vec::new())),
        });
        let deps = {
            let fs: Arc<dyn LibraryFileSystem> = gate.clone();
            ScanDeps {
                fs,
                ..ScanDeps::clone(&fixture.deps)
            }
        };
        Gated {
            fixture,
            gate,
            deps,
            sources: FakeImportSources::new(),
        }
    }

    fn source(key: &str) -> ImportSource {
        ImportSource::new(key).expect("valid source handle")
    }

    fn seed_song(gated: &Gated, path: &str, bytes: &[u8]) -> Song {
        let mut song = Song::new(
            crate::domain::ids::SongId::new(),
            gated.fixture.root,
            RelativeMediaPath::new(path).expect("valid path"),
            Revision::INITIAL,
        );
        song.apply_scan_facts(
            gated.deps.hasher.hash_of_bytes(bytes),
            u64::try_from(bytes.len()).unwrap_or(1),
            1,
            AudioFormat::Flac,
        );
        gated.fixture.database.upsert(&song).expect("seed song");
        song
    }

    fn violations(gated: &Gated) -> Vec<String> {
        gated.gate.violations.lock().unwrap().clone()
    }

    /// Register the tags an import source's *content* carries on a bare
    /// fixture (the content-keyed metadata lookup the planning step uses).
    fn tag_content(fixture: &ScanFixture, bytes: &[u8], artist: Option<&str>, title: Option<&str>) {
        fixture.metadata.set_bytes(
            bytes,
            ParsedMetadata {
                artist: artist.map(ToOwned::to_owned),
                title: title.map(ToOwned::to_owned),
                ..ParsedMetadata::default()
            },
        );
    }

    /// The operation id of the first imported input (assertion helper).
    fn operation_of(report: &ImportBatchReport) -> Option<OperationId> {
        report.results.iter().find_map(|outcome| match outcome {
            ImportOutcome::Imported { operation, .. } => Some(*operation),
            _ => None,
        })
    }

    /// An [`ImportSourceReader`] over REAL files inside injected temp
    /// directories — lets the import prove source invariance end-to-end
    /// without ever touching the user's home.
    struct TempFileSources {
        files: Mutex<BTreeMap<String, (String, std::path::PathBuf)>>,
        opened: Mutex<Vec<String>>,
    }

    impl TempFileSources {
        fn new() -> Self {
            Self {
                files: Mutex::new(BTreeMap::new()),
                opened: Mutex::new(Vec::new()),
            }
        }
        fn add(&self, key: &str, display_name: &str, path: &std::path::Path) {
            self.files.lock().unwrap().insert(
                key.to_owned(),
                (display_name.to_owned(), path.to_path_buf()),
            );
        }
        fn opened(&self) -> Vec<String> {
            self.opened.lock().unwrap().clone()
        }
    }

    impl ImportSourceReader for TempFileSources {
        fn describe(&self, source: &ImportSource) -> Result<ImportSourceInfo, Error> {
            let (display_name, path) = {
                let map = self.files.lock().unwrap();
                match map.get(source.key()) {
                    Some(entry) => entry.clone(),
                    None => return Err(Error::unavailable("import source", "unknown handle")),
                }
            };
            let size = std::fs::metadata(&path)
                .map_err(|e| Error::io("stat import source", e, path.clone()))?
                .len();
            Ok(ImportSourceInfo { display_name, size })
        }

        fn open<'a>(&'a self, source: &ImportSource) -> Result<Box<dyn Read + 'a>, Error> {
            self.opened.lock().unwrap().push(source.key().to_owned());
            let path = {
                let map = self.files.lock().unwrap();
                match map.get(source.key()) {
                    Some((_, path)) => path.clone(),
                    None => return Err(Error::unavailable("import source", "unknown handle")),
                }
            };
            let file = std::fs::File::open(&path)
                .map_err(|e| Error::io("open import source", e, path.clone()))?;
            Ok(Box::new(file))
        }
    }

    /// A fs wrapper that snapshots the song table at every ingestion step —
    /// the test double proving the record exists only after the full audio
    /// publish (`DatabaseCommitted` comes after `publish`).
    struct VisibilityProbe {
        inner: FakeLibraryFileSystem,
        songs: MemoryDatabase,
        root: LibraryRootId,
        events: Mutex<Vec<(&'static str, usize)>>,
    }

    impl VisibilityProbe {
        fn record(&self, event: &'static str) {
            let count =
                crate::application::ports::SongRepository::all_in_root(&self.songs, self.root)
                    .map_or(usize::MAX, |songs| songs.len());
            self.events.lock().unwrap().push((event, count));
        }
        fn events(&self) -> Vec<(&'static str, usize)> {
            self.events.lock().unwrap().clone()
        }
    }

    impl LibraryFileSystem for VisibilityProbe {
        fn enumerate(&self, root: LibraryRootId) -> Result<Vec<RelativeMediaPath>, Error> {
            self.inner.enumerate(root)
        }
        fn file_meta(
            &self,
            root: LibraryRootId,
            path: &RelativeMediaPath,
        ) -> Result<FileMeta, Error> {
            self.inner.file_meta(root, path)
        }
        fn read_head(
            &self,
            root: LibraryRootId,
            path: &RelativeMediaPath,
            limit: u64,
        ) -> Result<Vec<u8>, Error> {
            self.inner.read_head(root, path, limit)
        }
        fn publish(
            &self,
            root: LibraryRootId,
            staged: &StagedResource,
            target: &RelativeMediaPath,
        ) -> Result<(), Error> {
            let result = self.inner.publish(root, staged, target);
            self.record("publish");
            result
        }
        fn stage(
            &self,
            root: LibraryRootId,
            staged: &StagedResource,
            content: &[u8],
        ) -> Result<(), Error> {
            self.inner.stage(root, staged, content)
        }
        fn stage_stream(
            &self,
            root: LibraryRootId,
            staged: &StagedResource,
            content: &mut dyn Read,
        ) -> Result<StagedCopy, Error> {
            let result = self.inner.stage_stream(root, staged, content);
            self.record("stage_stream");
            result
        }
        fn read_staged(
            &self,
            root: LibraryRootId,
            staged: &StagedResource,
        ) -> Result<Vec<u8>, Error> {
            self.inner.read_staged(root, staged)
        }
        fn discard_staged(
            &self,
            root: LibraryRootId,
            staged: &StagedResource,
        ) -> Result<(), Error> {
            self.inner.discard_staged(root, staged)
        }
        fn write_capable(&self, root: LibraryRootId) -> Result<bool, Error> {
            self.inner.write_capable(root)
        }
    }

    #[test]
    fn mixed_batch_reports_each_input_independently() {
        let g = gated();
        // An existing library record owning some content, for the duplicate.
        let existing = seed_song(&g, "已有/other.flac", b"library-original");
        // The successful input: a supported audio file whose tags name the
        // target 歌手/歌手 - 晴天.flac and whose published file parses cleanly.
        g.sources.add("good", "晴天.flac", b"sunny-bytes");
        tagged(&g, b"sunny-bytes", Some("歌手"), Some("晴天"));
        g.fixture
            .set_audio("歌手/歌手 - 晴天.flac", "晴天", 269_000);
        g.sources.add("dup", "重复.flac", b"library-original");
        g.sources.add("text", "notes.txt", b"not audio");
        g.sources.add("locked", "locked.flac", b"unreadable");
        g.sources.fail("locked", "选择的外部文件不可读");

        let batch = [
            source("good"),
            source("dup"),
            source("text"),
            source("locked"),
        ];
        let report = PlanImport::new(&g.deps, &g.sources)
            .run(g.fixture.root, &batch)
            .expect("batch-level success");

        assert_eq!(report.results.len(), batch.len(), "one result per input");
        let ImportOutcome::Imported {
            operation: _,
            song: imported,
            target,
        } = &report.results[0]
        else {
            panic!(
                "the supported audio input must import: {:?}",
                report.results[0]
            );
        };
        assert_eq!(target.display(), "歌手/歌手 - 晴天.flac");
        assert_eq!(
            report.results[1],
            ImportOutcome::Duplicate {
                existing: existing.id()
            },
            "library content duplicate returns the existing UUID"
        );
        assert_eq!(report.results[2], ImportOutcome::Unsupported);
        let ImportOutcome::Failed { code, .. } = &report.results[3] else {
            panic!(
                "the unreadable source must fail, not abort: {:?}",
                report.results[3]
            );
        };
        assert_eq!(*code, "permission");

        // The imported input is fully usable: the file is published with the
        // exact source bytes and the record exists under the reserved identity.
        let published = g
            .fixture
            .fs
            .root_path(g.fixture.root)
            .expect("root")
            .join("歌手/歌手 - 晴天.flac");
        assert_eq!(
            std::fs::read(&published).expect("published bytes"),
            b"sunny-bytes"
        );
        let songs = g.fixture.all_songs();
        assert_eq!(songs.len(), 2, "seed + import, nothing else");
        let record = songs
            .iter()
            .find(|song| song.id() == *imported)
            .expect("record");
        assert_eq!(record.title(), Some("晴天"));
        assert_eq!(
            record.path(),
            &RelativeMediaPath::new("歌手/歌手 - 晴天.flac").unwrap()
        );
        assert!(violations(&g).is_empty());
    }

    #[test]
    fn import_reserves_operation_and_song_ids_before_side_effects() {
        let g = gated();
        g.sources.add("good", "晴天.flac", b"reserved-bytes");
        tagged(&g, b"reserved-bytes", Some("歌手"), Some("晴天"));
        g.fixture
            .set_audio("歌手/歌手 - 晴天.flac", "晴天", 269_000);

        let report = PlanImport::new(&g.deps, &g.sources)
            .run(g.fixture.root, &[source("good")])
            .expect("batch-level success");
        let ImportOutcome::Imported {
            operation,
            song,
            target,
        } = report.results.into_iter().next().expect("one result")
        else {
            panic!("the supported input must import");
        };

        assert!(
            violations(&g).is_empty(),
            "every stage/publish ran after the journal reservation: {:?}",
            violations(&g)
        );
        // The journal carries the operation with the RESERVED identity, the
        // target claim and the expected BLAKE3.
        let items = g.deps.journal.items(operation).expect("journal items");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].state, OperationState::Completed);
        assert_eq!(items[0].kind, OperationResourceKind::Audio);
        assert_eq!(items[0].song, Some(song), "UUID equals the reservation");
        assert_eq!(items[0].target_path, target);
        assert_eq!(items[0].claim_key, target.identity_key());
        assert_eq!(
            items[0].expected_hash,
            g.deps.hasher.hash_of_bytes(b"reserved-bytes"),
            "the journal records the expected content hash"
        );
        // Terminal state releases the claim (port contract).
        assert_eq!(g.fixture.database.released_claims(), vec![operation]);
        // The committed record carries the reserved UUID.
        let record = g.deps.songs.by_id(song).expect("query").expect("committed");
        assert_eq!(record.id(), song);
        assert_eq!(record.path(), &target);
    }

    #[test]
    fn single_input_failure_does_not_rollback_committed_inputs() {
        let g = gated();
        g.sources.add("a", "a.flac", b"content-a");
        tagged(&g, b"content-a", Some("歌手"), Some("A"));
        g.fixture.set_audio("歌手/歌手 - A.flac", "A", 1_000);
        g.sources.fail("b", "外部文件不可读");
        g.sources.add("c", "c.flac", b"content-c");
        tagged(&g, b"content-c", Some("歌手"), Some("C"));
        g.fixture.set_audio("歌手/歌手 - C.flac", "C", 3_000);

        let report = PlanImport::new(&g.deps, &g.sources)
            .run(g.fixture.root, &[source("a"), source("b"), source("c")])
            .expect("batch-level success");

        let ImportOutcome::Imported { song: a, .. } = report.results[0] else {
            panic!("the first input must succeed");
        };
        assert!(matches!(report.results[1], ImportOutcome::Failed { .. }));
        let ImportOutcome::Imported { song: c, .. } = report.results[2] else {
            panic!("the input AFTER the failure must still succeed");
        };
        assert_ne!(a, c, "each input reserves its own UUID");
        assert_eq!(
            g.fixture.all_songs().len(),
            2,
            "both successful inputs committed"
        );
        let root_dir = g.fixture.fs.root_path(g.fixture.root).expect("root");
        assert!(root_dir.join("歌手/歌手 - A.flac").exists());
        assert!(root_dir.join("歌手/歌手 - C.flac").exists());
        // The failed input left no claim behind: retries start clean.
        assert_eq!(g.fixture.database.released_claims().len(), 2);
        assert!(violations(&g).is_empty());
    }

    #[test]
    fn duplicate_content_within_a_batch_creates_one_record() {
        let g = gated();
        g.sources.add("x", "tune.flac", b"same-content");
        tagged(&g, b"same-content", Some("歌手"), Some("Tune"));
        g.fixture.set_audio("歌手/歌手 - Tune.flac", "Tune", 1_000);
        g.sources.add("y", "copy.flac", b"same-content");

        let report = PlanImport::new(&g.deps, &g.sources)
            .run(g.fixture.root, &[source("x"), source("y")])
            .expect("batch-level success");
        let ImportOutcome::Imported { song: first, .. } = report.results[0] else {
            panic!("the first input must import");
        };
        assert_eq!(
            report.results[1],
            ImportOutcome::Duplicate { existing: first },
            "the second copy of the same content reports the first import"
        );
        assert_eq!(g.fixture.all_songs().len(), 1, "one record per content");
    }

    #[test]
    fn unavailable_root_rejects_the_whole_batch_before_copying() {
        let g = gated();
        g.sources.add("good", "晴天.flac", b"some-bytes");
        g.sources.add("other", "other.flac", b"other-bytes");
        g.fixture.fs.set_write_capable(false);

        let report = PlanImport::new(&g.deps, &g.sources)
            .run(g.fixture.root, &[source("good"), source("other")])
            .expect("batch-level success");

        assert_eq!(
            report.results,
            vec![
                ImportOutcome::LibraryUnavailable,
                ImportOutcome::LibraryUnavailable
            ],
            "every input reports the library as unavailable"
        );
        assert!(
            g.sources.read_keys().is_empty(),
            "no source is read before the batch is refused"
        );
        assert!(
            g.fixture.all_songs().is_empty(),
            "nothing entered the library"
        );
    }

    #[test]
    fn same_name_different_content_gets_minimal_conflict_number() {
        let g = gated();
        g.fixture
            .write_file("歌手/歌手 - 晴天.flac", b"original-content");
        seed_song(&g, "歌手/歌手 - 晴天.flac", b"original-content");
        g.sources.add("new", "晴天.flac", b"different-content");
        tagged(&g, b"different-content", Some("歌手"), Some("晴天"));
        g.fixture
            .set_audio("歌手/歌手 - 晴天 (2).flac", "晴天", 1_000);

        let report = PlanImport::new(&g.deps, &g.sources)
            .run(g.fixture.root, &[source("new")])
            .expect("batch-level success");
        let ImportOutcome::Imported { song, target, .. } =
            report.results.into_iter().next().expect("one")
        else {
            panic!("the import must succeed under a numbered name");
        };
        assert_eq!(
            target.display(),
            "歌手/歌手 - 晴天 (2).flac",
            "minimal (n) after the occupied name"
        );

        let root_dir = g.fixture.fs.root_path(g.fixture.root).expect("root");
        assert_eq!(
            std::fs::read(root_dir.join("歌手/歌手 - 晴天.flac")).expect("original"),
            b"original-content",
            "the existing file is never replaced"
        );
        assert_eq!(
            std::fs::read(root_dir.join("歌手/歌手 - 晴天 (2).flac")).expect("new"),
            b"different-content"
        );
        assert_eq!(g.fixture.all_songs().len(), 2);
        let record = g.deps.songs.by_id(song).expect("query").expect("record");
        assert_eq!(
            record.path(),
            &RelativeMediaPath::new("歌手/歌手 - 晴天 (2).flac").unwrap()
        );
    }

    // -----------------------------------------------------------------------
    // Task 5.2: default naming, fallbacks, platform cleanup, truncation,
    // minimal numbering and the never-overwrite guarantee.
    // -----------------------------------------------------------------------

    #[test]
    fn default_target_is_artist_folder_with_artist_minus_title() {
        let g = gated();
        // The original extension case is normalized to lowercase for the
        // deterministic target name (`.FLAC` → `.flac`).
        g.sources.add("hit", "晴天.FLAC", b"hit-bytes");
        tagged(&g, b"hit-bytes", Some("周杰伦"), Some("晴天"));
        g.fixture
            .set_audio("周杰伦/周杰伦 - 晴天.flac", "晴天", 269_000);

        let report = PlanImport::new(&g.deps, &g.sources)
            .run(g.fixture.root, &[source("hit")])
            .expect("batch-level success");
        let ImportOutcome::Imported { song, target, .. } =
            report.results.into_iter().next().expect("one result")
        else {
            panic!("the tagged input must import");
        };

        assert_eq!(
            target.display(),
            "周杰伦/周杰伦 - 晴天.flac",
            "target is 歌手/歌手 - 歌曲名.扩展名"
        );
        let published = g
            .fixture
            .fs
            .root_path(g.fixture.root)
            .expect("root")
            .join(target.normalized());
        assert_eq!(
            std::fs::read(&published).expect("published bytes"),
            b"hit-bytes"
        );
        let record = g.deps.songs.by_id(song).expect("query").expect("record");
        assert_eq!(record.path(), &target);
        assert_eq!(record.title(), Some("晴天"));
    }

    #[test]
    fn missing_or_blank_tags_fall_back_to_deterministic_names() {
        let g = gated();
        // No tags at all → 未知艺人/未知艺人 - 未命名歌曲.flac.
        g.sources.add("none", "mystery.flac", b"bytes-none");
        g.fixture
            .set_audio("未知艺人/未知艺人 - 未命名歌曲.flac", "未命名歌曲", 1_000);
        // Blank (whitespace-only) tags count as missing → same fallback, so
        // this input collides with the first and takes the minimal (2).
        g.sources.add("blank", "blank.flac", b"bytes-blank");
        tagged(&g, b"bytes-blank", Some("   "), Some("\u{3000}"));
        g.fixture.set_audio(
            "未知艺人/未知艺人 - 未命名歌曲 (2).flac",
            "未命名歌曲",
            2_000,
        );
        // Title only → the artist still takes 未知艺人.
        g.sources.add("title-only", "t.flac", b"bytes-title");
        tagged(&g, b"bytes-title", Some("\t"), Some("晴天"));
        g.fixture
            .set_audio("未知艺人/未知艺人 - 晴天.flac", "晴天", 3_000);
        // Artist only → the title still takes 未命名歌曲.
        g.sources.add("artist-only", "a.flac", b"bytes-artist");
        tagged(&g, b"bytes-artist", Some("周杰伦"), None);
        g.fixture
            .set_audio("周杰伦/周杰伦 - 未命名歌曲.flac", "未命名歌曲", 4_000);

        let report = PlanImport::new(&g.deps, &g.sources)
            .run(
                g.fixture.root,
                &[
                    source("none"),
                    source("blank"),
                    source("title-only"),
                    source("artist-only"),
                ],
            )
            .expect("batch-level success");

        let targets: Vec<&str> = report
            .results
            .iter()
            .map(|outcome| match outcome {
                ImportOutcome::Imported { target, .. } => target.display(),
                other => panic!("every fallback input must import: {other:?}"),
            })
            .collect();
        assert_eq!(
            targets,
            vec![
                "未知艺人/未知艺人 - 未命名歌曲.flac",
                "未知艺人/未知艺人 - 未命名歌曲 (2).flac",
                "未知艺人/未知艺人 - 晴天.flac",
                "周杰伦/周杰伦 - 未命名歌曲.flac",
            ]
        );
        assert_eq!(g.fixture.all_songs().len(), 4);
    }

    #[test]
    fn tag_text_is_cleaned_for_platform_safe_targets() {
        let g = gated();
        // Forbidden separators/wildcards in tags, plus a control character
        // (dropped, not replaced) — cleaned on every platform's behalf.
        g.sources.add("messy", "track.flac", b"bytes-messy");
        tagged(&g, b"bytes-messy", Some("AC/DC"), Some("问\u{1}春*归?"));
        g.fixture
            .set_audio("AC_DC/AC_DC - 问春_归_.flac", "问春归", 1_000);
        // A Windows reserved device name as the artist.
        g.sources.add("con", "demo.flac", b"bytes-con");
        tagged(&g, b"bytes-con", Some("CON"), Some("Demo"));
        g.fixture.set_audio("CON_/CON_ - Demo.flac", "Demo", 2_000);

        let report = PlanImport::new(&g.deps, &g.sources)
            .run(g.fixture.root, &[source("messy"), source("con")])
            .expect("batch-level success");

        let targets: Vec<&str> = report
            .results
            .iter()
            .map(|outcome| match outcome {
                ImportOutcome::Imported { target, .. } => target.display(),
                other => panic!("every cleaned input must import: {other:?}"),
            })
            .collect();
        assert_eq!(
            targets,
            vec!["AC_DC/AC_DC - 问春_归_.flac", "CON_/CON_ - Demo.flac"]
        );
        // No platform-forbidden character survives inside any component
        // (the one `/` per target is the separator the builder itself emits).
        for target in &targets {
            let components: Vec<&str> = target.split('/').collect();
            assert_eq!(components.len(), 2, "artist/file form: {target}");
            for component in components {
                for c in ['\\', ':', '*', '?', '"', '<', '>', '|'] {
                    assert!(!component.contains(c), "{c:?} survived in {target}");
                }
            }
        }
        let root_dir = g.fixture.fs.root_path(g.fixture.root).expect("root");
        assert!(root_dir.join("AC_DC/AC_DC - 问春_归_.flac").exists());
        assert!(root_dir.join("CON_/CON_ - Demo.flac").exists());
    }

    #[test]
    fn oversized_components_are_truncated_with_short_hash_and_keep_extension() {
        let g = gated();
        // A far-over-cap title (CJK: 3 bytes per char).
        let long_title = "这是一段非常长的歌曲标题".repeat(12);
        let expected_title_target = format!(
            "{}/{}",
            target_artist_component(Some("周杰伦"), TARGET_COMPONENT_BYTES),
            truncate_component_with_extension(
                &target_file_stem(Some("周杰伦"), Some(&long_title)),
                "flac",
                TARGET_COMPONENT_BYTES
            )
        );
        g.sources.add("long", "long.flac", b"bytes-long");
        tagged(&g, b"bytes-long", Some("周杰伦"), Some(&long_title));
        g.fixture.set_audio(&expected_title_target, "长标题", 1_000);
        // A far-over-cap artist: the directory component is bounded too.
        let long_artist = "很长的艺人名字组合".repeat(30);
        let expected_artist_target = format!(
            "{}/{}",
            target_artist_component(Some(&long_artist), TARGET_COMPONENT_BYTES),
            truncate_component_with_extension(
                &target_file_stem(Some(&long_artist), Some("短")),
                "flac",
                TARGET_COMPONENT_BYTES
            )
        );
        g.sources.add("long-artist", "long2.flac", b"bytes-la");
        tagged(&g, b"bytes-la", Some(&long_artist), Some("短"));
        g.fixture.set_audio(&expected_artist_target, "短", 2_000);

        let report = PlanImport::new(&g.deps, &g.sources)
            .run(g.fixture.root, &[source("long"), source("long-artist")])
            .expect("batch-level success");

        let targets: Vec<&str> = report
            .results
            .iter()
            .map(|outcome| match outcome {
                ImportOutcome::Imported { target, .. } => target.display(),
                other => panic!("the truncated input must import: {other:?}"),
            })
            .collect();
        assert_eq!(
            targets,
            vec![expected_title_target, expected_artist_target],
            "the target follows the domain truncation rule exactly"
        );
        for target in &targets {
            let (dir, file) = target.rsplit_once('/').expect("artist/file form");
            assert!(dir.len() <= TARGET_COMPONENT_BYTES, "{dir}");
            assert!(file.len() <= TARGET_COMPONENT_BYTES, "{file}");
            assert!(
                std::path::Path::new(file)
                    .extension()
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("flac")),
                "extension survives: {file}"
            );
            assert!(file.contains('~'), "short-hash suffix present: {file}");
        }
        // The two truncations stay distinguishable.
        assert_ne!(targets[0], targets[1]);
        let root_dir = g.fixture.fs.root_path(g.fixture.root).expect("root");
        assert!(root_dir.join(targets[0]).exists());
        assert!(root_dir.join(targets[1]).exists());
    }

    #[test]
    fn minimal_numbering_skips_occupied_suffixes() {
        let g = gated();
        // Both the base name and its first numbered successor are taken
        // (library record + on-disk file each); the import must land on (3).
        g.fixture.write_file("歌手/歌手 - 晴天.flac", b"content-1");
        seed_song(&g, "歌手/歌手 - 晴天.flac", b"content-1");
        g.fixture
            .write_file("歌手/歌手 - 晴天 (2).flac", b"content-2");
        seed_song(&g, "歌手/歌手 - 晴天 (2).flac", b"content-2");
        g.sources.add("new", "晴天.flac", b"content-3");
        tagged(&g, b"content-3", Some("歌手"), Some("晴天"));
        g.fixture
            .set_audio("歌手/歌手 - 晴天 (3).flac", "晴天", 1_000);

        let report = PlanImport::new(&g.deps, &g.sources)
            .run(g.fixture.root, &[source("new")])
            .expect("batch-level success");
        let ImportOutcome::Imported { target, .. } =
            report.results.into_iter().next().expect("one")
        else {
            panic!("the import must succeed under a numbered name");
        };
        assert_eq!(target.display(), "歌手/歌手 - 晴天 (3).flac");
        let root_dir = g.fixture.fs.root_path(g.fixture.root).expect("root");
        assert_eq!(
            std::fs::read(root_dir.join("歌手/歌手 - 晴天.flac")).expect("first"),
            b"content-1",
            "occupied names are never replaced"
        );
        assert_eq!(
            std::fs::read(root_dir.join("歌手/歌手 - 晴天 (2).flac")).expect("second"),
            b"content-2"
        );
        assert_eq!(g.fixture.all_songs().len(), 3);
    }

    #[test]
    fn numbering_applies_within_one_batch() {
        let g = gated();
        // Same tags, different content, one batch: the second input must not
        // claim the first input's freshly planned target.
        g.sources.add("first", "one.flac", b"content-1");
        tagged(&g, b"content-1", Some("歌手"), Some("同名"));
        g.fixture.set_audio("歌手/歌手 - 同名.flac", "同名", 1_000);
        g.sources.add("second", "two.flac", b"content-2");
        tagged(&g, b"content-2", Some("歌手"), Some("同名"));
        g.fixture
            .set_audio("歌手/歌手 - 同名 (2).flac", "同名", 2_000);

        let report = PlanImport::new(&g.deps, &g.sources)
            .run(g.fixture.root, &[source("first"), source("second")])
            .expect("batch-level success");
        let targets: Vec<&str> = report
            .results
            .iter()
            .map(|outcome| match outcome {
                ImportOutcome::Imported { target, .. } => target.display(),
                other => panic!("both inputs must import: {other:?}"),
            })
            .collect();
        assert_eq!(
            targets,
            vec!["歌手/歌手 - 同名.flac", "歌手/歌手 - 同名 (2).flac"]
        );
        assert_eq!(g.fixture.all_songs().len(), 2, "two records, two files");
    }

    #[test]
    fn import_renumbers_around_files_that_exist_only_on_disk() {
        let g = gated();
        // A file at the exact tag-derived target with NO library record
        // (external placement, or a database that was dropped): planning must
        // still not claim the name.
        g.fixture
            .write_file("歌手/歌手 - 晴天.flac", b"someone-else");
        g.sources.add("new", "晴天.flac", b"fresh-bytes");
        tagged(&g, b"fresh-bytes", Some("歌手"), Some("晴天"));
        g.fixture
            .set_audio("歌手/歌手 - 晴天 (2).flac", "晴天", 1_000);

        let report = PlanImport::new(&g.deps, &g.sources)
            .run(g.fixture.root, &[source("new")])
            .expect("batch-level success");
        let ImportOutcome::Imported { target, .. } =
            report.results.into_iter().next().expect("one")
        else {
            panic!("the import must succeed under a numbered name");
        };
        assert_eq!(target.display(), "歌手/歌手 - 晴天 (2).flac");
        let root_dir = g.fixture.fs.root_path(g.fixture.root).expect("root");
        assert_eq!(
            std::fs::read(root_dir.join("歌手/歌手 - 晴天.flac")).expect("existing"),
            b"someone-else",
            "the disk-only file keeps its bytes"
        );
        assert_eq!(g.fixture.all_songs().len(), 1, "only the import");
    }

    /// A fs wrapper that plants a file at a chosen target during the
    /// `stage_stream` — the watcher/other-process race window between the
    /// batch snapshot and the publish. The port's create-new contract must
    /// absorb it: the import fails, the existing file survives.
    struct RacePlanter {
        inner: FakeLibraryFileSystem,
        plant: std::sync::Mutex<Option<String>>,
    }

    impl LibraryFileSystem for RacePlanter {
        fn enumerate(&self, root: LibraryRootId) -> Result<Vec<RelativeMediaPath>, Error> {
            self.inner.enumerate(root)
        }
        fn file_meta(
            &self,
            root: LibraryRootId,
            path: &RelativeMediaPath,
        ) -> Result<FileMeta, Error> {
            self.inner.file_meta(root, path)
        }
        fn read_head(
            &self,
            root: LibraryRootId,
            path: &RelativeMediaPath,
            limit: u64,
        ) -> Result<Vec<u8>, Error> {
            self.inner.read_head(root, path, limit)
        }
        fn publish(
            &self,
            root: LibraryRootId,
            staged: &StagedResource,
            target: &RelativeMediaPath,
        ) -> Result<(), Error> {
            self.inner.publish(root, staged, target)
        }
        fn stage(
            &self,
            root: LibraryRootId,
            staged: &StagedResource,
            content: &[u8],
        ) -> Result<(), Error> {
            self.inner.stage(root, staged, content)
        }
        fn stage_stream(
            &self,
            root: LibraryRootId,
            staged: &StagedResource,
            content: &mut dyn Read,
        ) -> Result<StagedCopy, Error> {
            let planted = self.plant.lock().unwrap().take();
            if let Some(rel) = planted {
                let base = self.inner.root_path(root).expect("root");
                let dest = base.join(&rel);
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent).expect("plant mkdir");
                }
                std::fs::write(&dest, b"planted-first").expect("plant write");
            }
            self.inner.stage_stream(root, staged, content)
        }
        fn read_staged(
            &self,
            root: LibraryRootId,
            staged: &StagedResource,
        ) -> Result<Vec<u8>, Error> {
            self.inner.read_staged(root, staged)
        }
        fn discard_staged(
            &self,
            root: LibraryRootId,
            staged: &StagedResource,
        ) -> Result<(), Error> {
            self.inner.discard_staged(root, staged)
        }
        fn write_capable(&self, root: LibraryRootId) -> Result<bool, Error> {
            self.inner.write_capable(root)
        }
    }

    #[test]
    fn publish_never_overwrites_a_target_created_after_planning() {
        let g = gated();
        let planter = Arc::new(RacePlanter {
            inner: g.fixture.fs.clone(),
            plant: std::sync::Mutex::new(Some("歌手/歌手 - 晴天.flac".to_owned())),
        });
        let deps = {
            let fs: Arc<dyn LibraryFileSystem> = planter;
            ScanDeps {
                fs,
                ..ScanDeps::clone(&g.deps)
            }
        };
        g.sources.add("new", "晴天.flac", b"new-bytes");
        tagged(&g, b"new-bytes", Some("歌手"), Some("晴天"));
        g.fixture.set_audio("歌手/歌手 - 晴天.flac", "晴天", 1_000);

        let report = PlanImport::new(&deps, &g.sources)
            .run(g.fixture.root, &[source("new")])
            .expect("batch-level success");
        let ImportOutcome::Failed { code, .. } = report.results.into_iter().next().expect("one")
        else {
            panic!("the raced import must fail, never overwrite");
        };
        assert_eq!(code, "conflict", "publish is create-new");

        // The planted file keeps its bytes; nothing entered the library; the
        // operation's claim was released for a clean retry.
        let root_dir = g.fixture.fs.root_path(g.fixture.root).expect("root");
        assert_eq!(
            std::fs::read(root_dir.join("歌手/歌手 - 晴天.flac")).expect("planted"),
            b"planted-first"
        );
        assert!(g.fixture.all_songs().is_empty());
        assert_eq!(g.fixture.database.released_claims().len(), 1);
    }

    // -----------------------------------------------------------------------
    // Task 5.3: streaming copy + BLAKE3, per-resource journal fields, the
    // exclusive publish boundary, staged-copy cleanup, DB visibility only
    // after the full audio publish, source invariance and staging isolation —
    // verified with injected temp directories, never the real user home.
    // -----------------------------------------------------------------------

    #[test]
    fn import_streams_the_copy_and_computes_blake3_during_it() {
        let g = gated();
        // Larger than the 64 KiB copy chunk: the chunked pump is exercised
        // end to end and the digest must equal the full-content BLAKE3.
        let content: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        g.sources.add("big", "大文件.flac", &content);
        tagged(&g, &content, Some("歌手"), Some("大文件"));
        g.fixture
            .set_audio("歌手/歌手 - 大文件.flac", "大文件", 1_000);

        let report = PlanImport::new(&g.deps, &g.sources)
            .run(g.fixture.root, &[source("big")])
            .expect("batch-level success");
        let ImportOutcome::Imported { song, target, .. } = &report.results[0] else {
            panic!("the streamed input must import");
        };
        assert_eq!(target.display(), "歌手/歌手 - 大文件.flac");

        // The published file holds every byte of the streamed copy.
        let published = g
            .fixture
            .fs
            .root_path(g.fixture.root)
            .expect("root")
            .join("歌手/歌手 - 大文件.flac");
        assert_eq!(
            std::fs::read(&published).expect("published bytes"),
            content,
            "the streaming copy is byte-exact"
        );
        // The journal carries the BLAKE3 computed during the copy.
        let operation = operation_of(&report).expect("operation");
        let items = g.deps.journal.items(operation).expect("journal items");
        let expected_hash = g.deps.hasher.hash_of_bytes(&content);
        assert_eq!(
            items.first().map(|item| item.expected_hash.as_str()),
            Some(expected_hash.as_str()),
        );
        // The committed record carries the same content hash and identity.
        let record = g.deps.songs.by_id(*song).expect("query").expect("record");
        assert_eq!(record.blake3_hash(), Some(expected_hash.as_str()));
        // The source was streamed exactly once (no second planning read).
        assert_eq!(g.sources.read_keys(), vec!["big".to_owned()]);
    }

    #[test]
    fn journal_item_records_source_staging_target_and_expected_hash() {
        let g = gated();
        g.sources.add("good", "晴天.flac", b"journal-bytes");
        tagged(&g, b"journal-bytes", Some("歌手"), Some("晴天"));
        g.fixture.set_audio("歌手/歌手 - 晴天.flac", "晴天", 1_000);

        let report = PlanImport::new(&g.deps, &g.sources)
            .run(g.fixture.root, &[source("good")])
            .expect("batch-level success");
        let ImportOutcome::Imported {
            operation,
            song,
            target,
        } = report.results.into_iter().next().expect("one result")
        else {
            panic!("the input must import");
        };

        // The per-resource item (design §8: 逐资源源定位/暂存/目标/hash).
        let items = g.deps.journal.items(operation).expect("journal items");
        assert_eq!(items.len(), 1);
        let item = &items[0];
        assert_eq!(item.kind, OperationResourceKind::Audio);
        assert_eq!(item.state, OperationState::Completed);
        assert_eq!(item.song, Some(song), "UUID equals the reservation");
        assert_eq!(
            item.source.as_deref(),
            Some("good"),
            "the logical source locator is recorded, never a path"
        );
        let staging = item.staging_path.as_ref().expect("staged location");
        assert!(
            staging.display().starts_with(".echo-test-staging/import/"),
            "staged inside the operation's slot: {}",
            staging.display()
        );
        assert!(
            staging.display().ends_with("/audio"),
            "one slot per operation resource: {}",
            staging.display()
        );
        assert_eq!(item.target_path, target);
        assert_eq!(item.claim_key, target.identity_key());
        assert_eq!(
            item.expected_hash,
            g.deps.hasher.hash_of_bytes(b"journal-bytes"),
        );
        // The operation envelope exists and is bound to the root (design §8:
        // 每个 operation 由总状态和逐资源 operation_items 组成).
        assert_eq!(
            g.fixture
                .database
                .envelope_of(operation)
                .map(|(root, _)| root),
            Some(g.fixture.root)
        );
        // Terminal state released the claim.
        assert_eq!(g.fixture.database.released_claims(), vec![operation]);
    }

    #[test]
    fn database_record_is_visible_only_after_the_full_audio_is_published() {
        let fixture = ScanFixture::new();
        let probe = Arc::new(VisibilityProbe {
            inner: fixture.fs.clone(),
            songs: fixture.database.clone(),
            root: fixture.root,
            events: std::sync::Mutex::new(Vec::new()),
        });
        let deps = {
            let fs: Arc<dyn LibraryFileSystem> = probe.clone();
            ScanDeps {
                fs,
                ..ScanDeps::clone(&fixture.deps)
            }
        };
        let sources = FakeImportSources::new();
        sources.add("good", "晴天.flac", b"visible-bytes");
        tag_content(&fixture, b"visible-bytes", Some("歌手"), Some("晴天"));
        fixture.set_audio("歌手/歌手 - 晴天.flac", "晴天", 1_000);

        let report = PlanImport::new(&deps, &sources)
            .run(fixture.root, &[source("good")])
            .expect("batch-level success");
        let ImportOutcome::Imported { song, .. } = report.results.into_iter().next().expect("one")
        else {
            panic!("the input must import");
        };

        // The song table was empty at staging time AND at publish time: the
        // record only becomes visible through the commit afterwards.
        assert_eq!(
            probe.events(),
            vec![("stage_stream", 0), ("publish", 0)],
            "no database record before the full audio publish"
        );
        // The published audio is complete (byte-identical), then committed
        // under the reserved identity.
        let published = fixture
            .fs
            .root_path(fixture.root)
            .expect("root")
            .join("歌手/歌手 - 晴天.flac");
        assert_eq!(
            std::fs::read(&published).expect("published"),
            b"visible-bytes"
        );
        let songs = fixture.all_songs();
        assert_eq!(songs.len(), 1);
        assert_eq!(songs[0].id(), song);
        assert_eq!(songs[0].path().display(), "歌手/歌手 - 晴天.flac");
    }

    #[test]
    fn duplicate_and_failed_inputs_discard_their_staged_copies() {
        let g = gated();
        let existing = seed_song(&g, "已有/other.flac", b"duplicate-content");
        // The duplicate's staged copy is created (dedup runs after the
        // streaming copy) and must be discarded without a claim.
        g.sources.add("dup", "重复.flac", b"duplicate-content");
        // The failed input hits a publish-time conflict (a file appears at
        // its planned target between the batch snapshot and the publish).
        let planter = Arc::new(RacePlanter {
            inner: g.fixture.fs.clone(),
            plant: std::sync::Mutex::new(Some("歌手/歌手 - 晴天.flac".to_owned())),
        });
        let deps = {
            let fs: Arc<dyn LibraryFileSystem> = planter;
            ScanDeps {
                fs,
                ..ScanDeps::clone(&g.deps)
            }
        };
        g.sources.add("new", "晴天.flac", b"fresh-bytes");
        tagged(&g, b"fresh-bytes", Some("歌手"), Some("晴天"));
        g.fixture.set_audio("歌手/歌手 - 晴天.flac", "晴天", 1_000);

        let report = PlanImport::new(&deps, &g.sources)
            .run(g.fixture.root, &[source("dup"), source("new")])
            .expect("batch-level success");
        assert_eq!(
            report.results[0],
            ImportOutcome::Duplicate {
                existing: existing.id()
            }
        );
        let ImportOutcome::Failed { code, .. } = &report.results[1] else {
            panic!("the raced input must fail: {:?}", report.results[1]);
        };
        assert_eq!(*code, "conflict");

        // No staged copy leaks and only the failed input ever held a claim.
        assert_eq!(
            g.fixture.fs.staged_count(),
            0,
            "staged copies are discarded"
        );
        assert_eq!(g.fixture.database.released_claims().len(), 1);
        assert_eq!(g.fixture.all_songs().len(), 1, "only the seed remains");
        // The conflicting file keeps its bytes (never overwritten).
        let root_dir = g.fixture.fs.root_path(g.fixture.root).expect("root");
        assert_eq!(
            std::fs::read(root_dir.join("歌手/歌手 - 晴天.flac")).expect("planted"),
            b"planted-first"
        );
    }

    /// The REAL root-constrained adapter over an injected temp library, with
    /// its controlled staging directory established (activation grants write
    /// capability by creating it — design §8: 首次获得写能力时
    /// exclusive-create; the runtime wires this, the test mirrors it).
    fn real_adapter_over(library: &std::path::Path) -> (LibraryRootId, Arc<dyn LibraryFileSystem>) {
        let root = LibraryRootId::new();
        let registry = crate::infrastructure::filesystem::registry::RootRegistry::new();
        registry.register(root, library);
        let real_fs =
            crate::infrastructure::filesystem::adapter::RootConstrainedFileSystem::new(registry);
        real_fs
            .staging()
            .ensure_dir(root)
            .expect("staging established");
        (root, Arc::new(real_fs))
    }

    /// Deterministic probe/tags fixtures for the full-stack import test: the
    /// published path probes as audio, and the source content parses to the
    /// 歌手/晴天 tags that drive the target name.
    fn seeded_reader_fixtures() -> (FakeMediaProbe, FakeMetadataReader) {
        let tags = ParsedMetadata {
            artist: Some("歌手".to_owned()),
            title: Some("晴天".to_owned()),
            ..ParsedMetadata::default()
        };
        let probe = FakeMediaProbe::new();
        probe.set(
            "歌手/歌手 - 晴天.flac",
            crate::application::ports::ProbeOutcome::Audio {
                format: AudioFormat::Flac,
                duration: Some(std::time::Duration::from_secs(1)),
            },
        );
        let metadata = FakeMetadataReader::new();
        metadata.set("歌手/歌手 - 晴天.flac", tags.clone());
        metadata.set_bytes(b"original-source-bytes", tags);
        (probe, metadata)
    }

    /// `ScanDeps` over the REAL root-constrained adapter (an injected temp
    /// library) with the deterministic doubles everywhere else.
    fn real_fs_deps(
        fs: &Arc<dyn LibraryFileSystem>,
        probe: FakeMediaProbe,
        metadata: FakeMetadataReader,
        database: &MemoryDatabase,
    ) -> ScanDeps {
        ScanDeps {
            roots: Arc::new(database.clone()),
            songs: Arc::new(database.clone()),
            lyrics: Arc::new(database.clone()),
            covers: Arc::new(database.clone()),
            runs: Arc::new(database.clone()),
            journal: Arc::new(database.clone()),
            uow: Arc::new(database.clone()),
            fs: Arc::clone(fs),
            probe: Arc::new(probe),
            metadata: Arc::new(metadata),
            hasher: Arc::new(FakeFileHasher::new(Arc::clone(fs))),
            lyrics_parser: Arc::new(FakeLyricsParser::new()),
            cover_cache: Arc::new(MemoryCoverCache::new()),
            ids: Arc::new(FakeIdGenerator::new()),
            clock: Arc::new(ManualClock::new()),
            config: ScanConfig::default(),
        }
    }

    #[test]
    fn source_files_stay_unchanged_and_foreign_staging_names_are_isolated() {
        let external = tempfile::tempdir().expect("external temp dir");
        let library = tempfile::tempdir().expect("library temp dir");
        let source_path = external.path().join("晴天.flac");
        std::fs::write(&source_path, b"original-source-bytes").expect("write source file");
        let modified_before = std::fs::metadata(&source_path)
            .expect("stat source")
            .modified()
            .expect("mtime");

        let (root, fs) = real_adapter_over(library.path());

        // A user directory that happens to carry Echo's staging prefix.
        let foreign = library.path().join(".echo-staging-user-owned");
        std::fs::create_dir_all(&foreign).expect("foreign dir");
        std::fs::write(foreign.join("user-file.txt"), b"user-content").expect("foreign file");

        let (probe, metadata) = seeded_reader_fixtures();
        let database = MemoryDatabase::new();
        let deps = real_fs_deps(&fs, probe, metadata, &database);
        let sources = TempFileSources::new();
        sources.add("hit", "晴天.flac", &source_path);

        let report = PlanImport::new(&deps, &sources)
            .run(root, &[source("hit")])
            .expect("batch-level success");
        let ImportOutcome::Imported {
            song: _, target, ..
        } = &report.results[0]
        else {
            panic!("the import must succeed: {:?}", report.results[0]);
        };
        assert_eq!(target.display(), "歌手/歌手 - 晴天.flac");

        // 源文件内容/名称/位置不变: the source was copied, never moved.
        assert_eq!(
            std::fs::read(&source_path).expect("source bytes"),
            b"original-source-bytes"
        );
        assert!(
            source_path.is_file() && source_path.parent() == Some(external.path()),
            "the source keeps its name and location"
        );
        assert_eq!(
            std::fs::metadata(&source_path)
                .expect("stat source")
                .modified()
                .expect("mtime"),
            modified_before,
            "the source was never written to"
        );
        assert_eq!(
            sources.opened(),
            vec!["hit".to_owned()],
            "read exactly once"
        );

        // The full audio was published into the library and committed.
        assert_eq!(
            std::fs::read(library.path().join("歌手/歌手 - 晴天.flac")).expect("published"),
            b"original-source-bytes"
        );
        assert_eq!(database.songs().len(), 1);

        // 同名用户目录绝不被写入: the foreign directory keeps exactly its
        // original content; Echo staged in its own marker-verified directory.
        assert_eq!(
            std::fs::read(foreign.join("user-file.txt")).expect("user file"),
            b"user-content"
        );
        assert_eq!(std::fs::read_dir(&foreign).expect("read dir").count(), 1);
        let staging_dirs: Vec<std::path::PathBuf> = std::fs::read_dir(library.path())
            .expect("root")
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| {
                path.file_name()
                    .is_some_and(|name| name.to_string_lossy().starts_with(".echo-staging-"))
            })
            .collect();
        let owned_dirs: Vec<std::path::PathBuf> = staging_dirs
            .into_iter()
            .filter(|path| *path != foreign)
            .collect();
        assert_eq!(
            owned_dirs.len(),
            1,
            "exactly one Echo-owned staging directory: {owned_dirs:?}"
        );
        assert!(
            owned_dirs[0].join(".echo-ownership-marker").is_file(),
            "the owned staging directory carries its marker"
        );
    }
}
