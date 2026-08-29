//! Import crash recovery (`RecoverOperations`, tasks 5.5 / 5.10, design §8).
//!
//! After a crash the application's *durable* truth is the operation journal
//! plus the locations each per-resource item points at: the staged copy, the
//! final target file, and the expected BLAKE3. This use case reads every
//! incomplete journal item of a root and drives each to a single, idempotent
//! terminal state by checking existence and hash at those locations — never
//! by trusting an in-memory call that "returned" before the crash.
//!
//! Per-item matrix (design §8 startup recovery rules):
//!
//! - **target already correct** (present + expected hash) → the publish is
//!   effectively applied: roll forward — for the audio resource, write the
//!   song record under the journal's **reserved** `SongId` (no new UUID), then
//!   advance `PublishApplied → DatabaseCommitted → Completed`.
//! - **valid staged copy, target absent or our own empty reservation** → retry
//!   the exclusive publish from the persisted staging path (只有暂存正确则重试
//!   exclusive publish), verify, then roll forward as above.
//! - **target occupied by different content** → `FailedRecoverable`: never
//!   overwrite; hold the target claim so nothing else claims it, and surface a
//!   conflict for manual resolution.
//! - **no correct target and no valid staging, not yet published** → roll back:
//!   clean the safe residual staged file, mark `RolledBack`, release the claim.
//! - An item that *claims* a publish (`publish_committed`) but whose file is
//!   missing is `FailedRecoverable` — the design never infers success from a
//!   missing path.
//!
//! Recovery is repeatable: re-running it is a no-op once every operation has
//! reached a terminal state, so the unique terminal state, the reserved UUID,
//! the absence of orphan final files, duplicates and ghost records all hold
//! across any number of recoveries.

use crate::application::ports::{OperationItem, OperationResourceKind, TxAccess};
use crate::application::relink::song_from_parsed;
use crate::application::scan::{parse_single_file, rewrap, FileOutcome, ScanDeps};
use crate::domain::entities::LyricsSource;
use crate::domain::ids::{LibraryRootId, OperationId, RelativeMediaPath};
use crate::domain::state::OperationState;
use crate::error::Error;

/// The journal `kind` of an import operation (recovery only handles imports in
/// 0.1.0; delete/restore recovery is tasks 5.7–5.8).
const IMPORT_OPERATION: &str = "import";

/// Per-item recovery result.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ItemResult {
    /// The item was rolled forward/back to a durable terminal state.
    Terminal,
    /// A conflicting/foreign target that must not be overwritten; the claim
    /// stays held and the operation reports a conflict.
    Conflict,
}

/// One operation's recovery summary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveryOperation {
    pub operation: OperationId,
    pub items: usize,
    /// Whether any item ended in `FailedRecoverable` (conflict held).
    pub held: bool,
}

/// The aggregate recovery report for one root.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct RecoveryReport {
    /// Operations that were not already terminal.
    pub touched: Vec<RecoveryOperation>,
}

/// The startup/retry recovery use case: finish or safely dispose every
/// incomplete import operation of a root. Blocking (file hashing, parsing,
/// publish): the runtime calls it on a worker thread before playback/watcher
/// start (task 5.10).
pub struct RecoverOperations<'a> {
    deps: &'a ScanDeps,
}

impl<'a> RecoverOperations<'a> {
    #[must_use]
    pub const fn new(deps: &'a ScanDeps) -> Self {
        Self { deps }
    }

    /// Recover every incomplete import operation of `root`. Idempotent once
    /// stable; never fabricates a result from a missing path.
    ///
    /// # Errors
    ///
    /// Propagates only infrastructure failures while reading the journal or
    /// the filesystem; per-item conflicts are reported in the result, not
    /// raised here.
    pub fn run(&self, root: LibraryRootId) -> Result<RecoveryReport, Error> {
        let items = self.deps.journal.incomplete_items(root)?;
        // Group per-operation, keeping only import operations (the recovery
        // matrix of 0.1.0).
        let mut ops: Vec<(OperationId, Vec<OperationItem>)> = Vec::new();
        for (operation, kind, item) in items {
            if kind != IMPORT_OPERATION {
                continue;
            }
            match ops.iter_mut().find(|(op, _)| *op == operation) {
                Some((_, list)) => list.push(item),
                None => ops.push((operation, vec![item])),
            }
        }
        let mut touched = Vec::new();
        for (operation, items) in ops {
            let held = self.recover_operation(root, operation, &items)?;
            touched.push(RecoveryOperation {
                operation,
                items: items.len(),
                held,
            });
        }
        Ok(RecoveryReport { touched })
    }

