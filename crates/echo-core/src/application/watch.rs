//! File-event reconciliation (task 4.9, design §6.6).
//!
//! The watch coordinator turns normalized [`FileEvent`]s into library state:
//!
//! - **Full-scan buffering**: while a scan generation is in flight, events
//!   are buffered (not guessed at) and released afterwards — the scan's own
//!   reconcile already covers the tree.
//! - **Target-claim arbitration**: before reconciling a created/modified
//!   path, the coordinator consults the active operations' journal items. A
//!   path claimed by an in-flight import defers the event; a path whose file
//!   the import *already published* reconciles under the journal's
//!   **reserved** `SongId` — never a second UUID (the unique-hash constraint
//!   is a last line of defence, not an identity picker).
//! - **Degradation**: watcher overflow and unclassifiable renames arrive as
//!   `RescanNeeded` and are handed to the runtime's rescan request instead
//!   of being interpreted.
//!
//! Tests drive the coordinator with scripted (out-of-order, duplicated,
//! lost) events over the port fakes; the production feed is the notify
//! adapter, whose debounce/stability/overflow rules run upstream.

use std::sync::{Arc, Mutex};

use crate::application::ports::{CoverAssetRef, FileEvent, FileEventKind, OperationItem, TxAccess};
use crate::application::relink::{song_from_parsed, RelinkPlanner, Resolution};
use crate::application::scan::{
    parse_single_file, rewrap, FileOutcome, ParsedOutcome, ScanDeps, ScanSupervisor,
};
use crate::domain::entities::{LyricsSource, SongAvailability as Availability};
use crate::domain::entities::{MediaDiagnostic, Song, SongAvailability};
use crate::domain::ids::{LibraryRootId, OperationId, RelativeMediaPath, Revision, SongId};
use crate::domain::state::OperationState;
use crate::error::Error;

/// Why the coordinator requested a rescan instead of interpreting events.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RescanReason {
    /// The watcher degraded (overflow / error) — a full rescan is safest.
    WatcherOverflow,
    /// A rename could not be classified — an incremental rescan resolves it.
    UnclassifiableRename,
}

/// What handling one event did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EventOutcome {
    /// Library state updated (record refreshed / restored / created).
    Applied { song: SongId, created: bool },
    /// The path belongs to an in-flight operation; the event is deferred.
    Deferred { operation: OperationId },
    /// A scan is in flight: the event was buffered.
    Buffered,
    /// Degraded: the runtime should run a rescan.
    RescanRequested(RescanReason),
    /// Nothing to do (file already gone, duplicate content, diagnostic only).
    Ignored,
}

/// The runtime callbacks the coordinator needs. Injected as boxed closures —
/// the runtime knows its in-flight operations and owns the rescan trigger.
pub struct WatchCallbacks {
    /// Operations with live target claims (import/delete in flight).
    pub active_operations: Box<dyn Fn() -> Vec<OperationId> + Send + Sync>,
    /// Called when events degrade to a rescan request.
    pub request_rescan: Box<dyn Fn(RescanReason) + Send + Sync>,
}

impl WatchCallbacks {
    #[must_use]
    pub fn noop() -> Self {
        Self {
            active_operations: Box::new(Vec::new),
            request_rescan: Box::new(|_| {}),
        }
    }
}

/// The file-event reconciler. One instance serves one active root; the
/// runtime owns it for the lifetime of the binding.
pub struct WatchCoordinator {
    deps: Arc<ScanDeps>,
    supervisor: Arc<ScanSupervisor>,
    callbacks: WatchCallbacks,
    buffer: Mutex<Vec<FileEvent>>,
}

impl WatchCoordinator {
    #[must_use]
    pub const fn new(
        deps: Arc<ScanDeps>,
        supervisor: Arc<ScanSupervisor>,
        callbacks: WatchCallbacks,
    ) -> Self {
        Self {
            deps,
            supervisor,
            callbacks,
            buffer: Mutex::new(Vec::new()),
        }
    }

