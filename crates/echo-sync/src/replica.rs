//! Bounded push/pull cycle for one authenticated host and local replica.

#![allow(clippy::missing_errors_doc)]

use echo_core::application::ports::{LocalSyncQueuePort, RemoteApplyPort};
use echo_core::domain::sync::{
    CommitSeq, DeviceId, MutationCandidate, MutationId, OriginAnchor, SyncBinding,
};
use echo_core::error::Error;
use reqwest::Url;
use serde::Deserialize;
use std::time::Duration;

use crate::client::{ClientError, LanClient};
use crate::protocol::{MutationDto, PROTOCOL_VERSION};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReplicaSyncReport {
    pub mutations_confirmed: usize,
    pub mutations_rejected: usize,
    pub commits_applied: usize,
    pub checkpoint: CommitSeq,
    pub digest: [u8; 32],
    /// More local mutations may remain after the request budget was consumed.
    pub push_may_continue: bool,
    /// More remote commits may be available after this pull page.
    pub pull_may_continue: bool,
}

#[derive(Debug)]
pub enum ReplicaSyncError {
    Client(ClientError),
    Core(Error),
    InvalidOutcome,
    WrongMutationAck,
    WrongBinding,
}

impl std::fmt::Display for ReplicaSyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Client(error) => write!(f, "sync transport failed: {error}"),
            Self::Core(error) => write!(f, "local sync state failed: {error}"),
            Self::InvalidOutcome => f.write_str("host returned an invalid mutation outcome"),
            Self::WrongMutationAck => f.write_str("host acknowledged a different mutation"),
            Self::WrongBinding => f.write_str("replica binding differs from the paired host"),
        }
    }
}
impl std::error::Error for ReplicaSyncError {}

impl From<ClientError> for ReplicaSyncError {
    fn from(value: ClientError) -> Self {
        Self::Client(value)
    }
}
impl From<Error> for ReplicaSyncError {
    fn from(value: Error) -> Self {
        Self::Core(value)
    }
}

/// Push one bounded ordered batch, then pull and atomically apply one published
/// page. Local event identities and payloads remain fixed across retries.
///
/// A predecessor rejection that the host explicitly marks retryable stops the
/// push at that point; later events are never submitted out of order. When the
/// rejection also names a host position this device's head does not continue,
/// the two ledgers have diverged and the pending chain is re-anchored onto the
/// host before the batch is offered again — the host owns that sequence space,
/// so retrying a locally invented one could never succeed.
pub async fn synchronize_once(
    client: &LanClient,
    endpoint: &Url,
    binding: SyncBinding,
    host_device_id: DeviceId,
    queue: &dyn LocalSyncQueuePort,
    remote_apply: &dyn RemoteApplyPort,
    limit: usize,
) -> Result<ReplicaSyncReport, ReplicaSyncError> {
    if !(1..=512).contains(&limit) {
        return Err(ReplicaSyncError::InvalidOutcome);
    }
    let mut mutations_confirmed = 0;
    let mut mutations_rejected = 0;
    let mut push_may_continue;
    // At most one re-anchor per cycle: the host's position is read from its own
    // rejection, the pending events are re-sequenced onto it, and the same batch
    // is offered again. A second divergence in one cycle would mean the host is
    // not the authority it claims to be, so it surfaces instead of looping.
    let mut reanchored = false;
    loop {
        let candidates = next_push_batch(queue, limit)?;
        push_may_continue = candidates.len() == limit;
        let mut anchor = None;
        for candidate in &candidates {
            let mutation = mutation_dto(
                binding,
                candidate,
                queue.local_mutation_changed_at_ms(candidate.mutation_id)?,
            );
            let ack = client.submit_mutation(endpoint, &mutation).await?;
            if ack.confirmation.mutation_id != candidate.mutation_id {
                return Err(ReplicaSyncError::WrongMutationAck);
            }
            let outcome: MutationOutcome = serde_json::from_str(&ack.confirmation.outcome_json)
                .map_err(|_| ReplicaSyncError::InvalidOutcome)?;
            if outcome.accepted {
                if ack.confirmation.checkpoint.is_none() {
                    return Err(ReplicaSyncError::InvalidOutcome);
                }
                mutations_confirmed += 1;
                continue;
            }
            if ack.confirmation.checkpoint.is_some() {
                return Err(ReplicaSyncError::InvalidOutcome);
            }
            if outcome.reason.as_deref() == Some("OutOfOrderPredecessor") {
                // The host names the last event it confirmed. When our head does
                // not continue that position, the chain itself is broken (a
                // rebuilt database, a rebound library root, a migration that
                // retired older events) and no amount of retrying fixes it.
                anchor = reanchor_target(candidate, &outcome);
                break;
            }
            queue.reject_local_mutation(candidate.mutation_id, ack.confirmation.outcome_json)?;
            mutations_rejected += 1;
        }
        let Some(anchor) = anchor else {
            break;
        };
        if reanchored {
            break;
        }
        queue.rebase_local_chain(anchor)?;
        reanchored = true;
    }

    let checkpoint = queue.replica_checkpoint(host_device_id)?;
    if checkpoint.binding != binding {
        return Err(ReplicaSyncError::WrongBinding);
    }
    let page = client
        .pull_commits(
            endpoint,
            checkpoint.sequence.as_u64(),
            checkpoint.digest,
            limit,
            Duration::ZERO,
        )
        .await?;
    if page.binding != binding {
        return Err(ReplicaSyncError::WrongBinding);
    }
    let pull_may_continue = page.entries.len() == limit;
    let commits_applied = page.entries.len();
    let applied = LanClient::apply_commit_page(&page, host_device_id, remote_apply)?;
    Ok(ReplicaSyncReport {
        mutations_confirmed,
        mutations_rejected,
        commits_applied,
        checkpoint: applied.sequence,
        digest: applied.digest,
        push_may_continue,
        pull_may_continue,
    })
}

