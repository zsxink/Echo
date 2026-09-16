//! Forward an expired Echo delete into the operating system's recycle bin.
//!
//! The platform call and `SQLite` cannot share a transaction. Consequently this
//! module treats the durable `TrashApplied` journal state—not a missing staged
//! path—as the sole proof that database cleanup may run. Any ambiguous outcome
//! keeps all song relationships and persistently disables writes for the root.

use std::collections::BTreeMap;

use crate::application::delete::DELETE_OPERATION;
use crate::application::ports::{OperationItem, SystemTrashPort, TxAccess};
use crate::application::scan::ScanDeps;
use crate::domain::entities::SongAvailability;
use crate::domain::ids::{LibraryRootId, OperationId, SongId};
use crate::domain::state::OperationState;
use crate::error::Error;

/// Summary of one irreversible-delete pass.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TrashFinalizationReport {
    /// Operations whose records were removed after a persisted trash receipt.
    pub finalized: Vec<OperationId>,
    /// Operations kept for a later system-trash retry.
    pub retryable: Vec<OperationId>,
    /// Operations with no safe proof of the system-trash result.
    pub outcome_unknown: Vec<OperationId>,
}

/// Advance expired delete operations through the system-trash irreversible
/// point. The desktop runtime supplies the platform adapter through
/// [`SystemTrashPort`]; `echo-core` remains platform-neutral.
pub struct FinalizeExpiredDeletes<'a> {
    deps: &'a ScanDeps,
    trash: &'a dyn SystemTrashPort,
}

impl<'a> FinalizeExpiredDeletes<'a> {
    #[must_use]
    pub const fn new(deps: &'a ScanDeps, trash: &'a dyn SystemTrashPort) -> Self {
        Self { deps, trash }
    }

    /// Process every incomplete delete operation of one root.
    ///
    /// A platform-call failure leaves `TrashPending` intact for retry. An
    /// ambiguous result is not an error return because the safe, user-visible
    /// state is durable `TrashOutcomeUnknown` plus disabled root writes.
    ///
    /// # Errors
    ///
    /// Returns an infrastructure or journal error while persisting a known
    /// transition. A later startup recovery can safely retry such failures.
    pub fn run(&self, root: LibraryRootId) -> Result<TrashFinalizationReport, Error> {
        let mut deletes: BTreeMap<OperationId, Vec<OperationItem>> = BTreeMap::new();
        for (operation, kind, item) in self.deps.journal.incomplete_items(root)? {
            if kind == DELETE_OPERATION {
                deletes.entry(operation).or_default().push(item);
            }
        }

        let mut report = TrashFinalizationReport::default();
        for (operation, items) in deletes {
            if root_is_write_locked(self.deps, root)? {
                // A safety lock is a hard batch barrier. The only safe work
                // left is irreversible database cleanup backed by an already
                // persisted TrashApplied receipt; no filesystem call or other
                // journal mutation may run for this root in this pass.
                if all_state(&items, OperationState::TrashApplied) {
                    finalize_persisted_trash(self.deps, operation, &items)?;
                    report.finalized.push(operation);
                }
                continue;
            }
            if !is_forward_candidate(&items) {
                // Stage/restore and recoverable-failure states belong to the
                // delete recovery matrix. The system trash has not been
                // attempted, so it must not turn them into an unknown outcome.
                continue;
            }
            let state = self.advance_operation(root, operation, &items)?;
            match state {
                ForwardResult::Finalized => report.finalized.push(operation),
                ForwardResult::Retryable => report.retryable.push(operation),
                ForwardResult::OutcomeUnknown => report.outcome_unknown.push(operation),
                ForwardResult::NotExpired => {}
            }
        }
        Ok(report)
    }