    /// Recover all items of one operation. When every item reached a terminal
    /// state the operation's target claims are released; if any item is held
    /// (conflict) the claims stay reserved so the path is not re-claimed.
    fn recover_operation(
        &self,
        root: LibraryRootId,
        operation: OperationId,
        items: &[OperationItem],
    ) -> Result<bool, Error> {
        let mut held = false;
        for item in items {
            match self.recover_item(root, operation, item)? {
                ItemResult::Terminal => {}
                ItemResult::Conflict => held = true,
            }
        }
        if !held {
            self.deps.journal.release_claims(operation)?;
        }
        Ok(held)
    }

    /// Bring one item to a durable terminal state from its persisted intent
    /// and the on-disk facts (existence + hash at target and staging).
    fn recover_item(
        &self,
        root: LibraryRootId,
        operation: OperationId,
        item: &OperationItem,
    ) -> Result<ItemResult, Error> {
        let target = self.path_hash_state(root, &item.target_path, &item.expected_hash)?;
        // `transpose` yields `Option<Option<bool>>` (None = no staging path OR
        // staging absent); flatten collapses it: None = absent, Some(bool) =
        // present-with-hash-match state.
        let staging: Option<bool> = item
            .staging_path
            .as_ref()
            .map(|path| self.path_hash_state(root, path, &item.expected_hash))
            .transpose()?
            .flatten();
        // A crash between exclusive reserve and rename leaves our own zero-byte
        // placeholder; `target` then reads as `Some(false)` (different hash).
        let empty_placeholder = matches!(target, Some(false))
            && self.target_is_our_empty_placeholder(root, &item.target_path)?;

        // 1) The final file is already durably correct → roll forward.
        if target == Some(true) {
            return self.roll_forward(root, operation, item);
        }
        // 2) A valid staged copy exists → retry the exclusive publish (the
        //    publish primitive clears our own empty reservation placeholder,
        //    but never touches non-empty foreign content).
        if staging == Some(true) && (target.is_none() || empty_placeholder) {
            self.publish_staged(root, operation, item)?;
            return self.roll_forward(root, operation, item);
        }
        // 3) Nothing usable:
        //    - a non-empty foreign file occupies the target, or an item that
        //      already claimed a publish whose file vanished → held conflict;
        //    - otherwise a pre-publish item rolls back cleanly.
        if !empty_placeholder && target.is_some() {
            return self.conflict(root, operation, item);
        }
        if item.state.publish_committed() {
            return self.conflict(root, operation, item);
        }
        self.roll_back(root, operation, item)
    }

    /// Roll the audio/lyrics item forward to terminal now that its final file
    /// is durably correct: write the song record for audio (reserved UUID, in
    /// one transaction with `DatabaseCommitted`), then `Completed`.
    fn roll_forward(
        &self,
        root: LibraryRootId,
        operation: OperationId,
        item: &OperationItem,
    ) -> Result<ItemResult, Error> {
        match item.kind {
            OperationResourceKind::Audio => {
                let song = item.song.ok_or_else(|| Error::InvariantViolation {
                    why: "recovered audio item without a reserved SongId".to_owned(),
                })?;
                // The record may already exist (a watcher that raced the import
                // completed it, or recovery ran before): skip, never duplicate.
                if self.deps.songs.by_id(song)?.is_none() {
                    self.commit_record(root, operation, item, song)?;
                } else {
                    // Record exists: advance the journal to DatabaseCommitted.
                    self.upsert(root, operation, item, OperationState::DatabaseCommitted)?;
                }
            }
            OperationResourceKind::Lyrics => {
                // A durable whole sidecar needs no database record.
            }
        }
        self.upsert(root, operation, item, OperationState::Completed)?;
        Ok(ItemResult::Terminal)
    }

    /// Retry the publish from the journal's persisted staging path, verifying
    /// the final size/hash before counting it applied.
    fn publish_staged(
        &self,
        root: LibraryRootId,
        operation: OperationId,
        item: &OperationItem,
    ) -> Result<(), Error> {
        let Some(staging_path) = item.staging_path.as_ref() else {
            return Err(Error::InvariantViolation {
                why: "recovered item without a staging path to publish".to_owned(),
            });
        };
        self.upsert(root, operation, item, OperationState::PublishPending)?;
        self.deps
            .fs
            .publish_from_staging_path(root, staging_path, &item.target_path)?;
        self.verify_target(root, item)?;
        self.upsert(root, operation, item, OperationState::PublishApplied)?;
        Ok(())
    }

    /// Verify the final file at the target matches the expected hash/size.
    fn verify_target(&self, root: LibraryRootId, item: &OperationItem) -> Result<(), Error> {
        if self.deps.hasher.hash(root, &item.target_path)? != item.expected_hash {
            return Err(Error::CorruptMedia {
                operation: IMPORT_OPERATION.to_owned(),
                reason: "recovered publish hash differs from the journal".to_owned(),
            });
        }
        Ok(())
    }

