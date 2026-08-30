//! Startup recovery before runtime-ready (task 5.10, design §8 startup rules).
//!
//! The runtime must finish import/delete crash recovery for the active root
//! before it starts the watcher, player and IPC handlers — otherwise the same
//! path could be scanned, imported, deleted or played while recovery is
//! reshuffling staged/published files. This module owns that single pre-ready
//! step ([`BootRecovery`]) and the per-root readiness gate
//! ([`BootRecoveryState`]) the runtime consults.
//!
//! While recovery runs, the active root's scan exclusion is held (via the
//! shared [`ScanSupervisor`]), so a concurrent scan of the same path is
//! rejected with `conflict`. Imports, deletes and playback are sequenced
//! after this step: the runtime exposes those entrypoints only once
//! [`BootRecovery::run`] has returned its gate, so none of them can observe
//! a half-recovered path.
//!
//! This is deliberately *one* startup step — task 7.1 owns the higher-level
//! order (single instance → preferences → DB → recovery → Core → player →
//! watcher → IPC ready). `echo-core` provides the recovery + binding gate
//! here and stays platform-neutral.

use crate::application::recover::RecoverOperations;
use crate::application::scan::{ScanCancelToken, ScanDeps, ScanSupervisor};
use crate::domain::ids::LibraryRootId;
use crate::error::Error;

/// The runtime's readiness for the active root after the pre-ready recovery
/// step. This is the contract the runtime (task 7.1) waits on before starting
/// the watcher and player.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BootRecoveryState {
    /// Recovery reached a stable terminal state; the root is safe for both
    /// reads and writes.
    Recovered,
    /// Some delete operations reached `TrashPending` and were handed off for
    /// the runtime's `SystemTrashPort` retry (nothing was inferred from a
    /// missing path). Reads and writes are safe, but the runtime should run
    /// `FinalizeExpiredDeletes` to finish them.
    NeedsSystemTrash,
    /// The root is durably isolated — a `TrashOutcomeUnknown` or a held
    /// `FailedRecoverable` conflict. Destructive operations must stay
    /// disabled until explicit operator recovery; reads (catalog/play of
    /// available songs) remain safe.
    ReadOnly,
}

impl BootRecoveryState {
    /// Whether the root may serve catalog queries and playback of available
    /// songs right now.
    #[must_use]
    pub const fn reads_safe(self) -> bool {
        matches!(
            self,
            Self::Recovered | Self::NeedsSystemTrash | Self::ReadOnly
        )
    }

    /// Whether destructive operations (import into the root, Echo delete) may
    /// begin. `ReadOnly` forbids them.
    #[must_use]
    pub const fn writes_safe(self) -> bool {
        matches!(self, Self::Recovered | Self::NeedsSystemTrash)
    }

    /// Whether the runtime still owes a `SystemTrashPort` finalization pass.
    #[must_use]
    pub const fn needs_system_trash(self) -> bool {
        matches!(self, Self::NeedsSystemTrash)
    }
}

/// The pre-ready recovery report for the active root.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BootReport {
    /// The active root that was recovered, if the app has one yet.
    pub root: Option<LibraryRootId>,
    /// The readiness gate for that root.
    pub state: BootRecoveryState,
    /// Operations driven to a terminal state (or handed off) in this pass.
    pub recovered_operations: usize,
}

/// The single pre-ready step: recover the active root under a per-root scan
/// exclusion and surface the readiness gate. `echo-desktop`'s supervisor
/// calls [`Self::run`] on a worker thread before it starts the watcher,
/// player and IPC handlers.
pub struct BootRecovery<'a> {
    deps: &'a ScanDeps,
    supervisor: &'a ScanSupervisor,
}

impl<'a> BootRecovery<'a> {
    #[must_use]
    pub const fn new(deps: &'a ScanDeps, supervisor: &'a ScanSupervisor) -> Self {
        Self { deps, supervisor }
    }