    fn advance_operation(
        &self,
        root: LibraryRootId,
        operation: OperationId,
        initial: &[OperationItem],
    ) -> Result<ForwardResult, Error> {
        let items = if all_state(initial, OperationState::HiddenInDatabase) {
            let Some(deadline) = self.deps.journal.undo_deadline(operation)? else {
                return mark_outcome_unknown(self.deps, root, operation, initial, false);
            };
            if now_ms(self.deps)? <= deadline {
                return Ok(ForwardResult::NotExpired);
            }
            persist_state(self.deps, operation, initial, OperationState::TrashPending)?;
            self.deps.journal.items(operation)?
        } else {
            initial.to_vec()
        };

        if all_state(&items, OperationState::TrashApplied) {
            finalize_persisted_trash(self.deps, operation, &items)?;
            return Ok(ForwardResult::Finalized);
        }
        if !all_state(&items, OperationState::TrashPending) {
            return mark_outcome_unknown(self.deps, root, operation, &items, false);
        }
        match staging_evidence(self.deps, root, &items) {
            StagingEvidence::Intact => {}
            StagingEvidence::MissingOrMismatched => {
                return mark_outcome_unknown(self.deps, root, operation, &items, false);
            }
            StagingEvidence::RootUnavailable => {
                return mark_outcome_unknown(self.deps, root, operation, &items, true);
            }
        }

        // The irreversible boundary. A return of Ok is the platform adapter's
        // explicit success receipt; no filesystem absence is consulted after it.
        if self.trash.send_to_trash(root, operation).is_err() {
            // An error can mean either a confirmed pre-call rejection or a
            // failure after the platform started moving the directory. Recheck
            // staged evidence before retrying; absence/unavailability is an
            // indeterminate destructive outcome, never a retry assumption.
            return match staging_evidence(self.deps, root, &items) {
                StagingEvidence::Intact => Ok(ForwardResult::Retryable),
                StagingEvidence::MissingOrMismatched => {
                    mark_outcome_unknown(self.deps, root, operation, &items, false)
                }
                StagingEvidence::RootUnavailable => {
                    mark_outcome_unknown(self.deps, root, operation, &items, true)
                }
            };
        }
        persist_state(self.deps, operation, &items, OperationState::TrashApplied)?;
        let applied = self.deps.journal.items(operation)?;
        finalize_persisted_trash(self.deps, operation, &applied)?;
        Ok(ForwardResult::Finalized)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ForwardResult {
    Finalized,
    Retryable,
    OutcomeUnknown,
    NotExpired,
}

fn all_state(items: &[OperationItem], state: OperationState) -> bool {
    !items.is_empty() && items.iter().all(|item| item.state == state)
}

fn is_forward_candidate(items: &[OperationItem]) -> bool {
    all_state(items, OperationState::HiddenInDatabase)
        || all_state(items, OperationState::TrashPending)
        || all_state(items, OperationState::TrashApplied)
        || all_state(items, OperationState::TrashOutcomeUnknown)
}

fn root_is_write_locked(deps: &ScanDeps, root: LibraryRootId) -> Result<bool, Error> {
    Ok(deps
        .roots
        .by_id(root)?
        .is_some_and(|record| record.write_safety_locked()))
}

fn now_ms(deps: &ScanDeps) -> Result<i64, Error> {
    let millis = deps
        .clock
        .now_wall()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|source| {
            Error::io(
                "clock",
                std::io::Error::other(source),
                std::path::PathBuf::new(),
            )
        })?
        .as_millis();
    Ok(i64::try_from(millis).unwrap_or(i64::MAX))
}

pub(crate) fn persist_state(
    deps: &ScanDeps,
    operation: OperationId,
    items: &[OperationItem],
    state: OperationState,
) -> Result<(), Error> {
    let next: Vec<OperationItem> = items
        .iter()
        .map(|item| OperationItem {
            state,
            ..item.clone()
        })
        .collect();
    deps.uow.with_tx(Box::new(move |tx: &mut dyn TxAccess| {
        for item in next {
            tx.upsert_operation_item(operation, item)?;
        }
        Ok(())
    }))
}

/// The result of inspecting an operation's staged data. A known missing or
/// mismatched file does not itself make the library unreadable; an I/O or
/// unavailable failure is conservatively treated as a root outage.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum StagingEvidence {
    Intact,
    MissingOrMismatched,
    RootUnavailable,
}

