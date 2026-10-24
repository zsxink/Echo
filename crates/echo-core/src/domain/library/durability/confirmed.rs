//! Complete confirmation evidence; validating it does not authorize a session.
use super::{state::encode, valid_digest, StateKey, StateNamespace, StateTarget};
use crate::domain::sync::{DeviceId, MutationCandidate, MutationId, SyncBinding};
use crate::error::Error;
use serde::Deserialize;
use serde_json::Value;

#[derive(Deserialize)]
struct Confirmation {
    author: DeviceId,
    mutation_id: MutationId,
    version: u64,
    sequence: u64,
    payload: Value,
    tombstone: bool,
    changed_at_ms: u64,
    original_body: String,
    previous_digest: String,
    digest: String,
}

#[derive(Deserialize)]
struct CommitBody {
    protocol_version: u16,
    binding: SyncBinding,
    commit_seq: u64,
    previous_digest: [u8; 32],
    mutation: MutationCandidate,
}

pub(super) fn validate(key: &StateKey, payload: &Value) -> Result<(), Error> {
    let fact: Confirmation = serde_json::from_value(payload.clone()).map_err(encode)?;
    let body: CommitBody = serde_json::from_str(&fact.original_body).map_err(encode)?;
    let raw: Value = serde_json::from_str(&fact.original_body).map_err(encode)?;
    let StateNamespace::Bound { binding } = key.namespace else {
        return Err(Error::conflict(
            "complete confirmation requires its original binding",
        ));
    };
    let tombstone = body.mutation.payload == serde_json::json!({"tombstone":true});
    let previous = blake3::Hash::from_bytes(body.previous_digest)
        .to_hex()
        .to_string();
    let length = u32::try_from(fact.original_body.len())
        .map_err(|_| Error::conflict("confirmation body is too large"))?;
    let mut hash = blake3::Hasher::new();
    hash.update(b"echo.lan.commit.v1\0");
    hash.update(&1u16.to_be_bytes());
    hash.update(&fact.sequence.to_be_bytes());
    hash.update(&body.previous_digest);
    hash.update(&length.to_be_bytes());
    hash.update(fact.original_body.as_bytes());
    if key.target
        != (StateTarget::Confirmed {
            object: body.mutation.object,
        })
        || body.protocol_version != 1
        || body.binding != binding
        || raw
            .pointer("/mutation/protocol_version")
            .and_then(Value::as_u64)
            != Some(1)
        || raw.pointer("/mutation/binding") != Some(&serde_json::to_value(binding).map_err(encode)?)
        || fact.sequence == 0
        || body.commit_seq != fact.sequence
        || fact.version != fact.sequence
        || fact.author != body.mutation.origin_device_id
        || fact.mutation_id != body.mutation.mutation_id
        || fact.payload != body.mutation.payload
        || fact.tombstone != tombstone
        || !valid_digest(&fact.previous_digest)
        || fact.previous_digest != previous
        || !valid_digest(&fact.digest)
        || hash.finalize().to_hex().as_str() != fact.digest
    {
        return Err(Error::conflict("invalid complete confirmation evidence"));
    }
    // This is the recorded SQL baseline time, not a reconstructed ledger clock.
    let _ = fact.changed_at_ms;
    Ok(())
}
