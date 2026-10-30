//! Only a complete hash-verified prefix ending at the independent live anchor is evidence.
use super::{
    number, params, schema, storage, text, Connection, DurableStateChange, Error,
    OptionalExtension, Reason, StateNamespace, SyncRecoveryContext, Value,
};
use crate::application::ports::RemoteCommit;
use crate::domain::sync::CommitSeq;
use std::collections::BTreeMap;

pub(super) struct Verified {
    pub commits: BTreeMap<u64, RemoteCommit>,
}

pub(super) fn verified(
    c: &Connection,
    context: SyncRecoveryContext,
    states: &[&DurableStateChange],
) -> Result<Verified, Error> {
    let anchor = context.verified_checkpoint;
    let mut commits = BTreeMap::new();
    for state in states.iter().filter(|s| schema(s, "echo.sync.commit.v1")) {
        if let Some(commit) = retained_commit(state, context)? {
            if commits.insert(commit.sequence.as_u64(), commit).is_some() {
                return Err(Error::conflict("duplicate retained ledger sequence"));
            }
        }
    }
    // Existing rows can supply missing retained predecessors, but must pass the
    // identical cryptographic check and full binding check.
    let mut statement=c.prepare("SELECT commit_seq,previous_digest,commit_digest,canonical_json FROM sync_commits WHERE library_id=?1 AND authority_id=?2 AND epoch=?3 AND commit_seq<=?4 ORDER BY commit_seq").map_err(storage)?;
    let rows = statement
        .query_map(
            params![
                anchor.binding.library_id.as_uuid().to_string(),
                anchor.binding.authority_id.as_uuid().to_string(),
                anchor.binding.epoch.as_u64(),
                anchor.sequence.as_u64()
            ],
            |r| {
                Ok((
                    r.get::<_, u64>(0)?,
                    r.get::<_, Vec<u8>>(1)?,
                    r.get::<_, Vec<u8>>(2)?,
                    r.get::<_, String>(3)?,
                ))
            },
        )
        .map_err(storage)?;
    for row in rows {
        let (sequence, previous, digest, raw) = row.map_err(storage)?;
        let p: Value = serde_json::from_str(&raw).map_err(storage)?;
        let candidate = &p["mutation"];
        let commit = RemoteCommit {
            sequence: CommitSeq::from_u64(sequence),
            previous_digest: previous
                .try_into()
                .map_err(|_| Error::conflict("invalid ledger predecessor digest"))?,
            digest: digest
                .try_into()
                .map_err(|_| Error::conflict("invalid ledger digest"))?,
            origin_device_id: serde_json::from_value(candidate["origin_device_id"].clone())
                .map_err(storage)?,
            mutation_id: serde_json::from_value(candidate["mutation_id"].clone())
                .map_err(storage)?,
            object: serde_json::from_value(candidate["object"].clone()).map_err(storage)?,
            payload_json: serde_json::to_string(&candidate["payload"]).map_err(storage)?,
            is_tombstone: candidate["payload"] == serde_json::json!({"tombstone":true}),
            canonical_json: raw,
        };
        crate::infrastructure::sqlite::sync::apply::verify_remote_commit(anchor.binding, &commit)?;
        if commits.get(&sequence).is_some_and(|old| old != &commit) {
            return Err(Error::conflict(
                "retained ledger forks from current session",
            ));
        }
        commits.insert(sequence, commit);
    }
    let mut previous = [0; 32];
    let mut expected = 1;
    for (seq, commit) in &commits {
        if *seq != expected || commit.previous_digest != previous {
            return Ok(Verified {
                commits: BTreeMap::new(),
            });
        }
        previous = commit.digest;
        expected += 1;
    }
    if expected - 1 != anchor.sequence.as_u64() || previous != anchor.digest {
        commits.clear();
    }
    Ok(Verified { commits })
}