/// Select the next push batch, repairing any cursor gap first.
///
/// A terminal rejection recorded while its predecessor was still outstanding
/// leaves the local origin cursor behind: `reject_local_mutation` only advances
/// it when the rejected event is exactly the next one in the chain, and a later
/// repair has to move it across the older event. The host validates
/// `previous_mutation_id` against that same cursor, so an un-advanced gap makes
/// every subsequent pending event look out of order — the push then stops on
/// `OutOfOrderPredecessor` at the identical place on every cycle and never
/// reaches the events behind it. Repair before reading the batch, which is what
/// the host-side confirmation loop does for its own ledger.
fn next_push_batch(
    queue: &dyn LocalSyncQueuePort,
    limit: usize,
) -> Result<Vec<MutationCandidate>, Error> {
    queue.advance_rejected_local_mutations()?;
    queue.pending_local_mutations(limit)
}

fn mutation_dto(
    binding: SyncBinding,
    candidate: &MutationCandidate,
    changed_at_ms: Option<u64>,
) -> MutationDto {
    MutationDto {
        resource_refs: candidate
            .resource_refs
            .iter()
            .map(crate::protocol::ResourceRefDto::from)
            .collect(),
        protocol_version: PROTOCOL_VERSION,
        binding,
        origin_device_id: candidate.origin_device_id,
        mutation_id: candidate.mutation_id,
        origin_seq: candidate.origin_seq,
        previous_mutation_id: candidate.previous_mutation_id,
        object: candidate.object,
        base_object_version: candidate.base_object_version,
        changed_at_ms,
        payload: candidate.payload.clone(),
    }
}

#[derive(Deserialize)]
struct MutationOutcome {
    accepted: bool,
    reason: Option<String>,
    /// The host's confirmed position in this origin chain. Present only on a
    /// retryable predecessor rejection, which is the one rejection that says
    /// "your chain does not fit mine" rather than "this event is invalid".
    #[serde(default)]
    origin_seq: Option<u64>,
    #[serde(default)]
    last_mutation_id: Option<MutationId>,
}

impl MutationOutcome {
    fn anchor(&self) -> Option<OriginAnchor> {
        Some(OriginAnchor {
            origin_seq: self.origin_seq?,
            last_mutation_id: self.last_mutation_id,
        })
    }
}

/// The host position this event must be re-anchored onto, if any.
///
/// Only a predecessor rejection that names a position the event does not
/// already continue is a broken chain. A host that is merely waiting for an
/// event we have not sent yet names exactly our predecessor, and re-anchoring
/// there would renumber the queue for no reason.
fn reanchor_target(
    candidate: &MutationCandidate,
    outcome: &MutationOutcome,
) -> Option<OriginAnchor> {
    if outcome.reason.as_deref() != Some("OutOfOrderPredecessor") {
        return None;
    }
    let anchor = outcome.anchor()?;
    (!anchor.is_next(candidate)).then_some(anchor)
}

#[cfg(test)]
#[path = "replica_tests.rs"]
mod tests;
