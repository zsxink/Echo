//! Scriptable port-boundary faults, independent from SQLite and filesystem IO.
use std::sync::{Arc, Mutex};

use crate::application::ports::{
    LibraryAssetPort, LibraryDurabilityPort, LibraryProjectionPort, LibraryProjectionVerifiedPort,
    LibraryWriteSessionPort,
};
use crate::domain::ids::LibraryRootId;
use crate::domain::library::durability::{
    CommitOutcome, LibraryBatch, LibraryCommit, LibraryProjectionStatus, LibraryRecoveryRecords,
    LibraryReplay,
};
use crate::error::Error;

use super::small_fakes::MemoryControlPlane;

mod refusing;
pub use refusing::RefusingAuthority;

/// Faults are one-shot and preserve the trace across clones. Native fsync and
/// materialization faults belong to the filesystem adapter's lower-level fake.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DurabilityBoundary {
    WriteAsset,
    ReadAsset,
    Commit,
    Recover,
    ReadOperation,
    OperationStatus,
}

#[derive(Clone, Debug, Default)]
pub struct FaultLibraryDurability {
    pub directory: MemoryControlPlane,
    failure: Arc<Mutex<Option<DurabilityBoundary>>>,
    calls: Arc<Mutex<Vec<DurabilityBoundary>>>,
}

impl FaultLibraryDurability {
    pub fn new() -> Self {
        Self {
            directory: MemoryControlPlane::new(),
            ..Self::default()
        }
    }
    pub fn fail_next(&self, boundary: DurabilityBoundary) {
        *self.failure.lock().unwrap() = Some(boundary);
    }
    pub fn calls(&self) -> Vec<DurabilityBoundary> {
        self.calls.lock().unwrap().clone()
    }
    fn enter(&self, boundary: DurabilityBoundary) -> Result<(), Error> {
        self.calls.lock().unwrap().push(boundary);
        let mut failure = self.failure.lock().unwrap();
        if *failure == Some(boundary) {
            *failure = None;
            return Err(Error::unavailable(
                "library durability fake",
                "scripted boundary failure",
            ));
        }
        Ok(())
    }
}

impl LibraryAssetPort for FaultLibraryDurability {
    fn write_asset(&self, root: LibraryRootId, bytes: &[u8]) -> Result<String, Error> {
        self.enter(DurabilityBoundary::WriteAsset)?;
        self.directory.write_asset(root, bytes)
    }
    fn read_asset(&self, root: LibraryRootId, hash: &str) -> Result<Option<Vec<u8>>, Error> {
        self.enter(DurabilityBoundary::ReadAsset)?;
        self.directory.read_asset(root, hash)
    }
}

impl LibraryWriteSessionPort for FaultLibraryDurability {}

impl LibraryDurabilityPort for FaultLibraryDurability {
    fn commit(&self, root: LibraryRootId, batch: &LibraryBatch) -> Result<CommitOutcome, Error> {
        self.enter(DurabilityBoundary::Commit)?;
        self.directory.commit(root, batch)
    }
    fn recover(&self, root: LibraryRootId) -> Result<LibraryReplay, Error> {
        self.enter(DurabilityBoundary::Recover)?;
        self.directory.recover(root)
    }
    fn read_operation(
        &self,
        root: LibraryRootId,
        id: uuid::Uuid,
    ) -> Result<Option<LibraryCommit>, Error> {
        self.enter(DurabilityBoundary::ReadOperation)?;
        self.directory.read_operation(root, id)
    }
    fn operation_status(
        &self,
        root: LibraryRootId,
        id: uuid::Uuid,
    ) -> Result<Option<CommitOutcome>, Error> {
        self.enter(DurabilityBoundary::OperationStatus)?;
        self.directory.operation_status(root, id)
    }
}

/// A projection sink, not another coordinator or business-table emulator.
/// A failed commit records the attempt but leaves the accepted replay unchanged.
#[derive(Clone, Debug, Default)]
pub struct FaultLibraryProjection {
    failure: Arc<Mutex<bool>>,
    attempts: Arc<Mutex<Vec<(LibraryRootId, LibraryReplay)>>>,
    accepted: Arc<Mutex<Vec<(LibraryRootId, LibraryReplay)>>>,
}
impl FaultLibraryProjection {
    pub fn fail_next_commit(&self) {
        *self.failure.lock().unwrap() = true;
    }
    pub fn attempts(&self) -> Vec<(LibraryRootId, LibraryReplay)> {
        self.attempts.lock().unwrap().clone()
    }
    pub fn accepted(&self) -> Vec<(LibraryRootId, LibraryReplay)> {
        self.accepted.lock().unwrap().clone()
    }
}
impl LibraryProjectionPort for FaultLibraryProjection {
    fn projection_status(&self, _: LibraryRootId) -> Result<LibraryProjectionStatus, Error> {
        Err(Error::unavailable(
            "projection status fake",
            "status is not modeled by replay sink",
        ))
    }
    fn read_recovery_records(&self, _: LibraryRootId) -> Result<LibraryRecoveryRecords, Error> {
        Err(Error::unavailable(
            "projection records fake",
            "records are not modeled by replay sink",
        ))
    }
    fn project_library_commits(
        &self,
        root: LibraryRootId,
        replay: &LibraryReplay,
    ) -> Result<(), Error> {
        self.attempts.lock().unwrap().push((root, replay.clone()));
        if std::mem::take(&mut *self.failure.lock().unwrap()) {
            return Err(Error::unavailable(
                "library projection fake",
                "scripted commit failure",
            ));
        }
        self.accepted.lock().unwrap().push((root, replay.clone()));
        Ok(())
    }
}

