//! Complete versioned receipt evidence, independent of live-session authority.
use super::{state::encode, valid_digest, StateKey, StateNamespace, StateTarget};
use crate::domain::sync::{CommitCheckpoint, DeviceId, MutationId};
use crate::error::Error;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
struct Receipt {
    author: DeviceId,
    mutation_id: MutationId,
    payload_digest: String,
    outcome: Confirmation,
    original_outcome: String,
    sequence: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Confirmation {
    mutation_id: MutationId,
    outcome_json: String,
    checkpoint: Option<CommitCheckpoint>,
}

pub(super) fn validate(key: &StateKey, payload: &Value) -> Result<(), Error> {
    if payload.get("sequence").is_none()
        || payload
            .get("outcome")
            .and_then(|outcome| outcome.get("checkpoint"))
            .is_none()
    {
        return Err(Error::conflict(
            "complete receipt requires explicit sequence and checkpoint evidence",
        ));
    }
    let receipt: Receipt = serde_json::from_value(payload.clone()).map_err(encode)?;
    let original: Value = serde_json::from_str(&receipt.original_outcome).map_err(encode)?;
    let outcome: Value = serde_json::from_str(&receipt.outcome.outcome_json).map_err(encode)?;
    let accepted = outcome.get("accepted").and_then(Value::as_bool);
    let checkpoint_valid = match (accepted, receipt.sequence, receipt.outcome.checkpoint) {
        (Some(false), None, None) => true,
        (Some(true), Some(sequence), Some(checkpoint)) => {
            sequence > 0
                && checkpoint.sequence.as_u64() == sequence
                && matches!(key.namespace, StateNamespace::Bound { binding } if binding == checkpoint.binding)
        }
        _ => false,
    };
    // Zero is a valid historical timestamp, but absence is not evidence.
    if payload.get("received_at").and_then(Value::as_u64).is_none() {
        return Err(Error::conflict(
            "complete receipt requires its recorded timestamp",
        ));
    }
    if key.target
        != (StateTarget::Receipt {
            author: receipt.author,
            mutation_id: receipt.mutation_id,
        })
        || receipt.outcome.mutation_id != receipt.mutation_id
        || !valid_digest(&receipt.payload_digest)
        || original != payload["outcome"]
        || !checkpoint_valid
    {
        return Err(Error::conflict("invalid complete receipt evidence"));
    }
    Ok(())
}
