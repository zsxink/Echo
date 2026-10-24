//! A publication-only capture requires immutable directory evidence of its base.
use super::{facts, identity};
use crate::application::ports::{LibraryDurabilityPort, RemoteCommit};
use crate::domain::{
    ids::LibraryRootId,
    library::{
        durability::{
            ChangeOrigin, CommitStatus, DurableStateChange, LibraryBatch, StateKey, StateNamespace,
        },
        LibraryId,
    },
    sync::CommitCheckpoint,
};
use crate::error::Error;
use crate::infrastructure::sqlite::support::storage;
use rusqlite::{params, Connection};
use serde_json::Value;

// The full exporter and the focused capture retain exactly the same after-image.
pub(super) const LEDGER_PAYLOAD: &str = "json_object('schema','echo.sync.commit.v1','sequence',commit_seq,'library',library_id,'authority',authority_id,'epoch',epoch,'author',origin_device_id,'mutation_id',mutation_id,'body',json(canonical_json),'original_body',canonical_json,'created_at',created_at,'previous_digest',lower(hex(previous_digest)),'digest',lower(hex(commit_digest)),'publication',publication_state)";

pub(super) fn capture(
    directory: &dyn LibraryDurabilityPort,
    connection: &Connection,
    root: LibraryRootId,
    library: LibraryId,
    checkpoint: CommitCheckpoint,
) -> Result<Option<DurableStateChange>, Error> {
    let current = current_ledger(connection, checkpoint)?;
    if verify_ledger(&current, checkpoint).is_err() {
        // Legacy or malformed evidence remains on the complete rescue path.
        return Ok(None);
    }
    let stage = identity(
        "host-confirmation",
        super::text(&current.payload, "mutation_id")?,
    );
    // NotSaved preparations are not authoritative. Missing/legacy evidence
    // requires the original complete capture, including any DB-only history.
    if directory
        .operation_status(root, stage)?
        .is_none_or(|outcome| outcome.status == CommitStatus::NotSaved)
    {
        return Ok(None);
    }
    let Some(saved) = directory.read_operation(root, stage)? else {
        return Ok(None);
    };
    if saved.batch.library_id != library || saved.batch.origin != ChangeOrigin::HostConfirmation {
        return Ok(None);
    }
    let mut matching = saved
        .batch
        .state
        .iter()
        .filter(|state| state.key == current.key);
    let Some(previous) = matching.next() else {
        return Ok(None);
    };
    if matching.next().is_some() || previous.kind != current.kind || previous.id != current.id {
        return Ok(None);
    }
    previous.validate()?;
    let mut expected = previous.clone();
    expected.payload["publication"] = current.payload["publication"].clone();
    // Exact equality covers original bytes/digest, sequence, author, resources,
    // predecessor and every other retained field, not SQL before/after equality.
    Ok((expected == current).then_some(current))
}

fn verify_ledger(state: &DurableStateChange, checkpoint: CommitCheckpoint) -> Result<(), Error> {
    let payload = &state.payload;
    let raw = super::text(payload, "original_body")?;
    let body: Value = serde_json::from_str(raw).map_err(storage)?;
    if body != payload["body"] || payload["publication"] != "published" {
        return Err(Error::conflict(
            "publication ledger differs from its original body",
        ));
    }
    let commit = RemoteCommit {
        sequence: checkpoint.sequence,
        previous_digest: *blake3::Hash::from_hex(super::text(payload, "previous_digest")?)
            .map_err(storage)?
            .as_bytes(),
        digest: checkpoint.digest,
        origin_device_id: serde_json::from_value(payload["author"].clone()).map_err(storage)?,
        mutation_id: serde_json::from_value(payload["mutation_id"].clone()).map_err(storage)?,
        object: serde_json::from_value(body["mutation"]["object"].clone()).map_err(storage)?,
        payload_json: serde_json::to_string(&body["mutation"]["payload"]).map_err(storage)?,
        canonical_json: raw.to_owned(),
        is_tombstone: body["mutation"]["payload"] == serde_json::json!({"tombstone":true}),
    };
    super::super::apply::verify_remote_commit(checkpoint.binding, &commit)
}

fn current_ledger(
    connection: &Connection,
    checkpoint: CommitCheckpoint,
) -> Result<DurableStateChange, Error> {
    let query = format!("SELECT {LEDGER_PAYLOAD} FROM sync_commits WHERE commit_seq=?1 AND library_id=?2 AND authority_id=?3 AND epoch=?4 AND commit_digest=?5");
    let raw: String = connection
        .query_row(
            &query,
            params![
                checkpoint.sequence.as_u64(),
                checkpoint.binding.library_id.as_uuid().to_string(),
                checkpoint.binding.authority_id.as_uuid().to_string(),
                checkpoint.binding.epoch.as_u64(),
                checkpoint.digest.as_slice()
            ],
            |row| row.get(0),
        )
        .map_err(storage)?;
    let payload: Value = serde_json::from_str(&raw).map_err(storage)?;
    let key = StateKey {
        namespace: StateNamespace::Bound {
            binding: facts::binding(&payload)?,
        },
        target: facts::target("commit", &payload)?,
    };
    DurableStateChange::scoped(key, payload)
}

pub(super) fn matches_saved(
    connection: &Connection,
    library: LibraryId,
    checkpoint: CommitCheckpoint,
    batch: &LibraryBatch,
) -> Result<bool, Error> {
    if batch.library_id != library || batch.origin != ChangeOrigin::HostConfirmation {
        return Ok(false);
    }
    let current = current_ledger(connection, checkpoint)?;
    verify_ledger(&current, checkpoint)?;
    let mut matching = batch.state.iter().filter(|state| state.key == current.key);
    Ok(matching.next() == Some(&current) && matching.next().is_none())
}