    /// Commit the song record under the reserved identity and record
    /// `DatabaseCommitted` in one transaction, exactly like a live import.
    fn commit_record(
        &self,
        root: LibraryRootId,
        operation: OperationId,
        item: &OperationItem,
        song_id: crate::domain::ids::SongId,
    ) -> Result<(), Error> {
        match parse_single_file(self.deps, root, &item.target_path) {
            FileOutcome::Parsed(parsed) => {
                let embedded = parsed
                    .embedded_lyrics
                    .clone()
                    .map(|candidate| rewrap(&candidate, LyricsSource::Embedded));
                let sidecar = parsed
                    .sidecar_lyrics
                    .clone()
                    .map(|candidate| rewrap(&candidate, LyricsSource::Sidecar));
                let cover = parsed.cover.clone();
                let committed = OperationItem {
                    kind: OperationResourceKind::Audio,
                    state: OperationState::DatabaseCommitted,
                    song: Some(song_id),
                    source: item.source.clone(),
                    staging_path: None,
                    target_path: item.target_path.clone(),
                    expected_hash: item.expected_hash.clone(),
                    claim_key: item.claim_key.clone(),
                };
                self.deps
                    .uow
                    .with_tx(Box::new(move |tx: &mut dyn TxAccess| {
                        let entity = song_from_parsed(song_id, root, &parsed.file);
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
                        tx.upsert_operation_item(operation, committed)?;
                        Ok(())
                    }))?;
                Ok(())
            }
            FileOutcome::FastSkip { .. } | FileOutcome::Diagnostic(_) => Err(Error::CorruptMedia {
                operation: IMPORT_OPERATION.to_owned(),
                reason: "recovered target could not be parsed into a record".to_owned(),
            }),
        }
    }

    /// Whether `target` is only Echo's own empty reservation placeholder (a
    /// crash between exclusive reserve and rename left a zero-byte file).
    fn target_is_our_empty_placeholder(
        &self,
        root: LibraryRootId,
        target: &RelativeMediaPath,
    ) -> Result<bool, Error> {
        if !self.deps.fs.path_exists(root, target)? {
            return Ok(false);
        }
        Ok(self.deps.fs.file_meta(root, target)?.size == 0)
    }

    /// Roll a pre-publish item back: clean the residual staged file (safe),
    /// mark `RolledBack`. The claim release is decided at the operation level.
    fn roll_back(
        &self,
        root: LibraryRootId,
        operation: OperationId,
        item: &OperationItem,
    ) -> Result<ItemResult, Error> {
        if let Some(staging_path) = item.staging_path.as_ref() {
            let _ = self.deps.fs.discard_staging_path(root, staging_path);
        }
        self.upsert(root, operation, item, OperationState::RolledBack)?;
        Ok(ItemResult::Terminal)
    }

    /// Hold the item on a foreign/missing conflict: `FailedRecoverable` still
    /// keeps the journal and the claim (never an automatic release or a
    /// missing-path success).
    fn conflict(
        &self,
        root: LibraryRootId,
        operation: OperationId,
        item: &OperationItem,
    ) -> Result<ItemResult, Error> {
        let _ = root;
        self.upsert(root, operation, item, OperationState::FailedRecoverable)?;
        Ok(ItemResult::Conflict)
    }

    fn upsert(
        &self,
        _root: LibraryRootId,
        operation: OperationId,
        item: &OperationItem,
        state: OperationState,
    ) -> Result<(), Error> {
        let next = OperationItem {
            state,
            ..item.clone()
        };
        self.deps.journal.upsert_item(operation, next)
    }

    /// `None` = absent; `Some(true)` = present with the expected hash;
    /// `Some(false)` = present with a different hash (foreign content or our
    /// own empty reservation placeholder).
    fn path_hash_state(
        &self,
        root: LibraryRootId,
        path: &RelativeMediaPath,
        expected: &str,
    ) -> Result<Option<bool>, Error> {
        if !self.deps.fs.path_exists(root, path)? {
            return Ok(None);
        }
        let hash = self.deps.hasher.hash(root, path)?;
        Ok(Some(hash == expected))
    }
}

#[cfg(test)]
mod tests {
    use std::io::Read;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    use super::*;
    use crate::application::import::{ImportOutcome, PlanImport};
    use crate::application::ports::{FileMeta, ImportSource, OperationJournalRepository};
    use crate::application::testing::{FakeImportSources, ScanFixture};
    use crate::domain::ids::SongId;
    use crate::domain::media::ParsedMetadata;