pub(crate) fn staging_evidence(
    deps: &ScanDeps,
    root: LibraryRootId,
    items: &[OperationItem],
) -> StagingEvidence {
    for item in items {
        let Some(path) = item.staging_path.as_ref() else {
            return StagingEvidence::MissingOrMismatched;
        };
        match deps.fs.path_exists(root, path) {
            Ok(true) => match deps.hasher.hash(root, path) {
                Ok(hash) if hash == item.expected_hash => {}
                Err(error) if root_access_error(&error) => return StagingEvidence::RootUnavailable,
                Ok(_) | Err(_) => return StagingEvidence::MissingOrMismatched,
            },
            Err(error) if root_access_error(&error) => return StagingEvidence::RootUnavailable,
            Ok(false) | Err(_) => return StagingEvidence::MissingOrMismatched,
        }
    }
    StagingEvidence::Intact
}

fn root_access_error(error: &Error) -> bool {
    matches!(error.code(), "io" | "unavailable")
        || matches!(error, Error::Storage { what, .. } if matches!(what.as_str(), "io" | "unavailable"))
}

/// Persist the safe failure state for an operation with an unprovable result.
/// The root is durably write-isolated; a root-access failure additionally
/// persists it as unavailable instead of falsely advertising it as readable.
pub(crate) fn mark_outcome_unknown(
    deps: &ScanDeps,
    root: LibraryRootId,
    operation: OperationId,
    items: &[OperationItem],
    root_unavailable: bool,
) -> Result<ForwardResult, Error> {
    let unknown: Vec<OperationItem> = items
        .iter()
        .filter(|item| item.state != OperationState::TrashOutcomeUnknown)
        .map(|item| OperationItem {
            state: OperationState::TrashOutcomeUnknown,
            ..item.clone()
        })
        .collect();
    let available = !root_unavailable
        && deps.roots.by_id(root)?.map_or(true, |record| {
            record.availability() == crate::domain::entities::RootAvailability::Available
        });
    deps.uow.with_tx(Box::new(move |tx: &mut dyn TxAccess| {
        if !unknown.is_empty() {
            for item in unknown {
                tx.upsert_operation_item(operation, item)?;
            }
        }
        tx.isolate_root_writes(root, available)
    }))?;
    Ok(ForwardResult::OutcomeUnknown)
}

/// Finalize only an operation whose every resource has a persisted
/// `TrashApplied` proof. This is also used by startup recovery after a crash
/// during database cleanup; it intentionally has no filesystem inference.
pub(crate) fn finalize_persisted_trash(
    deps: &ScanDeps,
    operation: OperationId,
    items: &[OperationItem],
) -> Result<(), Error> {
    if !all_state(items, OperationState::TrashApplied) {
        return Err(Error::InvariantViolation {
            why: "database finalization requires every item to persist TrashApplied".to_owned(),
        });
    }
    let song = subject_song(items)?;
    let stored = deps
        .songs
        .by_id(song)?
        .ok_or_else(|| Error::InvariantViolation {
            why: "trash-applied delete has no song record to finalize".to_owned(),
        })?;
    if stored.availability() != SongAvailability::PendingDelete {
        return Err(Error::InvariantViolation {
            why: "trash-applied delete song is not pending delete".to_owned(),
        });
    }
    let finalized: Vec<OperationItem> = items
        .iter()
        .map(|item| OperationItem {
            state: OperationState::DatabaseFinalized,
            ..item.clone()
        })
        .collect();
    deps.uow.with_tx(Box::new(move |tx: &mut dyn TxAccess| {
        for item in finalized {
            tx.upsert_operation_item(operation, item)?;
        }
        tx.delete_song(song)?;
        tx.release_operation_claims(operation)
    }))
}

