//! Per-input multi-select import planning (`PlanImport`, tasks 5.1/5.2,
//! design §8).
//!
//! A user-selected batch is processed **one input at a time**: each input is
//! classified (imported / duplicate content / unsupported / failed), and the
//! inputs that proceed are planned as their own operation — a fresh
//! `OperationId` and a **reserved** `SongId`, persisted in the operation
//! journal together with the target-path claim *before* the first side effect
//! (design §8: 先持久化意图，再执行调用，再持久化结果).
//!
//! The target is planned from the *parsed tags* (task 5.2): `歌手/歌手 -
//! 歌曲名.原扩展名`, with the visible fallbacks 未知艺人 / 未命名歌曲 for
//! missing tags, platform-safe component cleanup, short-hash truncation for
//! oversized components and the minimal ` (n)` that neither a library record,
//! an earlier batch input nor an existing on-disk file owns.
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
        // The extension is both the type filter and the target's 原扩展名
        // (normalized to lowercase for a deterministic target name).
        let Some(ext) = supported_extension_of(&info.display_name) else {
            return ImportOutcome::Unsupported;
        };
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
        // The target is named from the parsed tags (task 5.2); unparseable
        // content fails this input instead of guessing a name.
        let meta = match self.deps.metadata.read_bytes(&bytes) {
            Ok(meta) => meta,
            Err(error) => return failed_of(&error),
        };
        let Some(target) = plan_named_target(
            meta.artist.as_deref(),
            meta.title.as_deref(),
            &ext,
            &mut |key| state.taken_targets.contains(key),
        ) else {
            return failed_of(&Error::validation(
                Subject::Path,
                "import target",
                "no safe unique target name for the parsed tags",
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

    /// A fs wrapper that plants a file at a chosen target during the first
    /// `stage` — the watcher/other-process race window between planning and
    /// publishing. The port's create-new contract must absorb it: the import
    /// fails, the existing file survives.
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
            let planted = self.plant.lock().unwrap().take();
            if let Some(rel) = planted {
                let base = self.inner.root_path(root).expect("root");
                let dest = base.join(&rel);
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent).expect("plant mkdir");
                }
                std::fs::write(&dest, b"planted-first").expect("plant write");
            }
            self.inner.stage(root, staged, content)
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
}
