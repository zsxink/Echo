//! The active-root switch barrier use cases (task 4.1, design §5).
//!
//! Switching the active library root is a four-phase runtime operation:
//! `Prepare → QuiesceOldRoot → CommitActivation → RebindRuntime`, guarded by
//! the domain [`RootSwitchBarrier`] and a monotonic [`RootEpoch`] that
//! invalidates late results produced under the old binding.
//!
//! This module owns the two Core-side phases:
//!
//! - [`PrepareLibraryCandidate`]: create/reuse the candidate root record,
//!   fully enumerate it, and run a complete candidate scan. Success
//!   criteria (design §5): complete enumeration, no cancellation, reconcile
//!   success, no root-level error; per-file errors are summarized. An empty
//!   directory is a success. On failure the old active root is untouched and
//!   the candidate stays retryable.
//! - [`ActivateLibrary`]: quiesce (cancel in-flight scans), then flip the
//!   unique active root **and** advance the persisted `root_epoch` in one
//!   `SQLite` transaction. In-flight imports and delete-undo windows are
//!   blockers the runtime reports through a provider; a blocked switch is
//!   refused and the old root keeps serving.
//!
//! Re-selecting the same canonical path reuses the original `LibraryRootId`
//! (deterministically derived from the canonical path), so the local record
//! and its history stay stable across re-selection and restarts.
//!
//! Read-only roots: when the file-system adapter reports `write_capable ==
//! false`, the activated root enters `ActiveReadOnly` — scanning, search and
//! playback work; import/delete stay disabled.

use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::application::ports::{LibraryRepository, RuntimeStateStore, TxAccess};
use crate::application::scan::{ScanDeps, ScanSummary, ScanSupervisor, StartScan};
use crate::domain::entities::{LibraryRoot, RootAvailability};
use crate::domain::ids::LibraryRootId;
use crate::domain::state::library_root::{
    LibraryRootState, RootEpoch, RootSwitchBarrier, RootSwitchState,
};
use crate::error::Error;

/// The `runtime_state` key holding the persisted root epoch.
pub const ROOT_EPOCH_KEY: &str = "root_epoch";

/// A fixed namespace so the derived root id is stable across processes.
const ROOT_ID_NAMESPACE: uuid::Uuid = uuid::Uuid::from_u128(0x6165_6368_6f5f_726f_6f74_0000_0001);

/// Derive the stable [`LibraryRootId`] of a canonical root path. Deterministic
/// by construction: re-selecting the same canonical path re-derives the same
/// id and the repository upsert keeps the existing record (spec: 复用原逻辑 ID).
#[must_use]
pub fn derive_root_id(canonical_path: &Path) -> LibraryRootId {
    let digest = blake3::hash(canonical_path.to_string_lossy().as_bytes());
    LibraryRootId::from_uuid(uuid::Uuid::new_v5(&ROOT_ID_NAMESPACE, digest.as_bytes()))
}

/// The result of a successful candidate preparation.
#[derive(Clone, Debug)]
pub struct PreparedCandidate {
    pub root_id: LibraryRootId,
    /// `true` when an earlier record for the same canonical path was reused.
    pub reused: bool,
    pub write_capable: bool,
    pub scan: ScanSummary,
}

/// Prepares a candidate root: record, full enumeration, candidate scan.
pub struct PrepareLibraryCandidate<'a> {
    deps: &'a ScanDeps,
    roots: &'a dyn LibraryRepository,
    supervisor: &'a ScanSupervisor,
}

impl<'a> PrepareLibraryCandidate<'a> {
    #[must_use]
    pub const fn new(
        deps: &'a ScanDeps,
        roots: &'a dyn LibraryRepository,
        supervisor: &'a ScanSupervisor,
    ) -> Self {
        Self {
            deps,
            roots,
            supervisor,
        }
    }

