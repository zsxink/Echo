//! Crash-recovery unit tests (moved verbatim from the former `recover.rs`,
//! dedented one level; the `mod tests` wrapper is the file's root item).

use std::io::Read;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use super::*;
use crate::application::delete::{DeleteSongs, RestoreDeletedOperation};
use crate::application::import::{ImportOutcome, PlanImport};
use crate::application::ports::{
    FileMeta, ImportSource, OperationJournalRepository, PlaylistRepository, SongRepository,
};
use crate::application::testing::{
    small_fakes::MemoryControlPlane, FakeImportSources, ScanFixture,
};
use crate::domain::entities::SongAvailability;
use crate::domain::ids::SongId;
use crate::domain::library::PortableRecord;
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
    /// The delete/restore trash rename (`stage_to_trash`/`restore_from_trash`).
    TrashMove,
    /// The portable song-record materialization (`control.write_record`).
    RecordWrite,
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

impl crate::application::ports::filesystem::LegacyLibraryFileSystem for CrashFs {
    fn enumerate(&self, root: LibraryRootId) -> Result<Vec<RelativeMediaPath>, Error> {
        self.inner.enumerate(root)
    }
    fn file_meta(&self, root: LibraryRootId, path: &RelativeMediaPath) -> Result<FileMeta, Error> {
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
    fn path_exists(&self, root: LibraryRootId, path: &RelativeMediaPath) -> Result<bool, Error> {
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
    fn trash_path(
        &self,
        root: LibraryRootId,
        operation: OperationId,
        resource_key: &str,
    ) -> Result<RelativeMediaPath, Error> {
        self.inner.trash_path(root, operation, resource_key)
    }
    fn stage_to_trash(
        &self,
        root: LibraryRootId,
        operation: OperationId,
        source: &RelativeMediaPath,
        resource_key: &str,
    ) -> Result<RelativeMediaPath, Error> {
        let key = resource_key.to_owned();
        self.ctrl.crash(&Site::TrashMove, Phase::Before);
        let result = self.inner.stage_to_trash(root, operation, source, &key);
        self.ctrl.crash(&Site::TrashMove, Phase::After);
        result
    }
    fn restore_from_trash(
        &self,
        root: LibraryRootId,
        trash: &RelativeMediaPath,
        target: &RelativeMediaPath,
    ) -> Result<(), Error> {
        self.ctrl.crash(&Site::TrashMove, Phase::Before);
        let result = self.inner.restore_from_trash(root, trash, target);
        self.ctrl.crash(&Site::TrashMove, Phase::After);
        result
    }
    fn write_capable(&self, root: LibraryRootId) -> Result<bool, Error> {
        self.inner.write_capable(root)
    }
    fn establish_write_capability(&self, root: LibraryRootId) -> Result<(), Error> {
        self.inner.establish_write_capability(root)
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
    fn set_undo_deadline(&self, operation: OperationId, deadline_ms: i64) -> Result<(), Error> {
        self.inner.set_undo_deadline(operation, deadline_ms)
    }
    fn undo_deadline(&self, operation: OperationId) -> Result<Option<i64>, Error> {
        self.inner.undo_deadline(operation)
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

/// A `MemoryControlPlane` wrapper that crashes at the portable-record write
/// (`control.write_record`), the new materialization side effect. Like the
/// fs/journal wrappers, it shares the underlying fake with the fixture so
/// recovery (via the fixture's own deps) sees the durable record state.
struct CrashControlPlane {
    inner: MemoryControlPlane,
    ctrl: Arc<Controller>,
}

impl crate::application::ports::ControlPlanePort for CrashControlPlane {
    fn durability(&self) -> Option<&dyn crate::application::ports::LibraryDurabilityPort> {
        Some(self)
    }
    fn write_manifest(
        &self,
        root: LibraryRootId,
        manifest: &crate::domain::library::LibraryManifest,
    ) -> Result<(), Error> {
        self.inner.write_manifest(root, manifest)
    }
    fn read_manifest(
        &self,
        root: LibraryRootId,
    ) -> Result<Option<crate::domain::library::LibraryManifest>, Error> {
        self.inner.read_manifest(root)
    }
    fn write_record(&self, root: LibraryRootId, record: &PortableRecord) -> Result<(), Error> {
        self.ctrl.crash(&Site::RecordWrite, Phase::Before);
        let result = self.inner.write_record(root, record);
        self.ctrl.crash(&Site::RecordWrite, Phase::After);
        result
    }
    fn read_record(
        &self,
        root: LibraryRootId,
        kind: crate::domain::library::RecordKind,
        object_uuid: &str,
    ) -> Result<Option<PortableRecord>, Error> {
        self.inner.read_record(root, kind, object_uuid)
    }
    fn delete_record(
        &self,
        root: LibraryRootId,
        kind: crate::domain::library::RecordKind,
        object_uuid: &str,
    ) -> Result<(), Error> {
        self.inner.delete_record(root, kind, object_uuid)
    }
    fn list_records(
        &self,
        root: LibraryRootId,
        kind: crate::domain::library::RecordKind,
    ) -> Result<Vec<String>, Error> {
        self.inner.list_records(root, kind)
    }
    fn control_plane_usable(&self, root: LibraryRootId) -> Result<bool, Error> {
        self.inner.control_plane_usable(root)
    }
    fn manifest_state(
        &self,
        root: LibraryRootId,
    ) -> Result<crate::application::ports::ManifestState, Error> {
        self.inner.manifest_state(root)
    }
}

impl crate::application::ports::LibraryDurabilityPort for CrashControlPlane {
    fn write_asset(&self, root: LibraryRootId, bytes: &[u8]) -> Result<String, Error> {
        self.inner.write_asset(root, bytes)
    }
    fn read_asset(&self, root: LibraryRootId, hash: &str) -> Result<Option<Vec<u8>>, Error> {
        self.inner.read_asset(root, hash)
    }
    fn commit(
        &self,
        root: LibraryRootId,
        batch: &crate::domain::library::durability::LibraryBatch,
    ) -> Result<crate::domain::library::durability::CommitOutcome, Error> {
        // The old per-record materialization site is now the authoritative
        // commit boundary. Claim reservations contain no records and must not
        // masquerade as the song being committed.
        if !batch.records.is_empty() {
            self.ctrl.crash(&Site::RecordWrite, Phase::Before);
        }
        let result = self.inner.commit(root, batch);
        if !batch.records.is_empty() {
            self.ctrl.crash(&Site::RecordWrite, Phase::After);
        }
        result
    }
    fn recover(
        &self,
        root: LibraryRootId,
    ) -> Result<crate::domain::library::durability::LibraryReplay, Error> {
        self.inner.recover(root)
    }
    fn read_operation(
        &self,
        root: LibraryRootId,
        id: uuid::Uuid,
    ) -> Result<Option<crate::domain::library::durability::LibraryCommit>, Error> {
        self.inner.read_operation(root, id)
    }
    fn operation_status(
        &self,
        root: LibraryRootId,
        id: uuid::Uuid,
    ) -> Result<Option<crate::domain::library::durability::CommitOutcome>, Error> {
        self.inner.operation_status(root, id)
    }
}

/// The crash-wired deps for one import run: the same shared fakes as the
/// fixture, so after the injected crash the fixture's own (unwrapped) deps
/// see the durable journal/fs state for recovery.
fn crash_deps(fixture: &ScanFixture, ctrl: &Arc<Controller>) -> crate::application::scan::ScanDeps {
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
    let control: Arc<dyn crate::application::ports::ControlPlanePort> =
        Arc::new(CrashControlPlane {
            inner: fixture.control.clone(),
            ctrl: ctrl.clone(),
        });
    crate::application::scan::ScanDeps {
        uow,
        fs,
        journal,
        control,
        ..ScanDeps::clone(&fixture.deps)
    }
}

fn source(key: &str) -> ImportSource {
    ImportSource::new(key).expect("valid source handle")
}

/// Register a single well-formed source whose tags name
/// `media/歌手/歌手 - 晴天.flac` and whose published file parses cleanly.
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
    fixture.set_audio("media/歌手/歌手 - 晴天.flac", "晴天", 269_000);
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

/// The single distinct delete operation across all incomplete items (a
/// delete may span several resources that share one operation id).
fn delete_operation(fixture: &ScanFixture) -> OperationId {
    let items = OperationJournalRepository::incomplete_items(&fixture.database, fixture.root)
        .expect("incomplete items");
    assert!(
        !items.is_empty(),
        "a delete operation exists after the crash"
    );
    let operation = items[0].0;
    assert!(
        items.iter().all(|(op, _, _)| *op == operation),
        "exactly one delete operation after the crash: {items:?}"
    );
    operation
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
        read_file(fixture, "media/歌手/歌手 - 晴天.flac"),
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

#[path = "tests_delete_restore.rs"]
mod delete_restore;
#[path = "tests_imports.rs"]
mod imports;

impl crate::application::ports::ControlPlaneInventoryPort for CrashControlPlane {
    fn records_present(&self, root: LibraryRootId) -> Result<bool, Error> {
        self.inner.records_present(root)
    }
}
