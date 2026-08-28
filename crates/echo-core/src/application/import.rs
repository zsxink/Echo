//! Per-input multi-select import planning (`PlanImport`, task 5.1, design §8).
//!
//! A user-selected batch is processed **one input at a time**: each input is
//! classified (imported / duplicate content / unsupported / failed), and the
//! inputs that proceed are planned as their own operation — a fresh
//! `OperationId` and a **reserved** `SongId`, persisted in the operation
//! journal together with the target-path claim *before* the first side effect
//! (design §8: 先持久化意图，再执行调用，再持久化结果).
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
use crate::domain::text::{safe_component, truncate_component_with_extension};
use crate::error::{Error, Subject};

/// The one staging resource every import audio operation owns.
const IMPORT_AUDIO_RESOURCE: &str = "audio";
/// The `operation` classifier used by import verification errors.
const IMPORT_OPERATION: &str = "import";
/// Conservative single-component byte cap for planned targets (the OS allows
/// 255 on the major desktop filesystems; task 5.2 owns the final naming
/// rules, including the short-hash suffix, on top of this).
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

/// One input's reserved plan: the identity and target every later step (and
/// any recovery, in task 5.5) must agree on.
struct PlannedInput {
    root: LibraryRootId,
    target: RelativeMediaPath,
    operation: OperationId,
    reserved: SongId,
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

    /// The batch-start identity snapshot: every hash holder and every
    /// occupied target path of the root.
    fn batch_state(&self, root: LibraryRootId) -> Result<BatchState, Error> {
        let mut holders = HashMap::new();
        let mut taken_targets = HashSet::new();
        for song in self.deps.songs.all_in_root(root)? {
            if let Some(hash) = song.blake3_hash() {
                holders.entry(hash.to_owned()).or_insert_with(|| song.id());
            }
            taken_targets.insert(song.path().identity_key().to_owned());
        }
        Ok(BatchState {
            holders,
            taken_targets,
        })
    }

    /// Classify, plan and import one input. Never fails the batch: every
    /// failure mode becomes this input's `Failed` result (and rolls back its
    /// own journal claim) while the other inputs continue.
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
        if supported_extension_of(&info.display_name).is_none() {
            return ImportOutcome::Unsupported;
        }
        let bytes = match self.sources.read(source) {
            Ok(bytes) => bytes,
            Err(error) => return failed_of(&error),
        };
        if u64::try_from(bytes.len()).unwrap_or(u64::MAX) != info.size {
            // A source that does not match its description (vanished or
            // truncated mid-selection) never enters the library.
            return failed_of(&Error::CorruptMedia {
                operation: IMPORT_OPERATION.to_owned(),
                reason: "source content does not match the described size".to_owned(),
            });
        }
        let hash = self.deps.hasher.hash_of_bytes(&bytes);
        // Plan-time BLAKE3 dedup (task 5.6 adds the pre-commit re-check):
        // identical content returns the existing record, never a second UUID.
        if let Some(existing) = state.holders.get(&hash) {
            return ImportOutcome::Duplicate {
                existing: *existing,
            };
        }
        let Some(target) = plan_target(&info.display_name, &mut |key| {
            state.taken_targets.contains(key)
        }) else {
            return failed_of(&Error::validation(
                Subject::Path,
                "import target",
                "no safe unique target name for the source name",
            ));
        };

        let operation = self.deps.ids.new_operation_id();
        let reserved = self.deps.ids.new_song_id();
        // Persist the intent (reserved SongId + target claim + expected hash)
        // before the first side effect; the conditional unique claim keeps the
        // target path reserved until the operation reaches a terminal state.
        let intent = journal_item(
            OperationState::Planned,
            Some(reserved),
            target.clone(),
            &hash,
        );
        if let Err(error) = self.deps.journal.upsert_item(operation, intent) {
            return failed_of(&error);
        }
        state.taken_targets.insert(target.identity_key().to_owned());