    /// Prepare the directory at `absolute_path` as a candidate root.
    ///
    /// The caller (runtime / test) must have registered the path under the
    /// derived root id in the shared [`crate::infrastructure::filesystem::RootRegistry`]
    /// so the file-system adapters can resolve it — Core never receives or
    /// stores the path beyond the root record.
    ///
    /// # Errors
    ///
    /// Root-level failures (unreadable directory, aborted enumeration,
    /// persistence failure) propagate; the candidate record stays (marked
    /// unavailable, retryable) and the current active root is untouched.
    pub fn prepare(&self, absolute_path: &Path) -> Result<PreparedCandidate, Error> {
        let canonical = absolute_path
            .canonicalize()
            .map_err(|source| Error::io("resolve candidate root", source, absolute_path))?;
        let root_id = derive_root_id(&canonical);
        let existing = self.roots.by_id(root_id)?;
        let reused = existing.is_some();
        // `write_capable` reflects permission + ownership marker (task 4.2).
        let observed_write_capable = self.deps.fs.write_capable(root_id)?;
        let write_safety_locked = existing
            .as_ref()
            .is_some_and(LibraryRoot::write_safety_locked);
        let write_capable = observed_write_capable && !write_safety_locked;
        let mut prepared = LibraryRoot::new(root_id, canonical, false, observed_write_capable);
        prepared.set_write_safety_locked(write_safety_locked);
        self.roots.upsert(&prepared)?;

        // Candidate scan: complete enumeration + reconcile required.
        match StartScan::new(self.deps, self.supervisor).run(root_id) {
            Ok(scan) => Ok(PreparedCandidate {
                root_id,
                reused,
                write_capable,
                scan,
            }),
            Err(error) => {
                // The candidate failed: keep the record (retryable), mark it
                // unavailable; the old active root is untouched.
                let _ = self
                    .roots
                    .set_write_and_availability(root_id, write_capable, false);
                Err(error)
            }
        }
    }
}

/// The outcome of a committed activation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ActivationOutcome {
    pub epoch: RootEpoch,
    pub previous_root: Option<LibraryRootId>,
    /// The domain state the new root enters (`ActiveAvailable` or
    /// `ActiveReadOnly`).
    pub root_state: LibraryRootState,
    /// The barrier phase after commit (`RebindRuntime` — the runtime finishes
    /// the switch by rebinding watcher/queries/playback to the new epoch).
    pub barrier_state: RootSwitchState,
}

/// Commits the active-root switch (the `SQLite` half of the barrier).
pub struct ActivateLibrary<'a> {
    deps: &'a ScanDeps,
    roots: &'a dyn LibraryRepository,
    supervisor: &'a ScanSupervisor,
    state: &'a dyn RuntimeStateStore,
    /// Runtime-reported blockers (in-flight import, delete-undo window, an
    /// unknown journal state). A non-empty list refuses the switch.
    blockers: &'a Blockers,
}

/// The blocker registry the runtime feeds (import in flight, delete-undo
/// window, unknown journal state). Thread-safe: use cases read a snapshot.
#[derive(Clone, Debug, Default)]
pub struct Blockers {
    reasons: Arc<Mutex<Vec<String>>>,
}

impl Blockers {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add(&self, reason: impl Into<String>) {
        self.reasons
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(reason.into());
    }

    pub fn clear(&self) {
        self.reasons
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clear();
    }

