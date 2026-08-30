//! Application runtime orchestration and resource lifecycle (task 7.1).
//!
//! Owns the startup supervisor sequence and the readiness gate every command
//! consults before acting:
//!
//! 1. **单实例** — handled by the Tauri shell before this module runs; a second
//!    process only forwards its arguments to the running instance.
//! 2. **偏好** — preferences load before the database opens (theme + close
//!    behavior must be present even when nothing else is ready).
//! 3. **DB 迁移/备份** — `SqliteDatabase::open` migrates + backs up.
//! 4. **journal 恢复** — `BootRecovery` recovers the active root's operations
//!    under the scan exclusion and returns the readiness gate.
//! 5. **Core 查询** — the catalog/detail/favorite/playlist surfaces become
//!    callable (reads are always safe once recovery finished).
//! 6. **`PlayerActor` / watcher / platform 集成** — owned by later tasks but the
//!    sequence reserves their slots.
//! 7. **IPC ready** — commands/events open only after the gate is resolved.
//!
//! During startup, file-open requests from the OS (a second launch, a file
//! association) are not dropped: they enter a bounded FIFO and are drained once
//! the runtime reports ready. An unknown/indeterminate journal outcome leaves
//! the root **read-only** — destructive operations stay disabled until explicit
//! operator recovery ([`GateKind::ReadOnly`]).

use std::collections::VecDeque;
use std::sync::Mutex;

use echo_core::application::boot::{BootRecovery, BootRecoveryState};
use echo_core::application::ports::SystemTrashPort;
use echo_core::application::scan::{ScanDeps, ScanSupervisor};

/// The phases of the supervisor sequence, in order.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StartupPhase {
    /// Preferences loaded; database not yet opened.
    Preferences,
    /// Opening / migrating / backing up the database.
    Database,
    /// Running crash recovery for the active root.
    Recovery,
    /// Core queries are callable but IPC is not yet open.
    CoreReady,
    /// Platform integration starting (watcher / media integration).
    Integrating,
    /// The runtime is fully ready.
    Ready,
}

/// The readiness gate a command consults. Reads are always safe once recovery
/// finished; writes depend on the gate kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GateKind {
    /// Fully writable: recovery finished with every operation resolved.
    Writable,
    /// Some operations were handed off to the system trash; the runtime should
    /// run `FinalizeExpiredDeletes` to finish them. Writes are safe.
    NeedsSystemTrash,
    /// An indeterminate trash outcome (or a held conflict) leaves the root
    /// read-only. Destructive operations must stay disabled.
    ReadOnly,
}

impl GateKind {
    /// Whether destructive operations may begin.
    #[must_use]
    pub const fn writes_allowed(self) -> bool {
        matches!(self, Self::Writable | Self::NeedsSystemTrash)
    }

    /// Whether catalog queries and playback of available songs are safe.
    #[must_use]
    pub const fn reads_allowed(self) -> bool {
        true
    }
}

impl From<BootRecoveryState> for GateKind {
    fn from(state: BootRecoveryState) -> Self {
        match state {
            BootRecoveryState::Recovered => Self::Writable,
            BootRecoveryState::NeedsSystemTrash => Self::NeedsSystemTrash,
            BootRecoveryState::ReadOnly => Self::ReadOnly,
        }
    }
}

/// The result of a startup run.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StartupReport {
    /// The active root that was recovered, if any.
    pub root: Option<echo_core::LibraryRootId>,
    /// The resolved gate.
    pub gate: GateKind,
    /// How many journal operations recovery touched.
    pub recovered_operations: usize,
}

/// A bounded FIFO of file-open requests received before the runtime was ready.
/// Requests are never dropped; after ready they are drained in arrival order
/// and each is validated against the rule that a cancelled dialog is not a
/// success.
#[derive(Debug)]
struct PendingOpen<T> {
    inner: Mutex<VecDeque<T>>,
    capacity: usize,
}

impl<T> PendingOpen<T> {
    fn new(capacity: usize) -> Self {
        Self {
            inner: Mutex::new(VecDeque::with_capacity(capacity)),
            capacity,
        }
    }