    // -----------------------------------------------------------------------
    // Crash-injection harness. Wrappers around the real fakes panic at a
    // scripted point, leaving the durable journal/fs state exactly as a real
    // crash would; the test then recovers twice and asserts the invariants.
    // -----------------------------------------------------------------------

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Phase {
        /// Crash before the side effect / state write.
        Before,
        /// Crash after the side effect ran but before its result was recorded.
        After,
    }

    #[derive(Clone, Copy, Debug, Eq, PartialEq)]
    enum Site {
        /// The streaming copy (`stage_stream`).
        Copy,
        /// The publish (`fs.publish` rename).
        Publish,
        /// The database commit (`with_tx`).
        Commit,
        /// A journal `upsert_item` of the named state (task-documented chain).
        State(&'static str),
    }

    #[derive(Clone, Debug, Eq, PartialEq)]
    struct Point {
        site: Site,
        phase: Phase,
    }

    /// Shared crash controller; `crash` consumes the armed point so exactly
    /// one panic is injected per arming.
    #[derive(Default)]
    struct Controller {
        armed: Mutex<Option<Point>>,
        fired: AtomicBool,
    }

    impl Controller {
        fn arm(&self, point: Point) {
            *self.armed.lock().unwrap() = Some(point);
            self.fired.store(false, Ordering::SeqCst);
        }
        fn fired(&self) -> bool {
            self.fired.load(Ordering::SeqCst)
        }
        fn crash(&self, site: &Site, phase: Phase) {
            let mut guard = self.armed.lock().unwrap();
            if guard
                .as_ref()
                .is_some_and(|p| &p.site == site && p.phase == phase)
            {
                guard.take();
                self.fired.store(true, Ordering::SeqCst);
                drop(guard);
                panic!("injected crash: {site:?} {phase:?}");
            }
        }
    }

    struct CrashFs {
        inner: Arc<dyn crate::application::ports::LibraryFileSystem>,
        ctrl: Arc<Controller>,
    }

    impl crate::application::ports::LibraryFileSystem for CrashFs {
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
            staged: &crate::application::ports::StagedResource,
            target: &RelativeMediaPath,
        ) -> Result<(), Error> {
            self.ctrl.crash(&Site::Publish, Phase::Before);
            let result = self.inner.publish(root, staged, target);
            self.ctrl.crash(&Site::Publish, Phase::After);
            result
        }
        fn stage(
            &self,
            root: LibraryRootId,
            staged: &crate::application::ports::StagedResource,
            content: &[u8],
        ) -> Result<(), Error> {
            self.inner.stage(root, staged, content)
        }
        fn stage_stream(
            &self,
            root: LibraryRootId,
            staged: &crate::application::ports::StagedResource,
            content: &mut dyn Read,
        ) -> Result<crate::application::ports::StagedCopy, Error> {
            self.ctrl.crash(&Site::Copy, Phase::Before);
            let result = self.inner.stage_stream(root, staged, content);
            self.ctrl.crash(&Site::Copy, Phase::After);
            result
        }
        fn read_staged(
            &self,
            root: LibraryRootId,
            staged: &crate::application::ports::StagedResource,
        ) -> Result<Vec<u8>, Error> {
            self.inner.read_staged(root, staged)
        }
        fn discard_staged(
            &self,
            root: LibraryRootId,
            staged: &crate::application::ports::StagedResource,
        ) -> Result<(), Error> {
            self.inner.discard_staged(root, staged)
        }
        fn path_exists(
            &self,
            root: LibraryRootId,
            path: &RelativeMediaPath,
        ) -> Result<bool, Error> {
            self.inner.path_exists(root, path)
        }
        fn publish_from_staging_path(
            &self,
            root: LibraryRootId,
            staging_path: &RelativeMediaPath,
            target: &RelativeMediaPath,
        ) -> Result<(), Error> {
            self.inner
                .publish_from_staging_path(root, staging_path, target)
        }
        fn discard_staging_path(
            &self,
            root: LibraryRootId,
            staging_path: &RelativeMediaPath,
        ) -> Result<(), Error> {
            self.inner.discard_staging_path(root, staging_path)
        }
        fn discard_published(
            &self,
            root: LibraryRootId,
            target: &RelativeMediaPath,
        ) -> Result<(), Error> {
            self.inner.discard_published(root, target)
        }
        fn write_capable(&self, root: LibraryRootId) -> Result<bool, Error> {
            self.inner.write_capable(root)
        }
    }

    struct CrashJournal {
        inner: Arc<dyn crate::application::ports::OperationJournalRepository>,
        ctrl: Arc<Controller>,
    }