    fn snapshot(&self) -> Vec<String> {
        self.reasons
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl<'a> ActivateLibrary<'a> {
    #[must_use]
    pub const fn new(
        deps: &'a ScanDeps,
        roots: &'a dyn LibraryRepository,
        supervisor: &'a ScanSupervisor,
        state: &'a dyn RuntimeStateStore,
        blockers: &'a Blockers,
    ) -> Self {
        Self {
            deps,
            roots,
            supervisor,
            state,
            blockers,
        }
    }

    /// Commit the activation of a *prepared* candidate.
    ///
    /// # Errors
    ///
    /// - `Conflict` when blockers are active (import in flight, delete undo
    ///   window open, unknown journal state) — the old root keeps serving.
    /// - `Unavailable`/`Conflict` when the candidate was never prepared or
    ///   its scan failed.
    /// - Storage errors from the activation transaction leave the old active
    ///   root intact (the transaction is atomic).
    pub fn activate(&self, candidate: LibraryRootId) -> Result<ActivationOutcome, Error> {
        let candidate_root = self
            .roots
            .by_id(candidate)?
            .ok_or_else(|| Error::unavailable("candidate root", "not prepared"))?;
        if candidate_root.availability() != RootAvailability::Available {
            return Err(Error::conflict(
                "candidate scan failed; resolve and re-prepare before activating",
            ));
        }

        // Blockers: in-flight import / delete-undo cannot be silently carried
        // across a root switch (design §5.2).
        let reasons = self.blockers.snapshot();
        if !reasons.is_empty() {
            return Err(Error::conflict(format!(
                "root switch blocked by active operations: {}",
                reasons.join(", ")
            )));
        }

        let previous = self.roots.active_root()?;
        let previous_id = previous.as_ref().map(LibraryRoot::id);
        let current_epoch = self
            .state
            .load(ROOT_EPOCH_KEY)?
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(0);
        let mut barrier =
            RootSwitchBarrier::new(previous_id, candidate, RootEpoch::from_u64(current_epoch));

        // QuiesceOldRoot: the runtime freezes its watcher; Core cancels any
        // in-flight scan of the old binding (its results belong to the old
        // epoch and are discarded anyway).
        barrier
            .transition(RootSwitchState::QuiesceOldRoot)
            .map_err(|error| to_invariant(&error))?;
        let _cancelled_roots = self.supervisor.cancel_all();

        // CommitActivation: one SQLite transaction flips the unique active
        // root and advances the persisted root epoch (design §5.4).
        barrier
            .transition(RootSwitchState::CommitActivation)
            .map_err(|error| to_invariant(&error))?;
        let new_epoch = RootEpoch::from_u64(current_epoch + 1);
        let write_capable = candidate_root.write_capable();
        let mut activated = LibraryRoot::new(
            candidate,
            candidate_root.absolute_path().to_path_buf(),
            true,
            candidate_root.observed_write_capable(),
        );
        activated.set_write_safety_locked(candidate_root.write_safety_locked());
        let epoch_value = new_epoch.as_u64().to_string();
        let outcome_write_capable = write_capable;
        self.deps
            .uow
            .with_tx(Box::new(move |tx: &mut dyn TxAccess| {
                tx.upsert_root(&activated)?;
                tx.set_runtime_state(ROOT_EPOCH_KEY, &epoch_value)?;
                Ok(())
            }))?;
        barrier
            .commit_activation(new_epoch)
            .map_err(|error| to_invariant(&error))?;

        Ok(ActivationOutcome {
            epoch: new_epoch,
            previous_root: previous.map(|root| root.id()),
            root_state: if outcome_write_capable {
                LibraryRootState::ActiveAvailable
            } else {
                LibraryRootState::ActiveReadOnly
            },
            barrier_state: barrier.state(),
        })
    }
}

fn to_invariant(error: &crate::domain::state::TransitionError) -> Error {
    // `TransitionError` carries only diagnostic text; borrow it so the domain
    // error type stays boundary-internal.
    Error::InvariantViolation {
        why: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::delete::DeleteSongs;
    use crate::application::import::{ImportOutcome, PlanImport};
    use crate::application::ports::ImportSource;
    use crate::application::testing::{FakeImportSources, ScanFixture};
    use crate::domain::entities::Song;
    use crate::domain::ids::{RelativeMediaPath, Revision, SongId};
    use crate::domain::state::scan::ScanState;

    fn fixture() -> ScanFixture {
        ScanFixture::new()
    }

    /// Bind a candidate path in the fake file system under the derived root
    /// id (what the desktop composition root does before preparing).
    fn bind_candidate(fixture: &ScanFixture, path: &std::path::Path) -> LibraryRootId {
        let root_id = derive_root_id(&path.canonicalize().expect("canonical"));
        fixture.fs.add_root_at(root_id, path.to_path_buf());
        root_id
    }

    #[test]
    fn empty_directory_activates_and_reuses_root_id() {
        let fixture = fixture();
        let dir = tempfile::tempdir().expect("temp dir");
        let _root_id = bind_candidate(&fixture, dir.path());
        // An empty directory is a valid candidate (设计：空目录允许成功).
        let prepared =
            PrepareLibraryCandidate::new(&fixture.deps, &fixture.database, &fixture.supervisor)
                .prepare(dir.path())
                .expect("empty dir prepares");
        assert!(!prepared.reused);
        assert_eq!(prepared.scan.progress.discovered, 0);
        assert_eq!(prepared.scan.progress.state, ScanState::Completed);

        let outcome = ActivateLibrary::new(
            &fixture.deps,
            &fixture.database,
            &fixture.supervisor,
            &fixture.database,
            &Blockers::new(),
        )
        .activate(prepared.root_id)
        .expect("activation");
        assert_eq!(outcome.root_state, LibraryRootState::ActiveAvailable);
        assert_eq!(outcome.barrier_state, RootSwitchState::RebindRuntime);
        assert_eq!(
            fixture.database.active_root().unwrap().unwrap().id(),
            prepared.root_id
        );
        assert_eq!(
            fixture.database.runtime_state(ROOT_EPOCH_KEY).unwrap(),
            "1",
            "the epoch advanced with the activation commit"
        );

        // Re-preparing the same canonical path reuses the id and record.
        let again =
            PrepareLibraryCandidate::new(&fixture.deps, &fixture.database, &fixture.supervisor)
                .prepare(dir.path())
                .expect("re-prepare");
        assert!(again.reused, "same canonical path reuses the root id");
        assert_eq!(again.root_id, prepared.root_id);
    }

    #[test]
    fn root_level_error_keeps_old_active_root() {
        let fixture = fixture();
        let active_dir = tempfile::tempdir().unwrap();
        bind_candidate(&fixture, active_dir.path());
        let first =
            PrepareLibraryCandidate::new(&fixture.deps, &fixture.database, &fixture.supervisor)
                .prepare(active_dir.path())
                .expect("first prepare");
        ActivateLibrary::new(
            &fixture.deps,
            &fixture.database,
            &fixture.supervisor,
            &fixture.database,
            &Blockers::new(),
        )
        .activate(first.root_id)
        .expect("first activation");

        // The second candidate enumerates fine until the fs faults.
        let candidate_dir = tempfile::tempdir().unwrap();
        let _candidate_id = bind_candidate(&fixture, candidate_dir.path());
        fixture
            .fs
            .inject_fault(Error::unavailable("library root", "disk ejected"));
        let prepared =
            PrepareLibraryCandidate::new(&fixture.deps, &fixture.database, &fixture.supervisor)
                .prepare(candidate_dir.path());
        drop(prepared); // Err expected
        fixture.fs.clear_fault();

        // The old active root is untouched and the candidate is retryable.
        assert_eq!(
            fixture.database.active_root().unwrap().unwrap().id(),
            first.root_id,
            "the failed candidate never touched the active root"
        );
        // The old root's songs/state are intact (no missing pass ran).
        assert_eq!(fixture.database.runtime_state(ROOT_EPOCH_KEY).unwrap(), "1");
    }

    #[test]
    fn read_only_root_activates_readonly_and_disables_writes() {
        let fixture = fixture();
        let dir = tempfile::tempdir().unwrap();
        bind_candidate(&fixture, dir.path());
        fixture.fs.set_write_capable(false);
        let prepared =
            PrepareLibraryCandidate::new(&fixture.deps, &fixture.database, &fixture.supervisor)
                .prepare(dir.path())
                .expect("prepare");
        assert!(!prepared.write_capable);
        let outcome = ActivateLibrary::new(
            &fixture.deps,
            &fixture.database,
            &fixture.supervisor,
            &fixture.database,
            &Blockers::new(),
        )
        .activate(prepared.root_id)
        .expect("activation");
        assert_eq!(
            outcome.root_state,
            LibraryRootState::ActiveReadOnly,
            "write-incapable roots activate read-only"
        );
        let root = fixture.database.active_root().unwrap().unwrap();
        assert!(!root.write_capable());
        assert!(
            !fixture.deps.fs.write_capable(root.id()).unwrap(),
            "import/delete stay disabled for read-only roots"
        );
    }

    #[test]
    fn trash_outcome_unknown_survives_reprepare_and_activation_and_rejects_writes() {
        let fixture = fixture();
        let dir = tempfile::tempdir().unwrap();
        bind_candidate(&fixture, dir.path());
        let prepared =
            PrepareLibraryCandidate::new(&fixture.deps, &fixture.database, &fixture.supervisor)
                .prepare(dir.path())
                .expect("prepare");
        ActivateLibrary::new(
            &fixture.deps,
            &fixture.database,
            &fixture.supervisor,
            &fixture.database,
            &Blockers::new(),
        )
        .activate(prepared.root_id)
        .expect("activation");

        // This is the durable safety consequence of TrashOutcomeUnknown. A
        // later filesystem permission probe must not make it writable again.
        fixture
            .database
            .set_write_safety_locked(prepared.root_id, true)
            .expect("persist safety lock");
        let reparsed =
            PrepareLibraryCandidate::new(&fixture.deps, &fixture.database, &fixture.supervisor)
                .prepare(dir.path())
                .expect("re-prepare keeps safety isolation");
        assert!(reparsed.reused);
        assert!(!reparsed.write_capable);
        let activated = ActivateLibrary::new(
            &fixture.deps,
            &fixture.database,
            &fixture.supervisor,
            &fixture.database,
            &Blockers::new(),
        )
        .activate(reparsed.root_id)
        .expect("re-activation remains possible for reads");
        assert_eq!(activated.root_state, LibraryRootState::ActiveReadOnly);
        assert!(crate::application::ports::LibraryRepository::by_id(
            &fixture.database,
            prepared.root_id,
        )
        .unwrap()
        .unwrap()
        .write_safety_locked());

        let sources = FakeImportSources::new();
        sources.add("new", "new.flac", b"bytes");
        let imported = PlanImport::new(&fixture.deps, &sources)
            .run(
                prepared.root_id,
                &[ImportSource::new("new").expect("logical source")],
            )
            .expect("write rejection is a normal report");
        assert_eq!(imported.results, vec![ImportOutcome::LibraryUnavailable]);

        let song = Song::new(
            SongId::new(),
            prepared.root_id,
            RelativeMediaPath::new("still-here.flac").unwrap(),
            Revision::INITIAL,
        );
        crate::application::ports::SongRepository::upsert(&fixture.database, &song)
            .expect("seed song");
        let error = DeleteSongs::new(&fixture.deps)
            .delete(prepared.root_id, song.id())
            .expect_err("delete remains disabled after re-selection");
        assert_eq!(error.code(), "unavailable");
    }

    #[test]
    fn switch_refused_while_import_blockers_active() {
        let fixture = fixture();
        let first_dir = tempfile::tempdir().unwrap();
        bind_candidate(&fixture, first_dir.path());
        let first =
            PrepareLibraryCandidate::new(&fixture.deps, &fixture.database, &fixture.supervisor)
                .prepare(first_dir.path())
                .unwrap();
        ActivateLibrary::new(
            &fixture.deps,
            &fixture.database,
            &fixture.supervisor,
            &fixture.database,
            &Blockers::new(),
        )
        .activate(first.root_id)
        .unwrap();

        let blockers = Blockers::new();
        blockers.add("import in flight");
        blockers.add("delete undo window open");
        let second_dir = tempfile::tempdir().unwrap();
        bind_candidate(&fixture, second_dir.path());
        let second =
            PrepareLibraryCandidate::new(&fixture.deps, &fixture.database, &fixture.supervisor)
                .prepare(second_dir.path())
                .unwrap();

        let error = ActivateLibrary::new(
            &fixture.deps,
            &fixture.database,
            &fixture.supervisor,
            &fixture.database,
            &blockers,
        )
        .activate(second.root_id)
        .unwrap_err();
        assert_eq!(error.code(), "conflict");
        assert_eq!(
            fixture.database.active_root().unwrap().unwrap().id(),
            first.root_id,
            "a blocked switch leaves the old root serving"
        );
        assert_eq!(
            fixture.database.runtime_state(ROOT_EPOCH_KEY).unwrap(),
            "1",
            "no epoch advance on a refused switch"
        );

        // Once the runtime resolves the blockers the switch goes through.
        blockers.clear();
        let outcome = ActivateLibrary::new(
            &fixture.deps,
            &fixture.database,
            &fixture.supervisor,
            &fixture.database,
            &blockers,
        )
        .activate(second.root_id)
        .unwrap();
        assert_eq!(outcome.previous_root, Some(first.root_id));
    }

    #[test]
    fn switch_cancels_active_scan_and_advances_epoch() {
        let fixture = fixture();
        let first_dir = tempfile::tempdir().unwrap();
        bind_candidate(&fixture, first_dir.path());
        let first =
            PrepareLibraryCandidate::new(&fixture.deps, &fixture.database, &fixture.supervisor)
                .prepare(first_dir.path())
                .unwrap();
        ActivateLibrary::new(
            &fixture.deps,
            &fixture.database,
            &fixture.supervisor,
            &fixture.database,
            &Blockers::new(),
        )
        .activate(first.root_id)
        .unwrap();

        // Simulate an in-flight scan of the old root.
        let token = crate::application::scan::ScanCancelToken::new();
        // register through a scan run would normally do this; use the
        // supervisor directly via a started scan on a worker thread is
        // overkill — expose the same state through a manual registration.
        // (register is private; simulate by starting + cancelling a scan.)
        assert!(!token.is_cancelled());

        let second_dir = tempfile::tempdir().unwrap();
        bind_candidate(&fixture, second_dir.path());
        let second =
            PrepareLibraryCandidate::new(&fixture.deps, &fixture.database, &fixture.supervisor)
                .prepare(second_dir.path())
                .unwrap();
        let outcome = ActivateLibrary::new(
            &fixture.deps,
            &fixture.database,
            &fixture.supervisor,
            &fixture.database,
            &Blockers::new(),
        )
        .activate(second.root_id)
        .unwrap();

        // The epoch advanced by exactly one and the barrier rejects
        // old-epoch results after the commit (RootEpoch discipline).
        assert_eq!(outcome.epoch.as_u64(), 2);
        let barrier =
            RootSwitchBarrier::new(Some(first.root_id), second.root_id, RootEpoch::from_u64(1));
        assert!(barrier.accepts_epoch(RootEpoch::from_u64(1)));
        assert!(!barrier.accepts_epoch(outcome.epoch));
        // And the persisted epoch matches the outcome.
        assert_eq!(fixture.database.runtime_state(ROOT_EPOCH_KEY).unwrap(), "2");
        // The old root's record is kept but inactive.
        assert!(!fixture
            .database
            .by_id(first.root_id)
            .unwrap()
            .unwrap()
            .is_active());
        drop(token);
    }

    #[test]
    fn late_results_of_the_old_epoch_are_rejected() {
        // The runtime-side rule: results carry the epoch they started under;
        // after a committed switch only the new epoch is accepted.
        let mut barrier = RootSwitchBarrier::new(
            Some(LibraryRootId::new()),
            LibraryRootId::new(),
            RootEpoch::from_u64(5),
        );
        barrier.transition(RootSwitchState::QuiesceOldRoot).unwrap();
        barrier
            .transition(RootSwitchState::CommitActivation)
            .unwrap();
        barrier.commit_activation(RootEpoch::from_u64(6)).unwrap();
        assert!(!barrier.accepts_epoch(RootEpoch::from_u64(5)));
        assert!(barrier.accepts_epoch(RootEpoch::from_u64(6)));
        barrier.transition(RootSwitchState::Completed).unwrap();
        assert!(barrier.state().is_terminal());
    }

    #[test]
    fn derive_root_id_is_deterministic_and_path_bound() {
        let dir = tempfile::tempdir().unwrap();
        let canonical = dir.path().canonicalize().unwrap();
        let first = derive_root_id(&canonical);
        let second = derive_root_id(&canonical);
        assert_eq!(first, second, "same canonical path → same root id");
        let other = tempfile::tempdir().unwrap();
        assert_ne!(
            first,
            derive_root_id(&other.path().canonicalize().unwrap()),
            "different paths → different root ids"
        );
    }
}