impl LibraryProjectionVerifiedPort for FaultLibraryProjection {
    fn project_verified_library_suffix(
        &self,
        root: LibraryRootId,
        replay: &LibraryReplay,
    ) -> Result<(), Error> {
        self.project_library_commits(root, replay)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::library::{durability::ChangeOrigin, LibraryId};

    #[test]
    fn library_durability_fault_ports_preserve_saved_evidence_on_projection_failure() {
        let directory = FaultLibraryDurability::new();
        let projection = FaultLibraryProjection::default();
        let root = LibraryRootId::new();
        let batch = LibraryBatch::new(
            uuid::Uuid::new_v4(),
            LibraryId::from_uuid(uuid::Uuid::new_v4()),
            ChangeOrigin::LocalEdit,
            vec![],
        );
        directory.fail_next(DurabilityBoundary::Commit);
        assert!(directory.commit(root, &batch).is_err());
        assert!(directory
            .operation_status(root, batch.operation_id)
            .unwrap()
            .is_none());
        let saved = directory.commit(root, &batch).unwrap();
        let replay = directory.recover(root).unwrap();
        projection.fail_next_commit();
        assert!(projection.project_library_commits(root, &replay).is_err());
        assert!(projection.accepted().is_empty());
        assert_eq!(directory.commit(root, &batch).unwrap(), saved);
        projection
            .project_library_commits(root, &directory.recover(root).unwrap())
            .unwrap();
        assert_eq!(
            projection.attempts(),
            vec![(root, replay.clone()), (root, replay.clone())]
        );
        assert_eq!(projection.accepted(), vec![(root, replay)]);
    }

    #[test]
    fn library_projection_port_defaults_reject_unsupported_operations() {
        // An adapter that implements only the required methods must fail
        // closed on every optional rebuild operation, never fall through to a
        // silent no-op.
        let projection = FaultLibraryProjection::default();
        let root = LibraryRootId::new();
        let library = LibraryId::from_uuid(uuid::Uuid::new_v4());
        let replay = LibraryReplay {
            checkpoint: None,
            commits: vec![],
        };
        let error = projection
            .validate_library_projection(root, library, &replay)
            .expect_err("default rejects validation");
        assert_eq!(error.code(), "unavailable");
        let error = projection
            .rebuild_library_projection(root, library, &replay)
            .expect_err("default rejects rebuild");
        assert_eq!(error.code(), "unavailable");
        let error = projection
            .project_legacy_records(root, &[])
            .expect_err("default rejects legacy projection");
        assert_eq!(error.code(), "unavailable");
    }

    #[test]
    fn library_durability_sqlite_uow_is_only_projection_not_directory_commit() {
        use crate::application::ports::UnitOfWork;
        let directory = FaultLibraryDurability::new();
        let root = LibraryRootId::new();
        let batch = LibraryBatch::new(
            uuid::Uuid::new_v4(),
            LibraryId::from_uuid(uuid::Uuid::new_v4()),
            ChangeOrigin::LocalEdit,
            vec![],
        );
        let projection = crate::application::testing::MemoryUnitOfWork::new();
        projection.with_tx(Box::new(|_| Ok(()))).unwrap();
        assert!(directory.recover(root).unwrap().commits.is_empty());
        let saved = directory.commit(root, &batch).unwrap();
        projection.set_fail_commit(true);
        assert!(projection.with_tx(Box::new(|_| Ok(()))).is_err());
        assert_eq!(
            directory
                .operation_status(root, batch.operation_id)
                .unwrap(),
            Some(saved)
        );
        assert_eq!(directory.recover(root).unwrap().commits[0].batch, batch);
    }

    #[test]
    fn library_durability_fault_ports_fail_each_read_recovery_and_asset_boundary_once() {
        let directory = FaultLibraryDurability::new();
        let root = LibraryRootId::new();
        let id = uuid::Uuid::new_v4();
        directory.fail_next(DurabilityBoundary::WriteAsset);
        assert!(directory.write_asset(root, b"original").is_err());
        let hash = directory.write_asset(root, b"original").unwrap();
        directory.fail_next(DurabilityBoundary::ReadAsset);
        assert!(directory.read_asset(root, &hash).is_err());
        assert_eq!(
            directory.read_asset(root, &hash).unwrap(),
            Some(b"original".to_vec())
        );
        directory.fail_next(DurabilityBoundary::Recover);
        assert!(directory.recover(root).is_err());
        assert!(directory.recover(root).unwrap().commits.is_empty());
        directory.fail_next(DurabilityBoundary::ReadOperation);
        assert!(directory.read_operation(root, id).is_err());
        assert!(directory.read_operation(root, id).unwrap().is_none());
        directory.fail_next(DurabilityBoundary::OperationStatus);
        assert!(directory.operation_status(root, id).is_err());
        assert!(directory.operation_status(root, id).unwrap().is_none());
        assert_eq!(directory.calls().len(), 10);
    }
}
