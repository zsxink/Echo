//! Explicit recovery of an existing directory operation, never a new user edit.

use crate::application::ports::{ControlPlanePort, LibraryHistoryPort};
use crate::domain::ids::{LibraryRootId, OperationId};
use crate::domain::library::durability::CommitStatus;
use crate::error::Error;

pub struct RetryLibraryOperation<'a> {
    control: &'a dyn ControlPlanePort,
    projection: &'a dyn LibraryHistoryPort,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::LibraryDurabilityPort;
    use crate::application::testing::small_fakes::MemoryControlPlane;
    use crate::domain::library::durability::{ChangeOrigin, LibraryBatch, LibraryReplay};
    use crate::domain::library::{LibraryId, PortableRecordEnvelope};
    use std::sync::Mutex;

    #[derive(Default)]
    struct Projection {
        received: Mutex<Vec<LibraryReplay>>,
        fail: bool,
    }

    impl LibraryHistoryPort for Projection {
        fn capture_library_history(
            &self,
            _: LibraryRootId,
        ) -> Result<Vec<PortableRecordEnvelope>, Error> {
            panic!("retry must replay directory facts, not rescue the current database")
        }
        fn project_library_commits(
            &self,
            _: LibraryRootId,
            replay: &LibraryReplay,
        ) -> Result<(), Error> {
            self.received.lock().unwrap().push(replay.clone());
            if self.fail {
                Err(Error::conflict("scripted projection failure"))
            } else {
                Ok(())
            }
        }
    }

    #[test]
    fn library_durability_retry_unknown_operation_never_projects() {
        let control = MemoryControlPlane::new();
        let projection = Projection::default();
        assert!(RetryLibraryOperation::new(&control, &projection)
            .execute(LibraryRootId::new(), OperationId::new())
            .is_err());
        assert!(projection.received.lock().unwrap().is_empty());
    }

    #[test]
    fn library_durability_retry_catches_up_all_saved_operations_without_new_commits() {
        let root = LibraryRootId::new();
        let library = LibraryId::from_uuid(uuid::Uuid::new_v4());
        let operation = OperationId::new();
        let control = MemoryControlPlane::new();
        for id in [uuid::Uuid::new_v4(), operation.as_uuid()] {
            control
                .commit(
                    root,
                    &LibraryBatch::new(id, library, ChangeOrigin::LocalEdit, vec![]),
                )
                .unwrap();
        }
        let projection = Projection::default();
        let retry = RetryLibraryOperation::new(&control, &projection);
        retry.execute(root, operation).unwrap();
        retry.execute(root, operation).unwrap();
        let received = projection.received.lock().unwrap();
        assert_eq!(received.len(), 2);
        assert_eq!(received[0], received[1]);
        assert_eq!(received[0].commits.len(), 2);
        assert_eq!(received[0].commits[0].library_commit_seq, 1);
        assert_eq!(
            received[0].commits[1].batch.operation_id,
            operation.as_uuid()
        );
        assert_eq!(control.recover(root).unwrap(), received[0]);
    }

    #[test]
    fn library_durability_retry_projection_failure_keeps_original_identity() {
        let root = LibraryRootId::new();
        let operation = OperationId::new();
        let control = MemoryControlPlane::new();
        control
            .commit(
                root,
                &LibraryBatch::new(
                    operation.as_uuid(),
                    LibraryId::from_uuid(uuid::Uuid::new_v4()),
                    ChangeOrigin::LocalEdit,
                    vec![],
                ),
            )
            .unwrap();
        let projection = Projection {
            fail: true,
            ..Projection::default()
        };
        let error = RetryLibraryOperation::new(&control, &projection)
            .execute(root, operation)
            .unwrap_err();
        assert!(
            matches!(error, Error::LibrarySavedPending { operation: saved, stage: crate::error::LibraryPersistenceStage::Projection, .. } if saved == operation)
        );
        assert_eq!(control.recover(root).unwrap().commits.len(), 1);
    }
}

impl<'a> RetryLibraryOperation<'a> {
    #[must_use]
    pub const fn new(
        control: &'a dyn ControlPlanePort,
        projection: &'a dyn LibraryHistoryPort,
    ) -> Self {
        Self {
            control,
            projection,
        }
    }

    /// Recover the committed operation's library and catch the projection up in order.
    ///
    /// # Errors
    /// Rejects an unknown operation, missing adapter or failed recovery/projection.
    pub fn execute(&self, root: LibraryRootId, operation: OperationId) -> Result<(), Error> {
        let durable = self.control.durability().ok_or_else(|| {
            Error::unavailable("library recovery", "directory adapter is not configured")
        })?;
        let status = durable.operation_status(root, operation.as_uuid())?;
        if status.is_none_or(|saved| saved.status == CommitStatus::NotSaved) {
            return Err(Error::conflict(
                "no saved operation exists in the selected library",
            ));
        }
        let replay = durable
            .recover(root)
            .map_err(|error| Error::saved_pending_recovery(operation, error))?;
        self.projection
            .project_library_commits(root, &replay)
            .map_err(|error| Error::saved_pending_projection(operation, error))
    }

    /// Catch up the existing saved prefix without creating a user mutation.
    /// Also works when a saved operation lives in a compacted coverage image.
    /// # Errors
    /// Reports unreadable directory evidence or a failed local projection.
    pub fn catch_up(&self, root: LibraryRootId) -> Result<(), Error> {
        let durable = self.control.durability().ok_or_else(|| {
            Error::unavailable("library recovery", "directory adapter is not configured")
        })?;
        self.projection.project_library_commits(root, &durable.recover(root)?)
    }
}