fn subject_song(items: &[OperationItem]) -> Result<SongId, Error> {
    let song =
        items
            .first()
            .and_then(|item| item.song)
            .ok_or_else(|| Error::InvariantViolation {
                why: "delete operation item without a subject SongId".to_owned(),
            })?;
    if items.iter().all(|item| item.song == Some(song)) {
        Ok(song)
    } else {
        Err(Error::InvariantViolation {
            why: "delete operation items disagree on the subject song".to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::delete::DeleteSongs;
    use crate::application::ports::{
        LibraryRepository, OperationJournalRepository, PlaylistRepository, SongRepository,
        SystemTrashPort,
    };
    use crate::application::recover::RecoverOperations;
    use crate::application::testing::{FakeLibraryFileSystem, FakeTrash, ScanFixture};
    use crate::domain::entities::{LibraryRoot, Song};
    use crate::domain::ids::{PlaylistId, RelativeMediaPath, Revision, SongId};
    use crate::domain::media::AudioFormat;

    fn seed_delete(fixture: &ScanFixture) -> (SongId, PlaylistId) {
        let root_path = fixture.fs.root_path(fixture.root).expect("fixture root");
        LibraryRepository::upsert(
            &fixture.database,
            &LibraryRoot::new(fixture.root, root_path, true, true),
        )
        .expect("persist root");
        fixture.write_file("music/delete.flac", b"delete-bytes");
        fixture.set_audio("music/delete.flac", "Delete", 120_000);
        let mut song = Song::new(
            SongId::new(),
            fixture.root,
            fixture.path("music/delete.flac"),
            Revision::INITIAL,
        );
        song.apply_scan_facts(
            fixture.deps.hasher.hash_of_bytes(b"delete-bytes"),
            12,
            1,
            AudioFormat::Flac,
        );
        song.set_favorite(true);
        song.record_play();
        SongRepository::upsert(&fixture.database, &song).expect("song");
        let playlist = PlaylistId::new();
        fixture
            .database
            .create(playlist, fixture.root, "keep")
            .unwrap();
        fixture.database.add_member(playlist, song.id(), 3).unwrap();
        (song.id(), playlist)
    }

    fn expired_pending(fixture: &ScanFixture, song: SongId) -> OperationId {
        let operation = DeleteSongs::new(&fixture.deps)
            .delete(fixture.root, song)
            .expect("stage delete")
            .operation;
        fixture.clock.advance_ms(10_001);
        let report = RecoverOperations::new(&fixture.deps)
            .run(fixture.root)
            .expect("advance pending");
        assert!(report.touched[0].handed_off);
        operation
    }

    fn staged_file(fixture: &ScanFixture, operation: OperationId) -> std::path::PathBuf {
        fixture
            .fs
            .root_path(fixture.root)
            .expect("fixture root")
            .join(crate::domain::library::STAGING_ROOT)
            .join("trash")
            .join(operation.as_uuid().simple().to_string())
            .join("audio")
    }

    struct RemoveThenFailTrash {
        root: std::path::PathBuf,
        calls: std::sync::Mutex<Vec<OperationId>>,
    }

    impl SystemTrashPort for RemoveThenFailTrash {
        fn send_to_trash(&self, _root: LibraryRootId, operation: OperationId) -> Result<(), Error> {
            self.calls.lock().unwrap().push(operation);
            std::fs::remove_file(
                self.root
                    .join(crate::domain::library::STAGING_ROOT)
                    .join("trash")
                    .join(operation.as_uuid().simple().to_string())
                    .join("audio"),
            )
            .expect("simulate a platform move before its error");
            Err(Error::unavailable(
                "system trash",
                "simulated post-call failure",
            ))
        }
    }

    struct UnplugThenFailTrash {
        fs: FakeLibraryFileSystem,
    }

    impl SystemTrashPort for UnplugThenFailTrash {
        fn send_to_trash(
            &self,
            _root: LibraryRootId,
            _operation: OperationId,
        ) -> Result<(), Error> {
            self.fs.inject_fault(Error::unavailable(
                "library",
                "simulated unplug during trash",
            ));
            Err(Error::unavailable(
                "system trash",
                "simulated post-call failure",
            ))
        }
    }

    #[test]
    fn persisted_trash_applied_is_the_only_automatic_database_finalization_proof() {
        let fixture = ScanFixture::new();
        let (song, playlist) = seed_delete(&fixture);
        let operation = expired_pending(&fixture, song);

        // A retryable intent alone must not remove any relationship.
        RecoverOperations::new(&fixture.deps)
            .run(fixture.root)
            .unwrap();
        assert!(SongRepository::by_id(&fixture.database, song)
            .unwrap()
            .is_some());
        assert_eq!(fixture.database.members(playlist).unwrap().len(), 1);

        let items = fixture.database.items(operation).unwrap();
        persist_state(
            &fixture.deps,
            operation,
            &items,
            OperationState::TrashApplied,
        )
        .unwrap();
        RecoverOperations::new(&fixture.deps)
            .run(fixture.root)
            .unwrap();
        assert!(SongRepository::by_id(&fixture.database, song)
            .unwrap()
            .is_none());
        assert!(fixture.database.members(playlist).unwrap().is_empty());
        assert!(fixture
            .database
            .items(operation)
            .unwrap()
            .iter()
            .all(|item| item.state == OperationState::DatabaseFinalized));
    }

    #[test]
    fn explicit_system_trash_success_persists_applied_then_finalizes() {
        let fixture = ScanFixture::new();
        let (song, playlist) = seed_delete(&fixture);
        let operation = expired_pending(&fixture, song);
        let trash = FakeTrash::new();

        let report = FinalizeExpiredDeletes::new(&fixture.deps, &trash)
            .run(fixture.root)
            .unwrap();
        assert_eq!(report.finalized, vec![operation]);
        assert_eq!(trash.calls(), vec![operation]);
        assert!(SongRepository::by_id(&fixture.database, song)
            .unwrap()
            .is_none());
        assert!(fixture.database.members(playlist).unwrap().is_empty());
        assert!(fixture
            .database
            .items(operation)
            .unwrap()
            .iter()
            .all(|item| item.state == OperationState::DatabaseFinalized));
    }

    #[test]
    fn external_staging_cleanup_becomes_unknown_and_preserves_relationships() {
        let fixture = ScanFixture::new();
        let (song, playlist) = seed_delete(&fixture);
        let operation = expired_pending(&fixture, song);
        std::fs::remove_file(staged_file(&fixture, operation)).expect("external cleanup");

        RecoverOperations::new(&fixture.deps)
            .run(fixture.root)
            .unwrap();
        let retained = SongRepository::by_id(&fixture.database, song)
            .unwrap()
            .expect("song retained");
        assert!(retained.favorite());
        assert_eq!(retained.play_count().as_u64(), 1);
        assert_eq!(fixture.database.members(playlist).unwrap().len(), 1);
        assert!(fixture
            .database
            .items(operation)
            .unwrap()
            .iter()
            .all(|item| item.state == OperationState::TrashOutcomeUnknown));
        assert!(!LibraryRepository::by_id(&fixture.database, fixture.root)
            .unwrap()
            .expect("root")
            .write_capable());
    }

    #[test]
    fn disconnected_volume_becomes_unknown_and_disables_root_writes() {
        let fixture = ScanFixture::new();
        let (song, playlist) = seed_delete(&fixture);
        let operation = expired_pending(&fixture, song);
        fixture.fs.inject_fault(Error::unavailable(
            "library",
            "simulated volume unavailable",
        ));

        RecoverOperations::new(&fixture.deps)
            .run(fixture.root)
            .unwrap();
        assert!(SongRepository::by_id(&fixture.database, song)
            .unwrap()
            .is_some());
        assert_eq!(fixture.database.members(playlist).unwrap().len(), 1);
        assert!(fixture
            .database
            .items(operation)
            .unwrap()
            .iter()
            .all(|item| item.state == OperationState::TrashOutcomeUnknown));
        assert!(!LibraryRepository::by_id(&fixture.database, fixture.root)
            .unwrap()
            .expect("root")
            .write_capable());
        assert_eq!(
            LibraryRepository::by_id(&fixture.database, fixture.root)
                .unwrap()
                .expect("root")
                .availability(),
            crate::domain::entities::RootAvailability::Unavailable,
            "a disconnected root must not be advertised as readable"
        );
    }

    #[test]
    fn success_before_trash_applied_commit_recovers_as_unknown() {
        let fixture = ScanFixture::new();
        let (song, playlist) = seed_delete(&fixture);
        let operation = expired_pending(&fixture, song);
        fixture.database.set_fail_commit(true);
        let trash = FakeTrash::new();
        assert!(FinalizeExpiredDeletes::new(&fixture.deps, &trash)
            .run(fixture.root)
            .is_err());
        assert_eq!(
            trash.calls(),
            vec![operation],
            "platform call returned success"
        );
        fixture.database.set_fail_commit(false);
        // Model the successful platform move that happened before the crashed
        // `TrashApplied` commit. The temp-dir fixture never touches an OS bin.
        std::fs::remove_file(staged_file(&fixture, operation)).expect("platform moved staging");

        RecoverOperations::new(&fixture.deps)
            .run(fixture.root)
            .unwrap();
        assert!(SongRepository::by_id(&fixture.database, song)
            .unwrap()
            .is_some());
        assert_eq!(fixture.database.members(playlist).unwrap().len(), 1);
        assert!(fixture
            .database
            .items(operation)
            .unwrap()
            .iter()
            .all(|item| item.state == OperationState::TrashOutcomeUnknown));
    }

    #[test]
    fn system_trash_failure_keeps_the_verified_staging_for_retry() {
        let fixture = ScanFixture::new();
        let (song, playlist) = seed_delete(&fixture);
        let operation = expired_pending(&fixture, song);
        let trash = FakeTrash::new();
        trash.set_fails(true);

        let report = FinalizeExpiredDeletes::new(&fixture.deps, &trash)
            .run(fixture.root)
            .unwrap();
        assert_eq!(report.retryable, vec![operation]);
        assert!(staged_file(&fixture, operation).exists());
        assert!(SongRepository::by_id(&fixture.database, song)
            .unwrap()
            .is_some());
        assert_eq!(fixture.database.members(playlist).unwrap().len(), 1);
    }

    #[test]
    fn post_call_trash_error_rechecks_missing_staging_and_stops_other_operations() {
        let fixture = ScanFixture::new();
        let (song, _) = seed_delete(&fixture);
        let first = expired_pending(&fixture, song);
        let second = OperationId::new();
        let original = fixture.database.items(first).unwrap().remove(0);
        let staging = RelativeMediaPath::new(&format!(
            "{}/trash/{}/audio",
            crate::domain::library::STAGING_ROOT,
            second.as_uuid().simple()
        ))
        .expect("valid staging path");
        let staging_absolute = fixture
            .fs
            .root_path(fixture.root)
            .expect("fixture root")
            .join(staging.display());
        std::fs::create_dir_all(staging_absolute.parent().expect("staging parent"))
            .expect("create staging parent");
        std::fs::write(&staging_absolute, b"delete-bytes").expect("write staged audio");
        OperationJournalRepository::ensure_operation(
            &fixture.database,
            second,
            fixture.root,
            DELETE_OPERATION,
            Some(song),
        )
        .unwrap();
        OperationJournalRepository::upsert_item(
            &fixture.database,
            second,
            OperationItem {
                staging_path: Some(staging),
                ..original
            },
        )
        .unwrap();
        let trash = RemoveThenFailTrash {
            root: fixture.fs.root_path(fixture.root).unwrap(),
            calls: std::sync::Mutex::new(Vec::new()),
        };

        let report = FinalizeExpiredDeletes::new(&fixture.deps, &trash)
            .run(fixture.root)
            .unwrap();
        assert_eq!(report.outcome_unknown.len(), 1);
        assert_eq!(trash.calls.lock().unwrap().len(), 1);
        assert!(!LibraryRepository::by_id(&fixture.database, fixture.root)
            .unwrap()
            .unwrap()
            .write_capable());
        let untouched = if report.outcome_unknown[0] == first {
            second
        } else {
            first
        };
        assert!(fixture
            .database
            .items(untouched)
            .unwrap()
            .iter()
            .all(|item| item.state == OperationState::TrashPending));
    }

    #[test]
    fn post_call_trash_error_rechecks_unavailable_staging_as_unknown() {
        let fixture = ScanFixture::new();
        let (song, _) = seed_delete(&fixture);
        let operation = expired_pending(&fixture, song);
        let trash = UnplugThenFailTrash {
            fs: fixture.fs.clone(),
        };

        let report = FinalizeExpiredDeletes::new(&fixture.deps, &trash)
            .run(fixture.root)
            .unwrap();
        assert_eq!(report.outcome_unknown, vec![operation]);
        assert_eq!(
            LibraryRepository::by_id(&fixture.database, fixture.root)
                .unwrap()
                .unwrap()
                .availability(),
            crate::domain::entities::RootAvailability::Unavailable
        );
    }

    #[test]
    fn pre_trash_delete_states_are_skipped_without_calling_or_locking_the_root() {
        let fixture = ScanFixture::new();
        let (song, _) = seed_delete(&fixture);
        let operation = expired_pending(&fixture, song);
        let trash = FakeTrash::new();

        for state in [
            OperationState::StagePending,
            OperationState::StageApplied,
            OperationState::RestorePending,
            OperationState::FailedRecoverable,
        ] {
            let items = fixture.database.items(operation).unwrap();
            persist_state(&fixture.deps, operation, &items, state).unwrap();
            let report = FinalizeExpiredDeletes::new(&fixture.deps, &trash)
                .run(fixture.root)
                .unwrap();
            assert_eq!(report, TrashFinalizationReport::default());
            assert!(fixture
                .database
                .items(operation)
                .unwrap()
                .iter()
                .all(|item| item.state == state));
            assert!(LibraryRepository::by_id(&fixture.database, fixture.root)
                .unwrap()
                .expect("root")
                .write_capable());
        }
        assert!(
            trash.calls().is_empty(),
            "no system-trash call before handoff"
        );
    }

    #[test]
    fn unknown_state_and_root_isolation_rollback_together_when_lock_write_fails() {
        let fixture = ScanFixture::new();
        let (song, _) = seed_delete(&fixture);
        let operation = expired_pending(&fixture, song);
        std::fs::remove_file(staged_file(&fixture, operation)).expect("external cleanup");
        fixture.database.set_fail_root_isolation(true);

        assert!(
            FinalizeExpiredDeletes::new(&fixture.deps, &FakeTrash::new())
                .run(fixture.root)
                .is_err()
        );

        assert!(fixture
            .database
            .items(operation)
            .unwrap()
            .iter()
            .all(|item| item.state == OperationState::TrashPending));
        assert!(LibraryRepository::by_id(&fixture.database, fixture.root)
            .unwrap()
            .expect("root")
            .write_capable());
    }
}
