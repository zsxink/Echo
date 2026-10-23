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

use crate::application::continuation::{ContinuationReport, ContinueFromRecords};
use crate::application::ports::{
    LibraryRepository, RootWriteRepository, RuntimeStateStore, TxAccess,
};
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
    /// The object-record continuation that ran before the scan, when the
    /// directory carries a portable control surface (`echo/`). `None` for a
    /// legacy directory without one — those stay pure scan targets so the
    /// pre-portable behaviour is untouched.
    pub continuation: Option<ContinuationReport>,
    /// `true` when the control surface refused continuation (a manifest newer
    /// than this build, or an unreadable one). The candidate is still usable
    /// for reads, but must activate read-only: its user data cannot be
    /// materialized locally.
    pub control_plane_degraded: bool,
}

impl PreparedCandidate {
    /// Whether the root may accept logic changes (writes) after preparation.
    #[must_use]
    pub const fn writes_allowed(&self) -> bool {
        self.write_capable && !self.control_plane_degraded
    }
}

/// Prepares a candidate root: record, full enumeration, candidate scan.
pub struct PrepareLibraryCandidate<'a> {
    deps: &'a ScanDeps,
    roots: &'a dyn RootWriteRepository,
    supervisor: &'a ScanSupervisor,
}

impl<'a> PrepareLibraryCandidate<'a> {
    #[must_use]
    pub const fn new(
        deps: &'a ScanDeps,
        roots: &'a dyn RootWriteRepository,
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
        // Acquire write capability for the candidate by establishing its owned
        // staging dir (design §8: 首次获得写能力时 exclusive-create). A fresh
        // writable directory must activate writable — otherwise imports/deletes
        // are permanently LibraryUnavailable on first use. Idempotent.
        //
        // Best-effort: a genuinely read-only directory (OS refused the write,
        // or the staged marker cannot be created) must NOT fail preparation —
        // the root still activates read-only so scanning/playback work and only
        // imports/deletes are disabled (spec: 只读根目录允许扫描和播放).
        let _ = self.deps.fs.establish_write_capability(root_id);
        // `write_capable` reflects permission + ownership marker (task 4.2).
        let observed_write_capable = self.deps.fs.write_capable(root_id)?;
        let write_safety_locked = existing
            .as_ref()
            .is_some_and(LibraryRoot::write_safety_locked);
        let write_capable = observed_write_capable && !write_safety_locked;
        let mut prepared = LibraryRoot::new(root_id, canonical, false, observed_write_capable);
        prepared.set_write_safety_locked(write_safety_locked);
        self.roots.upsert(&prepared)?;

        // Continuation before the scan (design D2): a directory that already
        // owns object records must hand its UUIDs to the local store *before*
        // `media/` is enumerated, or every file looks new and gets a fresh
        // identity — the data loss behind issue #1.
        let (continuation, control_plane_degraded) =
            self.continue_from_records(root_id, write_capable);

        // An unsupported control surface remains browseable through its existing
        // projection. Scanning could mint identities or write a format we do
        // not understand, so explicitly report that no scan was performed.
        if control_plane_degraded {
            return Ok(PreparedCandidate {
                root_id,
                reused,
                write_capable,
                continuation,
                control_plane_degraded,
                scan: ScanSummary {
                    generation: self.deps.runs.latest_generation(root_id)?.unwrap_or(0),
                    cancelled: true,
                    progress: crate::domain::state::scan::ScanProgress {
                        state: crate::domain::state::scan::ScanState::Cancelled,
                        ..Default::default()
                    },
                },
            });
        }

        // Candidate scan: complete enumeration + reconcile required.
        match StartScan::new(self.deps, self.supervisor).run(root_id) {
            Ok(scan) => Ok(PreparedCandidate {
                root_id,
                reused,
                write_capable,
                scan,
                continuation,
                control_plane_degraded,
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

    /// Continue the candidate's local state from its portable object records.
    ///
    /// Only writable roots are touched: a read-only root cannot materialize
    /// what it adopts, so it keeps the pre-existing scan-only behaviour.
    ///
    /// A refusal (`unsupported_media` — a newer/unreadable manifest) is *not* a
    /// preparation failure: the directory still opens for reads, flagged
    /// degraded so activation drops write capability. Every other failure
    /// propagates and leaves the old active root serving.
    fn continue_from_records(
        &self,
        root_id: LibraryRootId,
        write_capable: bool,
    ) -> (Option<ContinuationReport>, bool) {
        let Ok(_activity) = super::sync::local_activity() else {
            return (None, true);
        };
        if !write_capable
            || self
                .roots
                .replica_enrollment_pending(root_id)
                .unwrap_or(true)
        {
            return (None, false);
        }
        match ContinueFromRecords::new(
            self.deps.control.as_ref(),
            self.deps.songs.as_ref(),
            self.deps.playlists.as_ref(),
            self.deps.uow.as_ref(),
        )
        .run(root_id)
        {
            Ok(report) => (Some(report), false),
            Err(error) if error.code() == "unsupported_media" => {
                tracing::warn!(
                    reason = %error,
                    "portable control surface is newer than this build — opening read-only"
                );
                (None, true)
            }
            Err(error) => {
                tracing::warn!(
                    reason = %error,
                    "object-record continuation failed — the candidate falls back to a plain scan"
                );
                (None, false)
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
    // error type stays boundary-internal.
    Error::InvariantViolation {
        why: error.to_string(),
    }
}

#[cfg(test)]
mod tests;