fn retained_commit(
    state: &DurableStateChange,
    context: SyncRecoveryContext,
) -> Result<Option<RemoteCommit>, Error> {
    let p = &state.payload;
    // Older exports lack canonical bytes. Re-encoding JSON cannot prove the
    // original digest and is deliberately left unresolved.
    if p.get("original_body").and_then(Value::as_str).is_none() {
        return Ok(None);
    }
    let sequence = number(p, "sequence")?;
    if sequence > context.verified_checkpoint.sequence.as_u64() {
        return Ok(None);
    }
    let raw = text(p, "original_body")?;
    let canonical: Value = serde_json::from_str(raw).map_err(storage)?;
    if canonical != p["body"] {
        return Err(Error::conflict(
            "ledger original body differs from retained value",
        ));
    }
    let candidate = &canonical["mutation"];
    let commit = RemoteCommit {
        sequence: CommitSeq::from_u64(sequence),
        previous_digest: digest(text(p, "previous_digest")?)?,
        digest: digest(text(p, "digest")?)?,
        origin_device_id: serde_json::from_value(candidate["origin_device_id"].clone())
            .map_err(storage)?,
        mutation_id: serde_json::from_value(candidate["mutation_id"].clone()).map_err(storage)?,
        object: serde_json::from_value(candidate["object"].clone()).map_err(storage)?,
        payload_json: serde_json::to_string(&candidate["payload"]).map_err(storage)?,
        canonical_json: raw.to_owned(),
        is_tombstone: candidate["payload"] == serde_json::json!({"tombstone":true}),
    };
    if commit.origin_device_id.as_uuid().to_string() != text(p, "author")?
        || commit.mutation_id.as_uuid().to_string() != text(p, "mutation_id")?
    {
        return Err(Error::conflict(
            "ledger target differs from canonical event",
        ));
    }
    crate::infrastructure::sqlite::sync::apply::verify_remote_commit(
        context.verified_checkpoint.binding,
        &commit,
    )?;
    Ok(Some(commit))
}

pub(super) fn project(
    c: &Connection,
    state: &DurableStateChange,
    verified: &Verified,
) -> Result<Option<Reason>, Error> {
    if !schema(state, "echo.sync.commit.v1") {
        return Ok(Some(Reason::InvalidStateFact));
    }
    let p = &state.payload;
    let Some(commit) = verified.commits.get(&number(p, "sequence")?) else {
        return Ok(Some(Reason::MissingSyncDependency));
    };
    // Loss of created_at is historical evidence loss, not permission to invent a clock.
    let Some(created) = p.get("created_at").and_then(Value::as_u64) else {
        return Ok(Some(Reason::RequiresStateRecovery));
    };
    let b = state
        .key
        .as_ref()
        .and_then(|key| match key.namespace {
            StateNamespace::Bound { binding } => Some(binding),
            _ => None,
        })
        .ok_or_else(|| Error::conflict("ledger has no binding"))?;
    let existing: Option<(String, Vec<u8>)> = c
        .query_row(
            "SELECT canonical_json,commit_digest FROM sync_commits WHERE commit_seq=?1",
            [commit.sequence.as_u64()],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()
        .map_err(storage)?;
    if existing.as_ref().is_some_and(|(raw, digest)| {
        raw != &commit.canonical_json || digest.as_slice() != commit.digest
    }) {
        return Ok(Some(Reason::IdentityConflict));
    }
    c.execute("INSERT INTO sync_commits(commit_seq,library_id,authority_id,epoch,origin_device_id,mutation_id,canonical_json,previous_digest,commit_digest,publication_state,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'published',?10) ON CONFLICT(commit_seq) DO NOTHING",params![commit.sequence.as_u64(),b.library_id.as_uuid().to_string(),b.authority_id.as_uuid().to_string(),b.epoch.as_u64(),commit.origin_device_id.as_uuid().to_string(),commit.mutation_id.as_uuid().to_string(),commit.canonical_json,commit.previous_digest.as_slice(),commit.digest.as_slice(),created]).map_err(storage)?;
    Ok(None)
}
fn digest(text: &str) -> Result<[u8; 32], Error> {
    let hash =
        blake3::Hash::from_hex(text).map_err(|_| Error::conflict("invalid ledger digest"))?;
    Ok(*hash.as_bytes())
}