    /// Events buffered while a scan was in flight.
    #[must_use]
    pub fn buffered(&self) -> Vec<FileEvent> {
        self.buffer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Release (and clear) the buffer: process each event in order. Called
    /// by the runtime after a scan completes.
    ///
    /// # Errors
    ///
    /// Propagates the first persistence failure; already-applied events stay
    /// applied (handling is idempotent by path identity).
    pub fn release_buffer(&self) -> Result<Vec<EventOutcome>, Error> {
        let events = std::mem::take(
            &mut *self
                .buffer
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        events
            .iter()
            .map(|event| self.handle_event(event.clone()))
            .collect()
    }

    /// Handle one normalized event.
    ///
    /// # Errors
    ///
    /// Persistence failures propagate; event *semantics* never fail (a bad
    /// file is a diagnostic, exactly as in the scan pipeline).
    pub fn handle_event(&self, event: FileEvent) -> Result<EventOutcome, Error> {
        if event.kind == FileEventKind::RescanNeeded {
            let reason = if event.path.normalized() == crate::application::ports::RESCAN_SENTINEL {
                RescanReason::WatcherOverflow
            } else {
                RescanReason::UnclassifiableRename
            };
            (self.callbacks.request_rescan)(reason);
            return Ok(EventOutcome::RescanRequested(reason));
        }
        // A running scan owns the whole tree: buffer instead of guessing.
        if self.supervisor.is_scanning(event.root) {
            self.buffer
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(event);
            return Ok(EventOutcome::Buffered);
        }
        match event.kind {
            FileEventKind::Created | FileEventKind::Modified => {
                self.upsert_path(event.root, &event.path)
            }
            FileEventKind::Removed => self.remove_path(event.root, &event.path),
            FileEventKind::Renamed { from } => self.rename_path(event.root, &from, &event.path),
            FileEventKind::RescanNeeded => unreachable!("handled above"),
        }
    }

    /// Created/Modified: claim arbitration first, then normal reconciliation.
    fn upsert_path(
        &self,
        root: LibraryRootId,
        path: &RelativeMediaPath,
    ) -> Result<EventOutcome, Error> {
        // The file vanished between the event and reconciliation (a Remove
        // will follow): nothing to do.
        if self.deps.fs.file_meta(root, path).is_err() {
            return Ok(EventOutcome::Ignored);
        }
        // Target-claim arbitration: an import owns this path right now.
        for operation in (self.callbacks.active_operations)() {
            if let Some(item) = self.claim_for(operation, path)? {
                let terminal = matches!(
                    item.state,
                    OperationState::Completed
                        | OperationState::RolledBack
                        | OperationState::FailedRecoverable
                        | OperationState::DatabaseFinalized
                );
                if terminal {
                    continue; // stale claim: the operation is finished
                }
                let published = matches!(
                    item.state,
                    OperationState::PublishApplied | OperationState::DatabaseCommitted
                );
                if published {
                    // The file is fully published under a reserved identity:
                    // reconcile with THAT id — never a second UUID.
                    let Some(reserved) = item.song else {
                        return Err(Error::InvariantViolation {
                            why: "published journal item without a reserved SongId".to_owned(),
                        });
                    };
                    let created = self.deps.songs.by_id(reserved)?.is_none();
                    self.apply_reserved(root, reserved, path)?;
                    return Ok(EventOutcome::Applied {
                        song: reserved,
                        created,
                    });
                }
                // Still copying/validating/publishing: the operation will
                // finish the record; the event is deferred to it.
                return Ok(EventOutcome::Deferred { operation });
            }
        }
        // No claim: reconcile exactly like the scan pipeline does.
        match parse_single_file(&self.deps, root, path) {
            FileOutcome::Parsed(parsed) => {
                let mut planner = RelinkPlanner::new(self.deps.songs.all_in_root(root)?);
                match planner.resolve(&parsed.file) {
                    Resolution::Keep { song } | Resolution::Relink { song } => {
                        let entity = planner.song(song);
                        self.apply_outcome(root, entity, &parsed)?;
                        Ok(EventOutcome::Applied {
                            song,
                            created: false,
                        })
                    }
                    Resolution::Create => {
                        let id = self.deps.ids.new_song_id();
                        let entity = Some(song_from_parsed(id, root, &parsed.file));
                        planner.register_created(entity.clone().expect("just built"));
                        self.apply_outcome(root, entity, &parsed)?;
                        Ok(EventOutcome::Applied {
                            song: id,
                            created: true,
                        })
                    }
                    Resolution::Duplicate { .. } => {
                        // Duplicate content through the watcher: report,
                        // never merge into a second UUID.
                        self.deps.runs.record_issue(
                            root,
                            self.deps.runs.latest_generation(root)?.unwrap_or(0),
                            &MediaDiagnostic::new(
                                path.clone(),
                                "duplicate_content",
                                format!(
                                    "content hash already owned by another path: {}",
                                    parsed.file.hash
                                ),
                                false,
                            ),
                        )?;
                        Ok(EventOutcome::Ignored)
                    }
                }
            }
            // Fast-skip through the watcher still restores a missing record.
            FileOutcome::FastSkip { restore, .. } => {
                if let Some(id) = restore {
                    self.deps
                        .songs
                        .set_availability(id, SongAvailability::Available)?;
                    return Ok(EventOutcome::Applied {
                        song: id,
                        created: false,
                    });
                }
                Ok(EventOutcome::Ignored)
            }
            // Unsupported/corrupt/IO problems are diagnostics, not failures.
            FileOutcome::Diagnostic(_) => Ok(EventOutcome::Ignored),
        }
    }

    /// Removed: the record collapses to `Missing`; identity stays (spec:
    /// 删除歌曲后关联可见 — UUID and associations are kept).
    fn remove_path(
        &self,
        root: LibraryRootId,
        path: &RelativeMediaPath,
    ) -> Result<EventOutcome, Error> {
        let Some(song) = self.deps.songs.by_path(root, path)? else {
            return Ok(EventOutcome::Ignored);
        };
        if song.availability() == Availability::Available {
            self.deps
                .songs
                .set_availability(song.id(), Availability::Missing)?;
        }
        Ok(EventOutcome::Applied {
            song: song.id(),
            created: false,
        })
    }

    /// Renamed: keep the identity when the moved file matches the record's
    /// hash; otherwise treat as removal + normal addition.
    fn rename_path(
        &self,
        root: LibraryRootId,
        from: &RelativeMediaPath,
        to: &RelativeMediaPath,
    ) -> Result<EventOutcome, Error> {
        let Some(song) = self.deps.songs.by_path(root, from)? else {
            // No record at the old path: the destination is a plain addition.
            return self.upsert_path(root, to);
        };
        if self.deps.fs.file_meta(root, to).is_err() {
            // The destination is not there (yet): the source removal wins.
            return self.remove_path(root, from);
        }
        let hash = self.deps.hasher.hash(root, to)?;
        if song.blake3_hash() == Some(hash.as_str()) {
            // Move/rename keeps the UUID and all associations.
            let mut moved = song;
            moved.relink(to.clone());
            moved.restore_available();
            self.deps.songs.upsert(&moved)?;
            return Ok(EventOutcome::Applied {
                song: moved.id(),
                created: false,
            });
        }
        // Content changed across the rename: old record goes missing, the
        // new file enters normal identity resolution.
        self.deps
            .songs
            .set_availability(song.id(), Availability::Missing)?;
        self.upsert_path(root, to)
    }

    fn claim_for(
        &self,
        operation: OperationId,
        path: &RelativeMediaPath,
    ) -> Result<Option<OperationItem>, Error> {
        let items = self.deps.journal.items(operation)?;
        Ok(items
            .into_iter()
            .find(|item| item.claim_key == path.identity_key()))
    }

    /// Reconcile a file under a **fixed**, journal-reserved identity.
    fn apply_reserved(
        &self,
        root: LibraryRootId,
        song_id: SongId,
        path: &RelativeMediaPath,
    ) -> Result<(), Error> {
        match parse_single_file(&self.deps, root, path) {
            FileOutcome::Parsed(parsed) => {
                let entity = Some(song_from_parsed(song_id, root, &parsed.file));
                self.apply_outcome(root, entity, &parsed)
            }
            FileOutcome::FastSkip { .. } | FileOutcome::Diagnostic(_) => Ok(()),
        }
    }

    /// Shared persistence path with the scan pipeline: song + lyrics + cover
    /// in one transaction.
    fn apply_outcome(
        &self,
        root: LibraryRootId,
        entity: Option<Song>,
        parsed: &ParsedOutcome,
    ) -> Result<(), Error> {
        let song = entity.unwrap_or_else(|| {
            Song::new(
                self.deps.ids.new_song_id(),
                root,
                parsed.file.path.clone(),
                Revision::INITIAL,
            )
        });
        let embedded = parsed
            .embedded_lyrics
            .clone()
            .map(|candidate| rewrap(&candidate, LyricsSource::Embedded));
        let sidecar = parsed
            .sidecar_lyrics
            .clone()
            .map(|candidate| rewrap(&candidate, LyricsSource::Sidecar));
        let cover: Option<CoverAssetRef> = parsed.cover.clone();
        self.deps
            .uow
            .with_tx(Box::new(move |tx: &mut dyn TxAccess| {
                tx.upsert_song(&song)?;
                match embedded {
                    Some(candidate) => tx.set_lyrics_candidate(song.id(), &candidate)?,
                    None => tx.clear_lyrics_candidate(song.id(), LyricsSource::Embedded)?,
                }
                match sidecar {
                    Some(candidate) => tx.set_lyrics_candidate(song.id(), &candidate)?,
                    None => tx.clear_lyrics_candidate(song.id(), LyricsSource::Sidecar)?,
                }
                if let Some(cover) = cover {
                    tx.attach_cover(song.id(), &cover)?;
                }
                Ok(())
            }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::{OperationResourceKind, SongRepository};
    use crate::application::scan::{ScanCancelToken, StartScan};
    use crate::application::testing::ScanFixture;
    use crate::domain::entities::SongAvailability;
    use crate::domain::ids::{LibraryRootId, OperationId, RelativeMediaPath};
    use crate::domain::state::OperationState;

    fn event(fixture: &ScanFixture, kind: FileEventKind, path: &str) -> FileEvent {
        FileEvent {
            root: fixture.root,
            path: RelativeMediaPath::new(path).expect("valid path"),
            kind,
        }
    }

    fn created(fixture: &ScanFixture, path: &str) -> FileEvent {
        event(fixture, FileEventKind::Created, path)
    }

    fn removed(fixture: &ScanFixture, path: &str) -> FileEvent {
        event(fixture, FileEventKind::Removed, path)
    }

    /// A coordinator with recorded outcomes for assertions.
    struct Recorded {
        coordinator: WatchCoordinator,
        rescans: Arc<Mutex<Vec<RescanReason>>>,
        active: Arc<Mutex<Vec<OperationId>>>,
    }

    fn coordinator(fixture: &ScanFixture) -> Recorded {
        let rescans = Arc::new(Mutex::new(Vec::new()));
        let active = Arc::new(Mutex::new(Vec::new()));
        let rescans_sink = Arc::clone(&rescans);
        let active_source = Arc::clone(&active);
        let coordinator = WatchCoordinator::new(
            Arc::clone(&fixture.deps),
            Arc::new(fixture.supervisor.clone()),
            WatchCallbacks {
                active_operations: Box::new(move || active_source.lock().unwrap().clone()),
                request_rescan: Box::new(move |reason| {
                    rescans_sink.lock().unwrap().push(reason);
                }),
            },
        );
        Recorded {
            coordinator,
            rescans,
            active,
        }
    }

    #[test]
    fn watch_events_converge_under_out_of_order_and_duplicate_delivery() {
        let fixture = ScanFixture::new();
        fixture.write_file("a.mp3", b"audio-a");
        fixture.write_file("b.mp3", b"audio-b");
        fixture.set_audio("a.mp3", "A", 1_000);
        fixture.set_audio("b.mp3", "B", 1_000);
        let recorded = coordinator(&fixture);

        // Out-of-order + duplicated creates for two files (the adapter may
        // deliver any order; duplicates are idempotent by path identity).
        let _ = recorded
            .coordinator
            .handle_event(created(&fixture, "b.mp3"))
            .unwrap();
        let _ = recorded
            .coordinator
            .handle_event(created(&fixture, "a.mp3"))
            .unwrap();
        let _ = recorded
            .coordinator
            .handle_event(created(&fixture, "b.mp3"))
            .unwrap();

        let songs = fixture.all_songs();
        assert_eq!(songs.len(), 2, "no duplicate records for duplicate events");
        assert!(songs
            .iter()
            .all(|song| song.availability() == SongAvailability::Available));

        // A lost Created (only Modified arrives) still reconciles.
        fixture.write_file("c.mp3", b"audio-c");
        fixture.set_audio("c.mp3", "C", 1_000);
        let _ = recorded
            .coordinator
            .handle_event(event(&fixture, FileEventKind::Modified, "c.mp3"))
            .unwrap();
        assert_eq!(fixture.all_songs().len(), 3);

        // Remove converges to Missing, keeping the UUID; a later re-create
        // restores the SAME record.
        let c_path = fixture.path("c.mp3");
        let c_id = song_by_path(&fixture, &c_path).unwrap().unwrap().id();
        let _ = recorded
            .coordinator
            .handle_event(removed(&fixture, "c.mp3"))
            .unwrap();
        assert_eq!(
            song_by_path(&fixture, &c_path)
                .unwrap()
                .unwrap()
                .availability(),
            SongAvailability::Missing
        );
        let _ = recorded
            .coordinator
            .handle_event(created(&fixture, "c.mp3"))
            .unwrap();
        let restored = song_by_path(&fixture, &c_path).unwrap().unwrap();
        assert_eq!(restored.id(), c_id, "re-created file reuses the record");
        assert_eq!(restored.availability(), SongAvailability::Available);
    }

    #[test]
    fn rename_keeps_the_uuid() {
        let fixture = ScanFixture::new();
        fixture.write_file("old.mp3", b"audio");
        fixture.set_audio("old.mp3", "Song", 1_000);
        let recorded = coordinator(&fixture);
        let _ = recorded
            .coordinator
            .handle_event(created(&fixture, "old.mp3"))
            .unwrap();
        let song = song_by_path(&fixture, &fixture.path("old.mp3"))
            .unwrap()
            .unwrap();

        // Rename on disk + event.
        fixture.write_file("new.mp3", b"audio");
        fixture.remove_file("old.mp3");
        let _ = recorded
            .coordinator
            .handle_event(event(
                &fixture,
                FileEventKind::Renamed {
                    from: RelativeMediaPath::new("old.mp3").unwrap(),
                },
                "new.mp3",
            ))
            .unwrap();

        let moved = song_by_path(&fixture, &fixture.path("new.mp3"))
            .unwrap()
            .expect("record at the new path");
        assert_eq!(moved.id(), song.id(), "rename keeps the UUID");
        assert!(
            song_by_path(&fixture, &fixture.path("old.mp3"))
                .unwrap()
                .is_none(),
            "no record lingers at the old path"
        );
    }

    #[test]
    fn claimed_paths_defer_until_the_operation_publishes() {
        let fixture = ScanFixture::new();
        fixture.write_file("importing.mp3", b"importing-bytes");
        let recorded = coordinator(&fixture);

        // An import operation holds the target claim, still copying.
        let operation = OperationId::new();
        let reserved = crate::domain::ids::SongId::new();
        let item = crate::application::ports::OperationItem {
            kind: OperationResourceKind::Audio,
            state: OperationState::CopyPending,
            song: Some(reserved),
            source: None,
            staging_path: None,
            target_path: RelativeMediaPath::new("importing.mp3").unwrap(),
            expected_hash: "a".repeat(64),
            claim_key: RelativeMediaPath::new("importing.mp3")
                .unwrap()
                .identity_key()
                .to_owned(),
        };
        crate::application::ports::OperationJournalRepository::upsert_item(
            &fixture.database,
            operation,
            item,
        )
        .unwrap();
        recorded.active.lock().unwrap().push(operation);

        let outcome = recorded
            .coordinator
            .handle_event(created(&fixture, "importing.mp3"))
            .unwrap();
        assert_eq!(
            outcome,
            EventOutcome::Deferred { operation },
            "the in-flight import owns the path; the event is deferred"
        );
        assert!(
            fixture.all_songs().is_empty(),
            "no record is created while the claim is active"
        );
    }

    #[test]
    fn published_files_reuse_the_journal_reserved_song_id() {
        let fixture = ScanFixture::new();
        fixture.write_file("published.mp3", b"published-bytes");
        fixture.set_audio("published.mp3", "Published", 1_000);
        let recorded = coordinator(&fixture);

        // The import published the file (PublishApplied) but the DB commit
        // has not happened yet: the event races the operation.
        let operation = OperationId::new();
        let reserved = crate::domain::ids::SongId::new();
        let item = crate::application::ports::OperationItem {
            kind: OperationResourceKind::Audio,
            state: OperationState::PublishApplied,
            song: Some(reserved),
            source: None,
            staging_path: None,
            target_path: RelativeMediaPath::new("published.mp3").unwrap(),
            expected_hash: "a".repeat(64),
            claim_key: RelativeMediaPath::new("published.mp3")
                .unwrap()
                .identity_key()
                .to_owned(),
        };
        crate::application::ports::OperationJournalRepository::upsert_item(
            &fixture.database,
            operation,
            item,
        )
        .unwrap();
        recorded.active.lock().unwrap().push(operation);

        let outcome = recorded
            .coordinator
            .handle_event(created(&fixture, "published.mp3"))
            .unwrap();
        assert_eq!(
            outcome,
            EventOutcome::Applied {
                song: reserved,
                created: true,
            },
            "the event completes the record under the reserved identity"
        );
        let songs = fixture.all_songs();
        assert_eq!(songs.len(), 1);
        assert_eq!(
            songs[0].id(),
            reserved,
            "UUID equals the journal reservation — never a second UUID"
        );
        assert_eq!(songs[0].title(), Some("Published"));
    }

    #[test]
    fn full_scan_buffers_events_and_release_replays_them() {
        let fixture = ScanFixture::new();
        fixture.write_file("a.mp3", b"audio-a");
        fixture.set_audio("a.mp3", "A", 1_000);
        fixture.write_file("b.mp3", b"audio-b");
        fixture.set_audio("b.mp3", "B", 1_000);
        let recorded = coordinator(&fixture);

        // A scan starts (its token is registered with the supervisor). A slow
        // probe keeps it in flight long enough to observe the buffering.
        let deps = ScanDeps {
            probe: Arc::new(crate::application::testing::small_fakes::SlowProbe::new(
                Arc::clone(&fixture.deps.probe),
                std::time::Duration::from_millis(80),
            )),
            ..ScanDeps::clone(&fixture.deps)
        };
        let supervisor = fixture.supervisor.clone();
        let root = fixture.root;
        let scan_thread = std::thread::spawn(move || StartScan::new(&deps, &supervisor).run(root));
        // Wait until the scan registers (is_scanning) — bounded spin.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while !fixture.supervisor.is_scanning(root) {
            assert!(
                std::time::Instant::now() < deadline,
                "scan never registered"
            );
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        // Events during the scan are buffered, not applied.
        let outcome = recorded
            .coordinator
            .handle_event(created(&fixture, "b.mp3"))
            .unwrap();
        assert_eq!(outcome, EventOutcome::Buffered);
        assert_eq!(recorded.coordinator.buffered().len(), 1);

        let summary = scan_thread.join().unwrap().expect("scan ok");
        assert_eq!(
            summary.progress.state,
            crate::domain::state::scan::ScanState::Completed
        );
        // The scan itself created both files; the buffered event replays
        // idempotently afterwards (fast-skip → Ignored is fine — either way
        // there is exactly one record per file and no second UUID).
        let outcomes = recorded.coordinator.release_buffer().unwrap();
        assert_eq!(outcomes.len(), 1);
        assert!(matches!(
            outcomes.first(),
            Some(EventOutcome::Applied { .. } | EventOutcome::Ignored)
        ));
        assert_eq!(
            fixture.all_songs().len(),
            2,
            "still exactly one record per file"
        );
    }

    #[test]
    fn overflow_and_unclassifiable_degrade_to_rescan_requests() {
        let fixture = ScanFixture::new();
        let recorded = coordinator(&fixture);

        // Watcher overflow: the sentinel event.
        let outcome = recorded
            .coordinator
            .handle_event(FileEvent {
                root: fixture.root,
                path: RelativeMediaPath::new(crate::application::ports::RESCAN_SENTINEL).unwrap(),
                kind: FileEventKind::RescanNeeded,
            })
            .unwrap();
        assert_eq!(
            outcome,
            EventOutcome::RescanRequested(RescanReason::WatcherOverflow)
        );
        // Unclassifiable rename: a rescan signal on a real path.
        let outcome = recorded
            .coordinator
            .handle_event(FileEvent {
                root: fixture.root,
                path: RelativeMediaPath::new("somewhere.mp3").unwrap(),
                kind: FileEventKind::RescanNeeded,
            })
            .unwrap();
        assert_eq!(
            outcome,
            EventOutcome::RescanRequested(RescanReason::UnclassifiableRename)
        );
        let rescans = recorded.rescans.lock().unwrap().clone();
        assert_eq!(
            rescans,
            vec![
                RescanReason::WatcherOverflow,
                RescanReason::UnclassifiableRename
            ]
        );
    }

    #[test]
    fn scripted_stability_is_respected_by_the_case_level() {
        // The port-level scripted source (out-of-order + duplicate frames)
        // feeds the same coordinator loop the runtime uses.
        use crate::application::ports::FileEventSource as _;
        let fixture = ScanFixture::new();
        fixture.write_file("a.mp3", b"audio-a");
        fixture.set_audio("a.mp3", "A", 1_000);
        let recorded = coordinator(&fixture);

        let source = crate::application::testing::ScriptedFileEvents::new(vec![
            created(&fixture, "a.mp3"),
            created(&fixture, "a.mp3"),
        ]);
        let mut subscription = source.subscribe(fixture.root).unwrap();
        let mut outcomes = Vec::new();
        while let Some(event) = subscription.recv().unwrap() {
            outcomes.push(recorded.coordinator.handle_event(event).unwrap());
        }
        assert_eq!(outcomes.len(), 2);
        assert!(matches!(outcomes[0], EventOutcome::Applied { .. }));
        // The duplicate frame finds the record already present and unchanged:
        // the pipeline fast-skips it — idempotent, no second record.
        assert!(matches!(outcomes[1], EventOutcome::Ignored));
        assert_eq!(fixture.all_songs().len(), 1, "duplicates collapse");
    }

    #[test]
    fn events_for_unknown_files_are_ignored_cleanly() {
        let fixture = ScanFixture::new();
        let recorded = coordinator(&fixture);
        // Removed for a file Echo never knew.
        let outcome = recorded
            .coordinator
            .handle_event(removed(&fixture, "ghost.mp3"))
            .unwrap();
        assert_eq!(outcome, EventOutcome::Ignored);
        // Created but the file vanished before reconciliation.
        let outcome = recorded
            .coordinator
            .handle_event(created(&fixture, "vanishing.mp3"))
            .unwrap();
        assert_eq!(outcome, EventOutcome::Ignored);
        assert!(fixture.all_songs().is_empty());
        // Scan cancellation token still usable afterwards.
        let token = ScanCancelToken::new();
        token.cancel();
        assert!(token.is_cancelled());
        let _ = LibraryRootId::new();
    }

    fn song_by_path(
        fixture: &ScanFixture,
        path: &RelativeMediaPath,
    ) -> Result<Option<crate::domain::entities::Song>, Error> {
        SongRepository::by_path(&fixture.database, fixture.root, path)
    }
}
