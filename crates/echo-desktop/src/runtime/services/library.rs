//! Library status, root switch and scan control (task 7.3, design §5).
//!
//! The whole pick → candidate-scan → activate barrier stays on the Rust side;
//! the UI receives only a path-free outcome.

use echo_core::application::root_switch::{
    derive_root_id, ActivateLibrary, PrepareLibraryCandidate,
};
use echo_core::application::scan::{CancelScan, ScanSummary, StartScan};
use echo_core::domain::ids::LibraryRootId;
use echo_core::error::Error;

use crate::ipc::dto::{LibraryRootStatusDto, LibraryStatus, ScanSnapshot};

impl super::AppServices {
    /// A read-only snapshot of the library's availability + write capability.
    /// This never mutates; it answers "is the library usable, and can I write?"
    /// without exposing any absolute path.
    ///
    /// # Errors
    ///
    /// Storage errors propagate.
    pub fn library_status(&self) -> Result<LibraryStatus, Error> {
        let root = self.deps.roots.active_root()?;
        let scanning = root
            .as_ref()
            .is_some_and(|root| self.supervisor.is_scanning(root.id()));
        Ok(LibraryStatus {
            configured: root.is_some(),
            read_only: root.as_ref().is_some_and(|root| !root.write_capable()),
            unavailable: root.as_ref().is_some_and(|root| {
                !matches!(
                    root.availability(),
                    echo_core::domain::entities::RootAvailability::Available
                )
            }),
            scanning,
            active_root: root.map(|root| root.id().to_string()),
        })
    }

    /// Run the directory picker and, on a confirmed selection, prepare and
    /// activate the chosen library root (design §5). The whole flow — pick,
    /// candidate scan, activation barrier — stays on the Rust/desktop side; the
    /// UI receives only a path-free, relative/none outcome. A cancelled dialog
    /// returns `Ok(None)` — never an empty or fake success.
    ///
    /// # Errors
    ///
    /// - `Unavailable` when the picker errored or the candidate could not be
    ///   prepared (unreadable, scan failed) — the old active root is untouched.
    /// - `Conflict` when a root switch is blocked by active operations
    ///   (in-flight import / delete-undo window) or the candidate's scan
    ///   failed (resolve and re-prepare before activating).
    /// - `InvariantViolation` if the assembled runtime-state store is missing —
    ///   the desktop's real stack always provides it.
    /// - Storage errors propagate.
    pub fn choose_library_root(&self) -> Result<Option<LibraryRootStatusDto>, Error> {
        let Some(absolute) = self.dialogs.pick_library_directory()? else {
            // Cancelled: not an error, not a success — a genuine no-op.
            return Ok(None);
        };

        // Bind the directory under its derivable root id so the file-system
        // adapters can resolve it during the candidate scan (Core never stores
        // the absolute path beyond the root record).
        let canonical = absolute.canonicalize().map_err(|source| {
            Error::io("resolve chosen library directory", source, absolute.clone())
        })?;
        let root_id = derive_root_id(&canonical);
        self.registry.register(root_id, canonical.clone());

        // Candidate scan + two-phase activation.
        let prepare =
            PrepareLibraryCandidate::new(&self.deps, self.deps.roots.as_ref(), &self.supervisor)
                .prepare(&canonical)?;
        if !self.startup.writes_allowed() {
            // A read-only gate (recovery left an indeterminate outcome, or the
            // runtime has not resolved) still allows reads on the *current*
            // root, but a root *switch* is refused: activating would advance the
            // persisted epoch over a library whose operations are unresolved.
            // The old root keeps serving (spec: 切换失败保留旧库).
            return Err(Error::unavailable(
                "library",
                "library is not in a switchable state",
            ));
        }
        let _outcome = ActivateLibrary::new(
            &self.deps,
            self.deps.roots.as_ref(),
            &self.supervisor,
            self.state.as_ref(),
            &self.blockers,
        )
        .activate(root_id)?;

        let mut status = LibraryRootStatusDto::configured(
            root_id,
            !prepare.write_capable || prepare.control_plane_degraded,
        );
        // The continuation report (design D7): what this open recovered from
        // `echo/records/`, whether the manifest had to be healed, and how many
        // records could not be placed.
        status.control_plane_read_only = prepare.control_plane_degraded;
        if let Some(continuation) = prepare.continuation.as_ref() {
            status.manifest_healed = continuation.control_plane.healed();
            status.continued_songs = continuation.projection.songs;
            status.continued_favorites = continuation.projection.favorites;
            status.continued_playlists = continuation.projection.playlists;
            status.continued_members = continuation.projection.members;
            status.continued_play_stats = continuation.projection.play_stats;
            status.unusable_records =
                continuation.projection.invalid + continuation.projection.superseded;
        }
        Ok(Some(status))
    }

    /// Start a full scan of `root`. Blocking: the Tauri layer calls this on a
    /// worker thread. Returns the terminal summary.
    ///
    /// # Errors
    ///
    /// `Conflict` when a scan is already running for the root; root-level scan
    /// failures propagate.
    pub fn start_scan(&self, root: LibraryRootId) -> Result<ScanSnapshot, Error> {
        let summary: ScanSummary = StartScan::new(&self.deps, &self.supervisor).run(root)?;
        Ok(ScanSnapshot::from(&summary))
    }

    /// Cancel the active scan of `root`; `true` when one was cancelled.
    #[must_use]
    pub fn cancel_scan(&self, root: LibraryRootId) -> bool {
        CancelScan::new(&self.supervisor).cancel(root)
    }
}