    /// Recover the active root and return its readiness gate.
    ///
    /// # Errors
    ///
    /// Propagates only infrastructure failures while reading the journal or
    /// the filesystem (the same surface [`RecoverOperations::run`] raises);
    /// per-item conflicts are folded into the returned gate, not raised here.
    /// A root that is already scanning is a `conflict` (the runtime must not
    /// start a scan before its pre-ready recovery).
    pub fn run(&self) -> Result<BootReport, Error> {
        let Some(root) = self.deps.roots.active_root()? else {
            return Ok(BootReport {
                root: None,
                state: BootRecoveryState::Recovered,
                recovered_operations: 0,
            });
        };
        let root = root.id();

        // Hold the root's scan exclusion for the whole recovery window: no
        // scan can start on this path until recovery has settled it.
        self.supervisor.register(root, ScanCancelToken::new())?;
        let result = self.recover_held(root);
        self.supervisor.unregister(root);
        result
    }

    /// Recover `root` while its scan exclusion is already held.
    fn recover_held(&self, root: LibraryRootId) -> Result<BootReport, Error> {
        let report = RecoverOperations::new(self.deps).run(root)?;
        let isolated = self
            .deps
            .roots
            .by_id(root)?
            .is_some_and(|record| record.write_safety_locked());
        let held = report.touched.iter().any(|op| op.held);
        let handed_off = report.touched.iter().any(|op| op.handed_off);
        let state = if isolated || held {
            BootRecoveryState::ReadOnly
        } else if handed_off {
            BootRecoveryState::NeedsSystemTrash
        } else {
            BootRecoveryState::Recovered
        };
        Ok(BootReport {
            root: Some(root),
            state,
            recovered_operations: report.touched.len(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::delete::DeleteSongs;
    use crate::application::ports::{LibraryRepository, SongRepository};
    use crate::application::scan::StartScan;
    use crate::application::testing::{scan_fixture::song_by_path, ScanFixture};
    use crate::domain::entities::{Song, SongAvailability};
    use crate::domain::ids::Revision;

    /// Seed one library song at `path` (favorite + one play), returning its id
    /// — the fixture state a boot recovery must never disturb.
    fn seed_song(fixture: &ScanFixture, path: &str) -> crate::domain::ids::SongId {
        fixture.write_file(path, b"audio-bytes");
        fixture.set_audio(path, "晴天", 269_000);
        let mut song = Song::new(
            crate::domain::ids::SongId::new(),
            fixture.root,
            fixture.path(path),
            Revision::INITIAL,
        );
        song.apply_scan_facts(
            fixture.deps.hasher.hash_of_bytes(b"audio-bytes"),
            11,
            1,
            crate::domain::media::AudioFormat::Flac,
        );
        song.set_favorite(true);
        song.record_play();
        SongRepository::upsert(&fixture.database, &song).expect("seed song");
        song.id()
    }

    #[test]
    fn no_active_root_is_an_immediate_clean_ready() {
        let fixture = ScanFixture::new();
        // No active root: BootRecovery must not fail and must not hold the
        // (absent) root.
        let report = BootRecovery::new(&fixture.deps, &fixture.supervisor)
            .run()
            .expect("clean ready with no active root");
        assert_eq!(report.root, None);
        assert_eq!(report.state, BootRecoveryState::Recovered);
        assert_eq!(report.recovered_operations, 0);
    }

    #[test]
    fn clean_active_root_recovers_idle_and_releases_the_scan_exclusion() {
        let fixture = ScanFixture::new();
        let song = seed_song(&fixture, "歌手/晴天.flac");
        LibraryRepository::upsert(
            &fixture.database,
            &crate::domain::entities::LibraryRoot::new(
                fixture.root,
                fixture
                    .fs
                    .root_path(fixture.root)
                    .expect("root path")
                    .canonicalize()
                    .unwrap_or_else(|_| fixture.fs.root_path(fixture.root).unwrap()),
                true,
                true,
            ),
        )
        .expect("activate root");
        crate::application::ports::LibraryRepository::set_write_and_availability(
            &fixture.database,
            fixture.root,
            true,
            true,
        )
        .expect("root writable");

        let report = BootRecovery::new(&fixture.deps, &fixture.supervisor)
            .run()
            .expect("boot recovery");
        assert_eq!(report.root, Some(fixture.root));
        assert_eq!(report.state, BootRecoveryState::Recovered);
        assert_eq!(report.recovered_operations, 0);
        assert!(report.state.writes_safe());
        assert!(report.state.reads_safe());

        // The exclusion is released: a normal scan may start after ready.
        StartScan::new(&fixture.deps, &fixture.supervisor)
            .run(fixture.root)
            .expect("scan after ready");
        assert_eq!(
            song_by_path(&fixture, &fixture.path("歌手/晴天.flac"))
                .unwrap()
                .unwrap()
                .availability(),
            SongAvailability::Available
        );
        assert_eq!(
            song_by_path(&fixture, &fixture.path("歌手/晴天.flac"))
                .unwrap()
                .unwrap()
                .id(),
            song
        );
    }

    #[test]
    fn held_scan_exclusion_rejects_a_concurrent_scan_of_the_same_path() {
        let fixture = ScanFixture::new();
        seed_song(&fixture, "歌手/晴天.flac");

        // While the root is held (exactly what BootRecovery does for its
        // recovery window), a concurrent scan of the same path is rejected.
        fixture
            .supervisor
            .register(fixture.root, ScanCancelToken::new())
            .expect("boot holds the root");
        let error = StartScan::new(&fixture.deps, &fixture.supervisor)
            .run(fixture.root)
            .expect_err("a scan must not start while recovery holds the root");
        assert_eq!(error.code(), "conflict");

        // Releasing the hold allows the scan again.
        fixture.supervisor.unregister(fixture.root);
        StartScan::new(&fixture.deps, &fixture.supervisor)
            .run(fixture.root)
            .expect("scan after the recovery hold is released");
    }

    #[test]
    fn boot_recovers_an_expired_delete_and_hands_it_to_system_trash() {
        let fixture = ScanFixture::new();
        let song = seed_song(&fixture, "歌手/晴天.flac");
        // Activate the root so BootRecovery targets it.
        LibraryRepository::upsert(
            &fixture.database,
            &crate::domain::entities::LibraryRoot::new(
                fixture.root,
                fixture.fs.root_path(fixture.root).unwrap(),
                true,
                true,
            ),
        )
        .expect("activate root");

        // An Echo delete is started and its 10 s undo window expires, leaving
        // a HiddenInDatabase op that a pre-ready pass must roll forward.
        DeleteSongs::new(&fixture.deps)
            .delete(fixture.root, song)
            .expect("delete");
        fixture.clock.advance_ms(20_000);

        let report = BootRecovery::new(&fixture.deps, &fixture.supervisor)
            .run()
            .expect("boot recovery");
        assert_eq!(report.root, Some(fixture.root));
        assert_eq!(
            report.state,
            BootRecoveryState::NeedsSystemTrash,
            "the expired delete is handed off for the SystemTrashPort, not marked done"
        );
        assert_eq!(report.recovered_operations, 1);
        assert!(report.state.writes_safe());
        assert!(report.state.needs_system_trash());
    }

    #[test]
    fn write_safety_locked_root_is_read_only_and_writes_stay_disabled() {
        let fixture = ScanFixture::new();
        seed_song(&fixture, "歌手/晴天.flac");
        LibraryRepository::upsert(
            &fixture.database,
            &crate::domain::entities::LibraryRoot::new(
                fixture.root,
                fixture.fs.root_path(fixture.root).unwrap(),
                true,
                true,
            ),
        )
        .expect("activate root");

        // A durable TrashOutcomeUnknown safety isolation (task 5.8) must
        // surface as a read-only gate and keep destructive ops disabled.
        LibraryRepository::set_write_safety_locked(&fixture.database, fixture.root, true)
            .expect("lock root");

        let report = BootRecovery::new(&fixture.deps, &fixture.supervisor)
            .run()
            .expect("boot recovery");
        assert_eq!(report.state, BootRecoveryState::ReadOnly);
        assert!(!report.state.writes_safe());
        assert!(report.state.reads_safe());

        let error = DeleteSongs::new(&fixture.deps)
            .delete(fixture.root, seed_song(&fixture, "another.flac"))
            .expect_err("deletes stay disabled on a read-only root after boot");
        assert_eq!(error.code(), "unavailable");
    }
}
