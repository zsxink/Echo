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

fn outcome(
    accepted: bool,
    reason: &str,
    seq: Option<u64>,
    last: Option<MutationId>,
) -> MutationOutcome {
    MutationOutcome {
        accepted,
        reason: Some(reason.to_owned()),
        origin_seq: seq,
        last_mutation_id: last,
    }
}

fn candidate(seq: u64, previous: Option<MutationId>) -> MutationCandidate {
    MutationCandidate {
        resource_refs: Vec::new(),
        mutation_id: MutationId::new(),
        origin_device_id: DeviceId::new(),
        origin_seq: seq,
        previous_mutation_id: previous,
        object: echo_core::domain::sync::ObjectKey::Song(echo_core::domain::ids::SongId::new()),
        base_object_version: None,
        payload: serde_json::json!({"title": "x"}),
    }
}

/// A chain that already continues the host's position must never be renumbered:
/// the host is simply waiting for an event behind this one.
#[test]
fn rejection_that_matches_the_local_chain_does_not_re_anchor() {
    let previous = MutationId::new();
    let outcome = outcome(false, "OutOfOrderPredecessor", Some(4), Some(previous));
    assert_eq!(
        reanchor_target(&candidate(5, Some(previous)), &outcome),
        None
    );
}

/// A host position the local head does not continue means the sequence space
/// itself diverged, which is what the replica re-anchors onto.
#[test]
fn rejection_from_a_foreign_chain_re_anchors_onto_the_host_position() {
    let host_last = MutationId::new();
    let divergent = outcome(false, "OutOfOrderPredecessor", Some(57), Some(host_last));
    assert_eq!(
        reanchor_target(&candidate(1, None), &divergent),
        Some(OriginAnchor {
            origin_seq: 57,
            last_mutation_id: Some(host_last),
        }),
        "a locally invented chain restarts from the host's confirmed position"
    );
    let same_seq_wrong_predecessor = outcome(false, "OutOfOrderPredecessor", Some(0), None);
    assert_eq!(
        reanchor_target(
            &candidate(1, Some(MutationId::new())),
            &same_seq_wrong_predecessor
        ),
        Some(OriginAnchor::GENESIS)
    );
}

/// An unparseable or legacy rejection names no position, so it stays a plain
/// retryable stop instead of renumbering the queue on a guess.
#[test]
fn rejection_without_a_host_position_never_re_anchors() {
    let legacy = outcome(false, "OutOfOrderPredecessor", None, None);
    assert_eq!(reanchor_target(&candidate(9, None), &legacy), None);
    let other = outcome(false, "WrongObject", Some(3), Some(MutationId::new()));
    assert_eq!(reanchor_target(&candidate(9, None), &other), None);
}

/// The re-anchor position crosses a crate boundary as JSON: `echo-core` writes
/// the rejection body and this adapter parses it. Every other test here builds
/// the struct directly, so a renamed field or a UUID that does not round-trip
/// would silently degrade to "the host named no position" — the replica would
/// keep retrying a chain the host can never accept, which is the exact stall
/// this repair exists to end.
#[test]
fn host_rejection_body_parses_into_a_re_anchor_position() {
    let host_last = MutationId::new();
    let body = format!(
        r#"{{"accepted":false,"reason":"OutOfOrderPredecessor","origin_seq":57,"last_mutation_id":"{}"}}"#,
        host_last.as_uuid()
    );
    let parsed: MutationOutcome = serde_json::from_str(&body).expect("host rejection body");
    assert_eq!(parsed.origin_seq, Some(57));
    assert_eq!(
        parsed.last_mutation_id,
        Some(host_last),
        "the predecessor UUID must survive the wire encoding"
    );
    assert_eq!(
        reanchor_target(&candidate(1, None), &parsed),
        Some(OriginAnchor {
            origin_seq: 57,
            last_mutation_id: Some(host_last),
        })
    );

    // A body from an older host names no position. It must still decode: that
    // is the "no repair is possible from this rejection" branch, not a decode
    // error that would replace the real diagnosis with a transport failure.
    let legacy: MutationOutcome =
        serde_json::from_str(r#"{"accepted":false,"reason":"OutOfOrderPredecessor"}"#)
            .expect("legacy body still decodes");
    assert_eq!(legacy.origin_seq, None);
    assert_eq!(reanchor_target(&candidate(1, None), &legacy), None);

    // Field-name drift is precisely what this test guards against. A camelCase
    // key is what a serialization-level rename produces, and it must not be
    // mistaken for the host naming genesis (which would renumber the chain).
    let drifted: MutationOutcome = serde_json::from_str(&format!(
        r#"{{"accepted":false,"reason":"OutOfOrderPredecessor","originSeq":57,"lastMutationId":"{}"}}"#,
        host_last.as_uuid()
    ))
    .expect("unknown keys are ignored");
    assert_eq!(
        drifted.origin_seq, None,
        "the adapter must read the key `echo-core` actually writes"
    );
    assert_eq!(reanchor_target(&candidate(1, None), &drifted), None);
}
