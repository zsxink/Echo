//! Host confirmation orchestration and publication recovery.

use crate::application::ports::{
    HostCommitLedgerPort, PublicationCheckpointPort, StagedConfirmation,
};
use crate::domain::sync::{MutationCandidate, SyncBinding};
use crate::error::Error;

/// Confirms a single incoming mutation and returns only after its published
/// checkpoint is durable. Network handlers call this without holding a SQL
/// transaction or platform lock.
pub struct ConfirmHostMutation<'a> {
    ledger: &'a dyn HostCommitLedgerPort,
    checkpoint: &'a dyn PublicationCheckpointPort,
}

impl<'a> ConfirmHostMutation<'a> {
    #[must_use]
    pub const fn new(
        ledger: &'a dyn HostCommitLedgerPort,
        checkpoint: &'a dyn PublicationCheckpointPort,
    ) -> Self {
        Self { ledger, checkpoint }
    }

    /// Confirm the candidate and durably publish its checkpoint.
    ///
    /// # Errors
    /// Propagates ledger validation, storage, or publication failures.
    pub fn execute(
        &self,
        binding: SyncBinding,
        candidate: &MutationCandidate,
    ) -> Result<StagedConfirmation, Error> {
        self.execute_at(binding, candidate, None)
    }

    /// Confirm a mutation while retaining its immutable local edit timestamp.
    ///
    /// # Errors
    /// Propagates ledger validation, storage, and publication failures.
    pub fn execute_at(
        &self,
        binding: SyncBinding,
        candidate: &MutationCandidate,
        changed_at_ms: Option<u64>,
    ) -> Result<StagedConfirmation, Error> {
        let staged = self
            .ledger
            .stage_confirmation_at(binding, candidate, changed_at_ms)?;
        self.finish_publication(&staged)?;
        Ok(staged)
    }

    /// Repair every pending publication in commit order after startup. A
    /// checkpoint already advanced before a crash is safe to write again.
    ///
    /// # Errors
    /// Propagates failures reading pending commits or publishing checkpoints.
    pub fn recover_publications(&self, limit: usize) -> Result<usize, Error> {
        let pending = self.ledger.pending_publications(limit)?;
        let mut published = 0;
        for staged in &pending {
            self.finish_publication(staged)?;
            if staged.checkpoint.is_some() {
                published += 1;
            }
        }
        Ok(published)
    }

    fn finish_publication(&self, staged: &StagedConfirmation) -> Result<(), Error> {
        if let Some(checkpoint) = staged.checkpoint {
            self.checkpoint.advance(checkpoint)?;
            self.ledger.mark_published(checkpoint)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use super::*;
    use crate::application::ports::{HostCommitLedgerPort, PublicationCheckpointPort};
    use crate::domain::ids::SongId;
    use crate::domain::sync::{
        AuthorityId, CommitCheckpoint, CommitSeq, DeviceId, Epoch, LibraryId, MutationCandidate,
        MutationId, ObjectKey,
    };

    #[derive(Default)]
    struct FakeLedger {
        staged: Mutex<BTreeMap<MutationId, StagedConfirmation>>,
        published: Mutex<Vec<CommitCheckpoint>>,
        fail_mark: Mutex<bool>,
    }

    impl HostCommitLedgerPort for FakeLedger {
        fn stage_confirmation(
            &self,
            binding: SyncBinding,
            candidate: &MutationCandidate,
        ) -> Result<StagedConfirmation, Error> {
            let mut staged = self.staged.lock().unwrap();
            if let Some(existing) = staged.get(&candidate.mutation_id) {
                return Ok(existing.clone());
            }
            let receipt = StagedConfirmation {
                mutation_id: candidate.mutation_id,
                outcome_json: "{\"accepted\":true}".to_owned(),
                checkpoint: Some(CommitCheckpoint {
                    binding,
                    sequence: CommitSeq::from_u64(1),
                    digest: [9; 32],
                }),
            };
            staged.insert(candidate.mutation_id, receipt.clone());
            drop(staged);
            Ok(receipt)
        }
        fn mark_published(&self, checkpoint: CommitCheckpoint) -> Result<(), Error> {
            let mut fail = self.fail_mark.lock().unwrap();
            if *fail {
                *fail = false;
                drop(fail);
                return Err(Error::unavailable("ledger", "injected publish failure"));
            }
            drop(fail);
            self.published.lock().unwrap().push(checkpoint);
            Ok(())
        }
        fn pending_publications(&self, _limit: usize) -> Result<Vec<StagedConfirmation>, Error> {
            Ok(self.staged.lock().unwrap().values().cloned().collect())
        }
        fn published_commits_after(
            &self,
            _binding: SyncBinding,
            _after: CommitSeq,
            _expected_digest: [u8; 32],
            _limit: usize,
        ) -> Result<Vec<crate::application::ports::PublishedCommit>, Error> {
            Ok(Vec::new())
        }
    }

    #[derive(Default)]
    struct FakeCheckpoint {
        values: Mutex<Vec<CommitCheckpoint>>,
        fail: Mutex<bool>,
    }
    impl PublicationCheckpointPort for FakeCheckpoint {
        fn advance(&self, checkpoint: CommitCheckpoint) -> Result<(), Error> {
            let mut fail = self.fail.lock().unwrap();
            if *fail {
                *fail = false;
                drop(fail);
                return Err(Error::unavailable(
                    "checkpoint",
                    "injected checkpoint failure",
                ));
            }
            drop(fail);
            self.values.lock().unwrap().push(checkpoint);
            Ok(())
        }
    }

    fn values() -> (SyncBinding, MutationCandidate) {
        let binding = SyncBinding {
            library_id: LibraryId::new(),
            authority_id: AuthorityId::new(),
            epoch: Epoch::INITIAL,
        };
        let candidate = MutationCandidate {
            resource_refs: Vec::new(),
            mutation_id: MutationId::new(),
            origin_device_id: DeviceId::new(),
            origin_seq: 1,
            previous_mutation_id: None,
            object: ObjectKey::Song(SongId::new()),
            base_object_version: None,
            payload: serde_json::json!({"title":"x"}),
        };
        (binding, candidate)
    }

    #[test]
    fn checkpoint_failure_leaves_staged_commit_for_retry() {
        let ledger = FakeLedger::default();
        let checkpoint = FakeCheckpoint::default();
        *checkpoint.fail.lock().unwrap() = true;
        let use_case = ConfirmHostMutation::new(&ledger, &checkpoint);
        let (binding, candidate) = values();
        assert!(use_case.execute(binding, &candidate).is_err());
        assert!(ledger.published.lock().unwrap().is_empty());
        assert_eq!(use_case.recover_publications(10).unwrap(), 1);
        assert_eq!(ledger.published.lock().unwrap().len(), 1);
    }

    #[test]
    fn crash_after_checkpoint_before_published_is_idempotently_repaired() {
        let ledger = FakeLedger::default();
        let checkpoint = FakeCheckpoint::default();
        *ledger.fail_mark.lock().unwrap() = true;
        let use_case = ConfirmHostMutation::new(&ledger, &checkpoint);
        let (binding, candidate) = values();
        assert!(use_case.execute(binding, &candidate).is_err());
        assert_eq!(checkpoint.values.lock().unwrap().len(), 1);
        assert_eq!(use_case.recover_publications(10).unwrap(), 1);
        assert_eq!(checkpoint.values.lock().unwrap().len(), 2);
        assert_eq!(ledger.published.lock().unwrap().len(), 1);
    }
}