    fn state_tag(state: OperationState) -> &'static str {
        match state {
            OperationState::Planned => "planned",
            OperationState::CopyPending => "copy_pending",
            OperationState::CopyApplied => "copy_applied",
            OperationState::ValidatePending => "validate_pending",
            OperationState::Validated => "validated",
            OperationState::PublishPending => "publish_pending",
            OperationState::PublishApplied => "publish_applied",
            OperationState::DatabaseCommitted => "database_committed",
            OperationState::Completed => "completed",
            OperationState::FailedRecoverable => "failed_recoverable",
            OperationState::RolledBack => "rolled_back",
            OperationState::StagePending => "stage_pending",
            OperationState::StageApplied => "stage_applied",
            OperationState::HiddenInDatabase => "hidden_in_database",
            OperationState::RestorePending => "restore_pending",
            OperationState::RestoreApplied => "restore_applied",
            OperationState::Restored => "restored",
            OperationState::TrashPending => "trash_pending",
            OperationState::TrashApplied => "trash_applied",
            OperationState::DatabaseFinalized => "database_finalized",
            OperationState::TrashOutcomeUnknown => "trash_outcome_unknown",
        }
    }

    impl crate::application::ports::OperationJournalRepository for CrashJournal {
        fn ensure_operation(
            &self,
            operation: OperationId,
            root: LibraryRootId,
            kind: &str,
            reserved_song: Option<SongId>,
        ) -> Result<(), Error> {
            self.inner
                .ensure_operation(operation, root, kind, reserved_song)
        }
        fn item_state(
            &self,
            operation: OperationId,
            item: &str,
        ) -> Result<Option<crate::application::ports::OperationItem>, Error> {
            self.inner.item_state(operation, item)
        }
        fn upsert_item(
            &self,
            operation: OperationId,
            item: crate::application::ports::OperationItem,
        ) -> Result<(), Error> {
            let site = Site::State(state_tag(item.state));
            self.ctrl.crash(&site, Phase::Before);
            let result = self.inner.upsert_item(operation, item);
            self.ctrl.crash(&site, Phase::After);
            result
        }
        fn items(
            &self,
            operation: OperationId,
        ) -> Result<Vec<crate::application::ports::OperationItem>, Error> {
            self.inner.items(operation)
        }
        fn incomplete_items(
            &self,
            root: LibraryRootId,
        ) -> Result<
            Vec<(
                OperationId,
                String,
                crate::application::ports::OperationItem,
            )>,
            Error,
        > {
            self.inner.incomplete_items(root)
        }
        fn release_claims(&self, operation: OperationId) -> Result<(), Error> {
            self.inner.release_claims(operation)
        }
    }

    struct CrashUow {
        inner: Arc<dyn crate::application::ports::UnitOfWork>,
        ctrl: Arc<Controller>,
    }

    impl crate::application::ports::UnitOfWork for CrashUow {
        fn with_tx(&self, f: crate::application::ports::TxWork) -> Result<(), Error> {
            self.ctrl.crash(&Site::Commit, Phase::Before);
            let result = self.inner.with_tx(f);
            self.ctrl.crash(&Site::Commit, Phase::After);
            result
        }
    }

    /// The crash-wired deps for one import run: the same shared fakes as the
    /// fixture, so after the injected crash the fixture's own (unwrapped) deps
    /// see the durable journal/fs state for recovery.
    fn crash_deps(
        fixture: &ScanFixture,
        ctrl: &Arc<Controller>,
    ) -> crate::application::scan::ScanDeps {
        let fs: Arc<dyn crate::application::ports::LibraryFileSystem> = Arc::new(CrashFs {
            inner: Arc::new(fixture.fs.clone()),
            ctrl: ctrl.clone(),
        });
        let journal: Arc<dyn crate::application::ports::OperationJournalRepository> =
            Arc::new(CrashJournal {
                inner: Arc::new(fixture.database.clone()),
                ctrl: ctrl.clone(),
            });
        let uow: Arc<dyn crate::application::ports::UnitOfWork> = Arc::new(CrashUow {
            inner: Arc::new(fixture.database.clone()),
            ctrl: ctrl.clone(),
        });
        crate::application::scan::ScanDeps {
            uow,
            fs,
            journal,
            ..ScanDeps::clone(&fixture.deps)
        }
    }

    fn source(key: &str) -> ImportSource {
        ImportSource::new(key).expect("valid source handle")
    }

    /// Register a single well-formed source whose tags name
    /// `歌手/歌手 - 晴天.flac` and whose published file parses cleanly.
    fn wire_single(fixture: &ScanFixture, sources: &FakeImportSources, key: &str, bytes: &[u8]) {
        sources.add(key, "晴天.flac", bytes);
        fixture.metadata.set_bytes(
            bytes,
            ParsedMetadata {
                artist: Some("歌手".to_owned()),
                title: Some("晴天".to_owned()),
                ..ParsedMetadata::default()
            },
        );
        fixture.set_audio("歌手/歌手 - 晴天.flac", "晴天", 269_000);
    }

    /// Run `PlanImport` for the single wired source; panics if the crash did
    /// not fire (the test should fail loudly rather than silently continue).
    fn run_import_until_crash(fixture: &ScanFixture, ctrl: &Arc<Controller>, point: Point) {
        let deps = crash_deps(fixture, ctrl);
        let sources = FakeImportSources::new();
        wire_single(fixture, &sources, "sunny", b"sunny-bytes");
        let described = format!("{point:?}");
        ctrl.arm(point);
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            PlanImport::new(&deps, &sources)
                .run(fixture.root, &[source("sunny")])
                .expect("batch-level success")
        }));
        assert!(
            outcome.is_err(),
            "the injection at {described} must crash the import"
        );
    }

    /// The `OperationId` of the single non-terminal import operation, found
    /// via the journal's recovery-input set (valid right after any crash).
    fn single_operation(fixture: &ScanFixture) -> OperationId {
        let items = OperationJournalRepository::incomplete_items(&fixture.database, fixture.root)
            .expect("incomplete items");
        assert_eq!(
            items.len(),
            1,
            "exactly one import operation after the crash: {items:?}"
        );
        items[0].0
    }

    /// The reserved `SongId` the import planned (from a journal item).
    fn reserved_of(fixture: &ScanFixture, operation: OperationId) -> SongId {
        fixture
            .database
            .items(operation)
            .expect("items")
            .into_iter()
            .find_map(|item| item.song)
            .expect("reserved song id")
    }

    /// Read the durable bytes of a root-relative file.
    fn read_file(fixture: &ScanFixture, rel: &str) -> Option<Vec<u8>> {
        let base = fixture.fs.root_path(fixture.root).expect("root");
        std::fs::read(base.join(rel)).ok()
    }

    /// Assert a successfully-recovered import: exactly one song = the RESERVED
    /// UUID, the target file present once with the exact source bytes, no
    /// ghost/orphan, all items terminal, claims released exactly once.
    fn assert_unique_terminal(fixture: &ScanFixture, operation: OperationId, reserved: SongId) {
        let songs = fixture.all_songs();
        assert_eq!(songs.len(), 1, "exactly one logical song, no duplicates");
        assert_eq!(
            songs[0].id(),
            reserved,
            "recovery reuses the journal-reserved UUID, never a second one"
        );
        assert_eq!(
            read_file(fixture, "歌手/歌手 - 晴天.flac"),
            Some(b"sunny-bytes".to_vec()),
            "the final file exists exactly once with the exact bytes"
        );
        assert!(
            read_file(fixture, songs[0].path().display()).is_some(),
            "every record has a real file (no ghost record)"
        );
        let items = fixture.database.items(operation).expect("items");
        assert!(
            items.iter().all(|item| item.state.is_terminal()),
            "all items terminal after recovery: {items:?}"
        );
        assert_eq!(
            fixture.database.released_claims(),
            vec![operation],
            "claims released exactly once at the unique terminal state"
        );
    }

    // -----------------------------------------------------------------------
    // Recovery-matrix tests: crash at every journal-state / fs / DB-commit
    // point, recover TWICE, and assert the unique terminal state, same
    // reserved UUID, no orphan final file, no duplicate, no ghost record.
    // -----------------------------------------------------------------------

    #[test]
    fn crash_at_every_state_and_fs_point_recovers_to_unique_terminal_twice() {
        let expect_success: Vec<(Site, Phase)> = vec![
            (Site::State("copy_pending"), Phase::Before),
            (Site::State("copy_applied"), Phase::Before),
            (Site::State("validate_pending"), Phase::Before),
            (Site::State("validated"), Phase::Before),
            (Site::State("publish_pending"), Phase::Before),
            (Site::State("publish_applied"), Phase::Before),
            // `database_committed` is written atomically with the song inside
            // the DB transaction, so a crash there is exactly a DB-commit
            // interruption (covered by `Commit Before/After` below).
            (Site::State("completed"), Phase::Before),
            (Site::Publish, Phase::Before),
            (Site::Publish, Phase::After),
            (Site::Commit, Phase::Before),
            (Site::Commit, Phase::After),
        ];
        for (site, phase) in expect_success {
            let fixture = ScanFixture::new();
            let ctrl = Arc::new(Controller::default());
            run_import_until_crash(&fixture, &ctrl, Point { site, phase });
            assert!(ctrl.fired(), "the crash fired at {site:?} {phase:?}");
            let operation = single_operation(&fixture);
            let reserved = reserved_of(&fixture, operation);

            // First recovery converges to the unique terminal state.
            RecoverOperations::new(&fixture.deps)
                .run(fixture.root)
                .unwrap();
            assert_unique_terminal(&fixture, operation, reserved);

            // Second recovery is a no-op: same UUID, still one song, no
            // duplicate file/record, claims not released twice.
            let before = fixture.all_songs();
            RecoverOperations::new(&fixture.deps)
                .run(fixture.root)
                .unwrap();
            let after = fixture.all_songs();
            assert_eq!(
                before, after,
                "second recovery is idempotent at {site:?} {phase:?}"
            );
            assert_eq!(after.len(), 1, "still exactly one record");
            assert_eq!(
                after[0].id(),
                reserved,
                "same reserved UUID across recoveries"
            );
            assert_eq!(fixture.database.released_claims(), vec![operation]);
        }
    }

    #[test]
    fn copy_crash_before_any_journal_leaves_nothing_to_recover() {
        // A crash during the streaming copy happens during pre-flight, before
        // the journal envelope/claim exists: nothing is recoverable, nothing
        // is orphaned, nothing enters the library.
        let fixture = ScanFixture::new();
        let ctrl = Arc::new(Controller::default());
        let deps = crash_deps(&fixture, &ctrl);
        let sources = FakeImportSources::new();
        wire_single(&fixture, &sources, "sunny", b"sunny-bytes");
        ctrl.arm(Point {
            site: Site::Copy,
            phase: Phase::Before,
        });
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            PlanImport::new(&deps, &sources)
                .run(fixture.root, &[source("sunny")])
                .expect("batch-level success")
        }));
        assert!(outcome.is_err(), "the copy crash aborted the import");
        assert!(fixture.all_songs().is_empty(), "no record was ever created");
        let report = RecoverOperations::new(&fixture.deps)
            .run(fixture.root)
            .unwrap();
        assert!(report.touched.is_empty(), "no incomplete journal items");
        assert!(
            read_file(&fixture, "歌手/歌手 - 晴天.flac").is_none(),
            "no orphan final file"
        );
    }

    #[test]
    fn truncated_source_is_rejected_and_leaves_nothing() {
        // The P2 "reading mid-copy truncation" gap: a source that serves fewer
        // bytes than it described must be rejected at validation and leave no
        // final file, no record, and a rolled-back journal.
        let fixture = ScanFixture::new();
        let sources = FakeImportSources::new();
        sources.add_truncated("broken", "晴天.flac", 200, b"only-partial");
        fixture.metadata.set_bytes(
            b"only-partial",
            ParsedMetadata {
                artist: Some("歌手".to_owned()),
                title: Some("晴天".to_owned()),
                ..ParsedMetadata::default()
            },
        );
        fixture.set_audio("歌手/歌手 - 晴天.flac", "晴天", 269_000);
        let report = PlanImport::new(&fixture.deps, &sources)
            .run(fixture.root, &[source("broken")])
            .expect("batch-level success");
        assert!(
            matches!(report.results[0], ImportOutcome::Failed { .. }),
            "truncated source must fail, not import: {:?}",
            report.results[0]
        );
        assert!(
            fixture.all_songs().is_empty(),
            "no record from a truncated copy"
        );
        assert!(
            read_file(&fixture, "歌手/歌手 - 晴天.flac").is_none(),
            "no final file from a truncated copy"
        );
    }

    #[test]
    fn recovery_never_creates_a_duplicate_when_a_watcher_preempts() {
        // Watcher preemption: the audio is published and a watcher reconciles
        // it under the RESERVED UUID before the import's DB commit (task 4.9).
        // Recovery must reuse that identity, never a second record or UUID.
        let fixture = ScanFixture::new();
        let ctrl = Arc::new(Controller::default());
        run_import_until_crash(
            &fixture,
            &ctrl,
            Point {
                site: Site::State("publish_applied"),
                phase: Phase::Before,
            },
        );
        let operation = single_operation(&fixture);
        let reserved = reserved_of(&fixture, operation);
        let target = "歌手/歌手 - 晴天.flac";
        assert!(
            read_file(&fixture, target).is_some(),
            "audio already published"
        );

        // The watcher applied a record under the reserved identity (the same
        // path `apply_reserved` uses in `WatchCoordinator`).
        let mut song = crate::domain::entities::Song::new(
            reserved,
            fixture.root,
            RelativeMediaPath::new(target).unwrap(),
            crate::domain::ids::Revision::INITIAL,
        );
        song.apply_scan_facts(
            fixture.deps.hasher.hash_of_bytes(b"sunny-bytes"),
            b"sunny-bytes".len() as u64,
            1,
            crate::domain::media::AudioFormat::Flac,
        );
        crate::application::ports::SongRepository::upsert(&fixture.database, &song).unwrap();
        assert_eq!(
            fixture.all_songs().len(),
            1,
            "watcher created the reserved record"
        );

        RecoverOperations::new(&fixture.deps)
            .run(fixture.root)
            .unwrap();
        let songs = fixture.all_songs();
        assert_eq!(
            songs.len(),
            1,
            "no duplicate record from watcher + recovery"
        );
        assert_eq!(songs[0].id(), reserved, "the reserved UUID is the only one");
        let items = fixture.database.items(operation).expect("items");
        assert!(items.iter().all(|item| item.state.is_terminal()));
        assert_eq!(fixture.database.released_claims(), vec![operation]);
    }

    #[test]
    fn foreign_target_content_is_a_held_conflict_never_overwritten() {
        // A target already occupied by non-empty foreign content is never
        // published over: recovery holds the item FailedRecoverable, keeps the
        // claim, leaves the foreign file intact, and creates no record.
        let fixture = ScanFixture::new();
        let ctrl = Arc::new(Controller::default());
        run_import_until_crash(
            &fixture,
            &ctrl,
            Point {
                site: Site::Publish,
                phase: Phase::Before,
            },
        );
        let operation = single_operation(&fixture);
        let reserved = reserved_of(&fixture, operation);
        // A foreign file (different content) now sits at the target, and the
        // staged copy is lost — the worst collision recovery can face.
        let base = fixture.fs.root_path(fixture.root).expect("root");
        let items = fixture.database.items(operation).expect("items");
        for item in &items {
            if let Some(staging) = item.staging_path.as_ref() {
                let _ = std::fs::remove_file(base.join(staging.normalized()));
            }
        }
        std::fs::create_dir_all(base.join("歌手")).expect("mkdir artist");
        std::fs::write(base.join("歌手/歌手 - 晴天.flac"), b"foreign-content")
            .expect("write foreign");

        RecoverOperations::new(&fixture.deps)
            .run(fixture.root)
            .unwrap();
        assert!(fixture.all_songs().is_empty(), "no record, no second UUID");
        assert_eq!(
            read_file(&fixture, "歌手/歌手 - 晴天.flac"),
            Some(b"foreign-content".to_vec()),
            "the foreign file is never overwritten"
        );
        let items = fixture.database.items(operation).expect("items");
        assert!(
            items
                .iter()
                .all(|item| item.state == OperationState::FailedRecoverable),
            "conflict is held FailedRecoverable: {items:?}"
        );
        assert!(
            fixture.database.released_claims().is_empty(),
            "an unresolved conflict keeps its claim (never released)"
        );
        // Recovery is idempotent on the conflict too.
        RecoverOperations::new(&fixture.deps)
            .run(fixture.root)
            .unwrap();
        assert_eq!(
            read_file(&fixture, "歌手/歌手 - 晴天.flac"),
            Some(b"foreign-content".to_vec()),
            "second recovery still never overwrites"
        );
        assert!(fixture.all_songs().is_empty());
        let _ = reserved;
    }

    #[test]
    fn nothing_recoverable_rolls_back_and_releases_cleanly() {
        // A crash while only a claim exists (no staged copy, no target) rolls
        // back cleanly: nothing created, the item RolledBack, claim released.
        let fixture = ScanFixture::new();
        let ctrl = Arc::new(Controller::default());
        run_import_until_crash(
            &fixture,
            &ctrl,
            Point {
                site: Site::State("copy_pending"),
                phase: Phase::Before,
            },
        );
        let operation = single_operation(&fixture);
        let reserved = reserved_of(&fixture, operation);
        // Remove the staged copy and any target so nothing is recoverable.
        let base = fixture.fs.root_path(fixture.root).expect("root");
        for item in fixture.database.items(operation).expect("items") {
            if let Some(staging) = item.staging_path.as_ref() {
                let _ = std::fs::remove_file(base.join(staging.normalized()));
            }
        }

        RecoverOperations::new(&fixture.deps)
            .run(fixture.root)
            .unwrap();
        assert!(fixture.all_songs().is_empty(), "nothing created");
        let items = fixture.database.items(operation).expect("items");
        assert!(
            items
                .iter()
                .all(|item| item.state == OperationState::RolledBack),
            "nothing usable rolls back and releases: {items:?}"
        );
        assert_eq!(
            fixture.database.released_claims(),
            vec![operation],
            "the claim is released"
        );
        let _ = reserved;
        // Second recovery no-op (idempotent, no double release).
        RecoverOperations::new(&fixture.deps)
            .run(fixture.root)
            .unwrap();
        assert_eq!(fixture.database.released_claims(), vec![operation]);
    }
}
