//! Bounded push/pull cycle for one authenticated host and local replica.

#![allow(clippy::missing_errors_doc)]

use echo_core::application::ports::{LocalSyncQueuePort, RemoteApplyPort};
use echo_core::domain::sync::{CommitSeq, DeviceId, MutationCandidate, SyncBinding};
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
/// push at that point; later events are never submitted out of order. The
/// caller schedules another bounded cycle after backoff or predecessor repair.
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
    let candidates = queue.pending_local_mutations(limit)?;
    let push_may_continue = candidates.len() == limit;
    let mut mutations_confirmed = 0;
    let mut mutations_rejected = 0;
    for candidate in &candidates {
        let mutation = mutation_dto(binding, candidate);
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
        } else {
            if ack.confirmation.checkpoint.is_some() {
                return Err(ReplicaSyncError::InvalidOutcome);
            }
            if outcome.reason.as_deref() == Some("OutOfOrderPredecessor") {
                break;
            }
            queue.reject_local_mutation(candidate.mutation_id, ack.confirmation.outcome_json)?;
            mutations_rejected += 1;
        }
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

fn mutation_dto(binding: SyncBinding, candidate: &MutationCandidate) -> MutationDto {
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
        payload: candidate.payload.clone(),
    }
}

#[derive(Deserialize)]
struct MutationOutcome {
    accepted: bool,
    reason: Option<String>,
}