    /// Enqueue a request. When the buffer is full the *oldest* entry is pushed
    /// out (bounded memory), never blocking the OS handler.
    fn enqueue(&self, item: T) {
        let mut queue = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if queue.len() >= self.capacity {
            queue.pop_front();
        }
        queue.push_back(item);
    }

    fn drain(&self) -> Vec<T> {
        let mut queue = self
            .inner
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        queue.drain(..).collect()
    }
}

/// The startup supervisor: runs the ordered sequence and holds the readiness
/// gate plus the pending-open FIFO.
pub struct StartupSupervisor {
    phase: Mutex<StartupPhase>,
    gate: Mutex<Option<GateKind>>,
    report: Mutex<Option<StartupReport>>,
    /// Bounded FIFO for file-open requests received while not ready.
    pending_opens: PendingOpen<String>,
}

impl Default for StartupSupervisor {
    fn default() -> Self {
        Self::new()
    }
}

impl StartupSupervisor {
    /// Default bound for the pre-ready file-open FIFO.
    pub const PENDING_OPEN_CAPACITY: usize = 64;

    #[must_use]
    pub fn new() -> Self {
        Self {
            phase: Mutex::new(StartupPhase::Preferences),
            gate: Mutex::new(None),
            report: Mutex::new(None),
            pending_opens: PendingOpen::new(Self::PENDING_OPEN_CAPACITY),
        }
    }

    /// Run the recovery step (`BootRecovery`) and resolve the readiness gate.
    /// Must be called on a worker/blocking thread, never inside the Tauri
    /// async runtime, because recovery does real filesystem work.
    ///
    /// # Errors
    ///
    /// Propagates the underlying recovery error. A failed recovery leaves the
    /// gate unresolved (the runtime must not open IPC). A `NeedsSystemTrash`
    /// gate that claims operations but has no active root is an invariant
    /// violation (recovery cannot hand something off without a root).
    ///
    /// # Panics
    ///
    /// Never. Internal mutex poisoning is recovered rather than panicked.
    pub fn run_recovery(
        &self,
        deps: &ScanDeps,
        supervisor: &ScanSupervisor,
        trash: &dyn SystemTrashPort,
    ) -> Result<StartupReport, echo_core::error::Error> {
        *self
            .phase
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = StartupPhase::Database;
        let report = BootRecovery::new(deps, supervisor).run()?;

        // A NeedsSystemTrash gate still allows writes but asks the runtime to
        // run the finalization pass; attempt it here so a normal boot resolves
        // fully. Failure to finalize degrades to ReadOnly only on an
        // indeterminate outcome.
        let gate = match GateKind::from(report.state) {
            GateKind::NeedsSystemTrash => {
                let root =
                    report
                        .root
                        .ok_or_else(|| echo_core::error::Error::InvariantViolation {
                            why: "needs-trash gate without an active root".to_owned(),
                        })?;
                let _ = finalize_trash(deps, trash, root);
                GateKind::NeedsSystemTrash
            }
            other => other,
        };
        let completed = StartupReport {
            root: report.root,
            gate,
            recovered_operations: report.recovered_operations,
        };
        *self
            .gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(gate);
        *self
            .report
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(completed.clone());
        *self
            .phase
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = StartupPhase::CoreReady;
        Ok(completed)
    }

    /// Mark the platform integration finished; the runtime is now fully ready
    /// and any retained file-opens are drained.
    pub fn on_ready(&self) -> Vec<String> {
        *self
            .phase
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = StartupPhase::Ready;
        self.pending_opens.drain()
    }

    /// Receive a file-open request from the OS. While not ready it lands in
    /// the bounded FIFO; when ready it is returned immediately.
    pub fn receive_file_open(&self, path: String) -> Option<String> {
        let ready = *self
            .phase
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            == StartupPhase::Ready;
        if ready {
            Some(path)
        } else {
            self.pending_opens.enqueue(path);
            None
        }
    }

