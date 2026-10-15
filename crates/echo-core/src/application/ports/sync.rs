//! Durable ledger and independent checkpoint capabilities for host sync.

use crate::domain::sync::{
    CommitCheckpoint, CommitSeq, DeviceId, MutationCandidate, MutationId, SyncBinding,
};
use crate::error::Error;
use serde::{Deserialize, Serialize};

/// An immutable host view with one published log checkpoint. The opaque token
/// remains valid across application restarts until `expires_at_ms`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HostSnapshotSession {
    pub token: String,
    pub checkpoint: CommitCheckpoint,
    pub page_size: u32,
    pub record_count: u64,
    pub expires_at_ms: i64,
}

/// Typed records share one frozen order, so aliases and objects cannot acquire
/// independent pagination cursors. Final tombstones remain explicit records:
/// enrollment must retain deletion knowledge even when no live object remains.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum HostSnapshotRecord {
    Object {
        candidate: MutationCandidate,
        version: crate::domain::sync::ObjectVersion,
        resources: Vec<crate::domain::sync::SharedResourceManifest>,
    },
    SongAlias {
        alias: crate::domain::ids::SongId,
        canonical: crate::domain::ids::SongId,
    },
    PlaylistMemberAlias(crate::domain::sync::PlaylistMemberAlias),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct HostSnapshotPage {
    pub session: HostSnapshotSession,
    pub page_index: u32,
    pub next_page_index: Option<u32>,
    pub records: Vec<HostSnapshotRecord>,
}

/// One authenticated page received from the frozen host snapshot. Object
/// versions are host versions, independent of each mutation's base version.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ReplicaSnapshotPage {
    pub snapshot_token: String,
    pub checkpoint: CommitCheckpoint,
    pub page_index: u32,
    pub next_page_index: Option<u32>,
    pub records: Vec<HostSnapshotRecord>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AppliedSnapshotPage {
    pub next_page_index: Option<u32>,
    /// Present only after the full snapshot and peer cursor commit together.
    pub checkpoint: Option<CommitCheckpoint>,
    /// Newly staged records; an exact page retry returns zero.
    pub applied: usize,
}

/// Stage consecutive pages durably. The final page atomically projects all
/// records and advances the host cursor, without creating local mutations.
/// Enrollment requires a matching, empty replica without local pending edits.
pub trait ReplicaSnapshotApplyPort: Send + Sync {
    fn apply_snapshot_page(
        &self,
        peer_device_id: crate::domain::sync::DeviceId,
        page: &ReplicaSnapshotPage,
    ) -> Result<AppliedSnapshotPage, Error>;

    /// Discard an incomplete transfer after snapshot expiry or cancellation.
    /// Completed enrollment cannot be discarded by this method.
    fn discard_snapshot(
        &self,
        peer_device_id: crate::domain::sync::DeviceId,
        snapshot_token: &str,
    ) -> Result<(), Error>;
}

/// Capture and page durable published state without exporting local optimistic
/// projections, database paths, or credentials. Page size is 1..=512; lifetime
/// is `1..=86_400_000` milliseconds. Expiry is checked on every page read.
pub trait HostSnapshotPort: Send + Sync {
    fn create_host_snapshot(
        &self,
        binding: SyncBinding,
        page_size: usize,
        lifetime_ms: u64,
    ) -> Result<HostSnapshotSession, Error>;

    fn host_snapshot_page(
        &self,
        binding: SyncBinding,
        token: &str,
        page_index: u32,
    ) -> Result<HostSnapshotPage, Error>;
}

/// Result persisted by the host's inbox transaction. A checkpoint is present
/// only for an accepted commit; durable rejections still have an idempotent
/// outcome but do not advance the visible commit log.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StagedConfirmation {
    pub mutation_id: MutationId,
    pub outcome_json: String,
    pub checkpoint: Option<CommitCheckpoint>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PublishedCommit {
    pub sequence: CommitSeq,
    pub previous_digest: [u8; 32],
    pub digest: [u8; 32],
    pub canonical_json: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemoteCommit {
    pub sequence: CommitSeq,
    pub previous_digest: [u8; 32],
    pub digest: [u8; 32],
    pub origin_device_id: crate::domain::sync::DeviceId,
    pub mutation_id: MutationId,
    pub object: crate::domain::sync::ObjectKey,
    pub payload_json: String,
    pub canonical_json: String,
    pub is_tombstone: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AppliedRemoteCursor {
    pub sequence: CommitSeq,
    pub digest: [u8; 32],
    pub applied: usize,
}

/// Apply a host-confirmed batch without generating local outbox mutations.
pub trait RemoteApplyPort: Send + Sync {
    fn apply_remote_batch(
        &self,
        binding: SyncBinding,
        peer_device_id: crate::domain::sync::DeviceId,
        anchor: CommitCheckpoint,
        commits: &[RemoteCommit],
    ) -> Result<AppliedRemoteCursor, Error>;
}

/// SQLite-backed host ledger. `stage_confirmation` must atomically persist the
/// inbox identity/result, object projection, commit log and pending publication.
/// It is idempotent for the same mutation ID and exact body.
pub trait HostCommitLedgerPort: Send + Sync {
    fn stage_confirmation(
        &self,
        binding: SyncBinding,
        candidate: &MutationCandidate,
    ) -> Result<StagedConfirmation, Error>;

    fn mark_published(&self, checkpoint: CommitCheckpoint) -> Result<(), Error>;

    fn pending_publications(&self, limit: usize) -> Result<Vec<StagedConfirmation>, Error>;

    fn published_commits_after(
        &self,
        binding: SyncBinding,
        after: CommitSeq,
        expected_digest: [u8; 32],
        limit: usize,
    ) -> Result<Vec<PublishedCommit>, Error>;
}

/// Re-check a peer's current host authorization for every protected request.
/// Credential possession alone is insufficient because revocation must take
/// effect for already-running server instances and existing TLS sessions.
pub trait HostPeerAuthorizationPort: Send + Sync {
    fn peer_is_authorized(
        &self,
        binding: SyncBinding,
        peer_device_id: crate::domain::sync::DeviceId,
    ) -> Result<bool, Error>;
}

/// Durable local replica work and its host checkpoint. Rejected mutations are
/// terminal queue outcomes; they remain in history but no longer block the
/// device's later origin-chain entries.
pub trait LocalSyncQueuePort: Send + Sync {
    fn pending_local_mutations(&self, limit: usize) -> Result<Vec<MutationCandidate>, Error>;

    fn reject_local_mutation(
        &self,
        mutation_id: MutationId,
        outcome_json: String,
    ) -> Result<(), Error>;

    fn replica_checkpoint(&self, host_device_id: DeviceId) -> Result<CommitCheckpoint, Error>;
}

/// Platform-owned durable high-water independent of the library/ledger backup.
pub trait PublicationCheckpointPort: Send + Sync {
    /// Atomically persist a monotone checkpoint for the existing binding.
    /// Concurrent or delayed publication retries must never move it backwards:
    /// an older sequence is a no-op, and an identical checkpoint is idempotent.
    /// Reject a different binding or a different digest at the same sequence;
    /// initialization and explicit epoch recovery use separate platform flows.
    /// Return success only after durable storage, and leave the prior value
    /// intact on failure. Implementations must serialize comparison and write.
    fn advance(&self, checkpoint: CommitCheckpoint) -> Result<(), Error>;
}