        let planned = PlannedInput {
            root,
            target,
            operation,
            reserved,
            hash: hash.clone(),
            size: info.size,
        };
        match self.execute(&planned, &bytes) {
            Ok(()) => {
                state.holders.insert(hash, reserved);
                ImportOutcome::Imported {
                    operation,
                    song: reserved,
                    target: planned.target,
                }
            }
            Err(error) => {
                // Per-input rollback: the terminal journal state releases this
                // input's claim; committed inputs and the remaining batch are
                // untouched.
                let rolled_back = journal_item(
                    OperationState::RolledBack,
                    Some(reserved),
                    planned.target,
                    &hash,
                );
                let _ = self.deps.journal.upsert_item(operation, rolled_back);
                let _ = self.deps.journal.release_claims(operation);
                failed_of(&error)
            }
        }
    }

    /// Stage → publish → verify → parse → commit one planned input.
    fn execute(&self, planned: &PlannedInput, bytes: &[u8]) -> Result<(), Error> {
        let staged = StagedResource::new(planned.operation, IMPORT_AUDIO_RESOURCE)?;
        self.deps.fs.stage(planned.root, &staged, bytes)?;
        // Publish is create-new in the adapter: an occupied target is a
        // conflict, never a replacement (绝不覆盖既有文件).
        self.deps
            .fs
            .publish(planned.root, &staged, &planned.target)?;
        // `*Applied` only counts once the final location is verified: size and
        // hash must match the planned expectation (design §8).
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
        let committed = journal_item(
            OperationState::DatabaseCommitted,
            Some(planned.reserved),
            planned.target.clone(),
            &planned.hash,
        );
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
            journal_item(
                OperationState::Completed,
                Some(planned.reserved),
                planned.target.clone(),
                &planned.hash,
            ),
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

/// Plan the target file name for one source: the sanitized display name, and
/// on conflicts the minimal ` (n)` suffix that is still free. Never replaces
/// an occupied name — numbering exists so a collision cannot claim another
/// input's target (task 5.2 owns the full artist/title naming rules).
fn plan_target(
    display_name: &str,
    is_taken: &mut dyn FnMut(&str) -> bool,
) -> Option<RelativeMediaPath> {
    let file_name = display_name.rsplit(['/', '\\']).next()?;
    let (stem, ext) = file_name.rsplit_once('.')?;
    if stem.is_empty() || ext.is_empty() {
        return None;
    }
    let base = safe_component(stem);
    for suffix in 1..=MAX_NUMBERING {
        let stem = if suffix == 1 {
            base.clone()
        } else {
            format!("{base} ({suffix})")
        };
        let name = truncate_component_with_extension(&stem, ext, TARGET_COMPONENT_BYTES);
        if let Ok(path) = RelativeMediaPath::new(&name) {
            if !is_taken(path.identity_key()) {
                return Some(path);
            }
        }
    }
    None
}

/// One journal item row for the import audio resource.
fn journal_item(
    state: OperationState,
    song: Option<SongId>,
    target: RelativeMediaPath,
    expected_hash: &str,
) -> OperationItem {
    OperationItem {
        kind: OperationResourceKind::Audio,
        state,
        song,
        claim_key: target.identity_key().to_owned(),
        target_path: target,
        expected_hash: expected_hash.to_owned(),
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
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::application::ports::{
        FileMeta, LibraryFileSystem, OperationJournalRepository, SongRepository as _,
    };
    use crate::application::testing::ScanFixture;
    use crate::application::testing::{FakeImportSources, FakeLibraryFileSystem, MemoryDatabase};
    use crate::domain::entities::Song;
    use crate::domain::ids::Revision;
    use crate::domain::media::AudioFormat;

    /// A fs wrapper that refuses every side effect unless the operation's
    /// journal reservation (`Planned` item with a reserved `SongId`) is already
    /// persisted — the test double that proves 先持久化意图，再执行调用.
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
            self.verify_reserved(staged.operation());
            self.inner.stage(root, staged, content)
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

    #[test]
    fn mixed_batch_reports_each_input_independently() {
        let g = gated();
        // An existing library record owning some content, for the duplicate.
        let existing = seed_song(&g, "已有/other.flac", b"library-original");
        // The successful input: a supported audio file whose published target
        // probes and tags cleanly.
        g.sources.add("good", "晴天.flac", b"sunny-bytes");
        g.fixture.set_audio("晴天.flac", "晴天", 269_000);
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
        assert_eq!(target.display(), "晴天.flac");
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
            .join("晴天.flac");
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
        assert_eq!(record.path(), &RelativeMediaPath::new("晴天.flac").unwrap());
        assert!(violations(&g).is_empty());
    }

    #[test]
    fn import_reserves_operation_and_song_ids_before_side_effects() {
        let g = gated();
        g.sources.add("good", "晴天.flac", b"reserved-bytes");
        g.fixture.set_audio("晴天.flac", "晴天", 269_000);

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
        g.fixture.set_audio("a.flac", "A", 1_000);
        g.sources.fail("b", "外部文件不可读");
        g.sources.add("c", "c.flac", b"content-c");
        g.fixture.set_audio("c.flac", "C", 3_000);

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
        assert!(root_dir.join("a.flac").exists());
        assert!(root_dir.join("c.flac").exists());
        // The failed input left no claim behind: retries start clean.
        assert_eq!(g.fixture.database.released_claims().len(), 2);
        assert!(violations(&g).is_empty());
    }

    #[test]
    fn duplicate_content_within_a_batch_creates_one_record() {
        let g = gated();
        g.sources.add("x", "tune.flac", b"same-content");
        g.fixture.set_audio("tune.flac", "Tune", 1_000);
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
        g.fixture.write_file("song.flac", b"original-content");
        seed_song(&g, "song.flac", b"original-content");
        g.sources.add("new", "song.flac", b"different-content");
        g.fixture.set_audio("song (2).flac", "Second", 1_000);

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
            "song (2).flac",
            "minimal (n) after the occupied name"
        );

        let root_dir = g.fixture.fs.root_path(g.fixture.root).expect("root");
        assert_eq!(
            std::fs::read(root_dir.join("song.flac")).expect("original"),
            b"original-content",
            "the existing file is never replaced"
        );
        assert_eq!(
            std::fs::read(root_dir.join("song (2).flac")).expect("new"),
            b"different-content"
        );
        assert_eq!(g.fixture.all_songs().len(), 2);
        let record = g.deps.songs.by_id(song).expect("query").expect("record");
        assert_eq!(
            record.path(),
            &RelativeMediaPath::new("song (2).flac").unwrap()
        );
    }
}