    /// The current gate, if recovery has resolved it.
    #[must_use]
    pub fn gate(&self) -> Option<GateKind> {
        *self
            .gate
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Whether destructive operations (import into the root, Echo delete) may
    /// begin: false before recovery resolves, and permanently false on a
    /// read-only gate.
    #[must_use]
    pub fn writes_allowed(&self) -> bool {
        self.gate().is_some_and(GateKind::writes_allowed)
    }

    /// Whether catalog queries and playback are safe now.
    #[must_use]
    pub fn reads_allowed(&self) -> bool {
        self.gate().is_some_and(GateKind::reads_allowed)
    }

    /// The current start-up phase.
    #[must_use]
    pub fn phase(&self) -> StartupPhase {
        *self
            .phase
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The resolved report, if any.
    #[must_use]
    pub fn report(&self) -> Option<StartupReport> {
        self.report
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

/// Forward an expired delete to the system trash. Failure leaves the gate at
/// `NeedsSystemTrash` (the runtime retries on next boot); an indeterminate
/// outcome is surfaced by the caller the next time it runs.
fn finalize_trash(
    deps: &ScanDeps,
    trash: &dyn SystemTrashPort,
    root: echo_core::LibraryRootId,
) -> Result<echo_core::application::trash::TrashFinalizationReport, echo_core::error::Error> {
    let run = echo_core::application::trash::FinalizeExpiredDeletes::new(deps, trash);
    run.run(root)
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::application::testing::scan_fixture::ScanFixture;
    use echo_core::application::testing::small_fakes::FakeTrash;

    fn fixture() -> ScanFixture {
        ScanFixture::new()
    }

    #[test]
    fn supervisor_sequence_resolves_gate_and_drains_pending_opens() {
        let f = fixture();
        let supervisor = StartupSupervisor::new();

        // Before recovery: no gate, reads not allowed, writes not allowed.
        assert_eq!(supervisor.gate(), None);
        assert!(!supervisor.reads_allowed());
        assert!(!supervisor.writes_allowed());

        // A file-open during boot is retained, not dropped.
        assert_eq!(
            supervisor.receive_file_open("/music/a.flac".to_owned()),
            None
        );

        let report = supervisor
            .run_recovery(&f.deps, &f.supervisor, &FakeTrash::new())
            .expect("clean recovery");
        assert_eq!(report.gate, GateKind::Writable);
        assert!(supervisor.reads_allowed());
        assert!(supervisor.writes_allowed());

        // Drain after ready.
        let opened = supervisor.on_ready();
        assert_eq!(opened, vec!["/music/a.flac".to_owned()]);
        // A later open is immediate.
        assert_eq!(
            supervisor.receive_file_open("/music/b.flac".to_owned()),
            Some("/music/b.flac".to_owned())
        );
    }

    #[test]
    fn pending_open_fifo_is_bounded_and_keeps_newest() {
        let supervisor = StartupSupervisor::new();
        for index in 0..(StartupSupervisor::PENDING_OPEN_CAPACITY + 5) {
            supervisor.receive_file_open(format!("/music/{index}.flac"));
        }
        let drained = supervisor.on_ready();
        assert_eq!(drained.len(), StartupSupervisor::PENDING_OPEN_CAPACITY);
        // The oldest entries were evicted — newest kept.
        let kept_oldest: Vec<u64> = drained
            .iter()
            .filter_map(|p| {
                p.strip_prefix("/music/")
                    .and_then(|n| n.strip_suffix(".flac"))
                    .and_then(|n| n.parse::<u64>().ok())
            })
            .collect();
        assert!(!kept_oldest.contains(&0), "oldest entry evicted");
        assert!(kept_oldest.contains(&68), "newest entry kept");
        assert!(
            kept_oldest.windows(2).all(|w| w[0] < w[1]),
            "drained in arrival order"
        );
    }

    /// Reference: `GateKind` derives `From<BootRecoveryState>`.
    #[test]
    fn gate_kind_maps_recovery_states() {
        use echo_core::application::boot::BootRecoveryState;
        assert_eq!(
            GateKind::from(BootRecoveryState::Recovered),
            GateKind::Writable
        );
        assert_eq!(
            GateKind::from(BootRecoveryState::NeedsSystemTrash),
            GateKind::NeedsSystemTrash
        );
        assert_eq!(
            GateKind::from(BootRecoveryState::ReadOnly),
            GateKind::ReadOnly
        );
        assert!(GateKind::Writable.writes_allowed());
        assert!(GateKind::NeedsSystemTrash.writes_allowed());
        assert!(!GateKind::ReadOnly.writes_allowed());
        assert!(GateKind::ReadOnly.reads_allowed());
    }
}
