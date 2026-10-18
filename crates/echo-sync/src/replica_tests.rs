use super::*;
use echo_core::domain::sync::{CommitCheckpoint, MutationId};
use std::sync::Mutex;

/// A queue that records the order of the two calls the push batch depends on
/// and refuses to go further, so no transport is ever involved.
#[derive(Default)]
struct RecordingQueue {
    calls: Mutex<Vec<&'static str>>,
}

impl LocalSyncQueuePort for RecordingQueue {
    fn advance_rejected_local_mutations(&self) -> Result<usize, Error> {
        self.calls
            .lock()
            .expect("call log")
            .push("advance_rejected_local_mutations");
        Ok(1)
    }

    fn pending_local_mutations(&self, _limit: usize) -> Result<Vec<MutationCandidate>, Error> {
        self.calls
            .lock()
            .expect("call log")
            .push("pending_local_mutations");
        Err(Error::conflict("stop the cycle before any transport"))
    }

    fn reject_local_mutation(
        &self,
        _mutation_id: MutationId,
        _outcome_json: String,
    ) -> Result<(), Error> {
        Err(Error::conflict("unused"))
    }

    fn replica_checkpoint(&self, _host_device_id: DeviceId) -> Result<CommitCheckpoint, Error> {
        Err(Error::conflict("unused"))
    }
}

/// A rejected event whose predecessor was still outstanding leaves the origin
/// cursor one step behind. Reading the batch first would hand the host events
/// it considers out of order, and every later cycle would fail at the same
/// event — the replica would never sync again. The repair must come first.
#[test]
fn push_batch_repairs_a_rejected_cursor_gap_before_reading() {
    let queue = RecordingQueue::default();
    assert!(next_push_batch(&queue, 8).is_err());
    assert_eq!(
        *queue.calls.lock().expect("call log"),
        [
            "advance_rejected_local_mutations",
            "pending_local_mutations"
        ],
        "the cursor must be repaired before the batch is selected"
    );
}
