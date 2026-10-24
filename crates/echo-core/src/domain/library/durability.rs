//! Library-local durable transaction protocol. These values never contain paths
//! outside the library, credentials, database handles, or transport clients.

mod change_set;
mod confirmed;
mod frozen;
pub mod projection;
mod receipt;
mod session;
mod state;
#[cfg(test)]
mod tests;
pub use frozen::{FrozenMutation, MutationDisposition, MutationOperation, MutationStatus};
pub use projection::{
    LibraryProjectionStatus, LibraryRecoveryRecords, ProjectionTarget, ProjectionUnresolvedReason,
    UnresolvedProjectionRecord,
};
pub use session::{RecoverySessionRole, SyncRecoveryContext};
pub use state::{StateKey, StateNamespace, StateTarget};

use super::{LibraryId, PortableRecordEnvelope, PortableSerialize};
use crate::error::Error;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

pub type LibraryOperationId = uuid::Uuid;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeOrigin {
    LocalEdit,
    Import,
    Scan,
    Remote,
    HostConfirmation,
    Migration,
    Recovery,
}

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StateKind {
    Confirmed,
    Pending,
    PendingDisposition,
    Receipt,
    Alias,
    Tombstone,
    ResourceLocation,
    Cursor,
    SyncLedger,
    DeleteIntent,
    Conflict,
    PlaybackEvent,
    Adoption,
    Claim,
}

impl StateKind {
    #[must_use]
    pub const fn directory(self) -> &'static str {
        match self {
            Self::Confirmed => "confirmed",
            Self::Pending => "pending",
            Self::PendingDisposition => "pending-dispositions",
            Self::Receipt => "receipts",
            Self::Alias => "aliases",
            Self::Tombstone => "tombstones",
            Self::ResourceLocation => "resource-locations",
            Self::Cursor => "cursors",
            Self::SyncLedger => "sync-ledger",
            Self::DeleteIntent => "delete-intents",
            Self::Conflict => "conflicts",
            Self::PlaybackEvent => "playback-events",
            Self::Adoption => "adoptions",
            Self::Claim => "claims",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DurableStateChange {
    pub kind: StateKind,
    pub id: uuid::Uuid,
    pub payload: serde_json::Value,
    /// Absence denotes the original unscoped evidence format. Omission on
    /// serialization is essential: old committed canonical digests stay valid.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<StateKey>,
}
impl DurableStateChange {
    #[must_use]
    pub const fn new(kind: StateKind, id: uuid::Uuid, payload: serde_json::Value) -> Self {
        Self {
            kind,
            id,
            payload,
            key: None,
        }
    }
    /// Construct a new scoped fact without changing its protocol event identity.
    ///
    /// # Errors
    /// Rejects invalid keys, non-object payloads, and mismatched typed bodies.
    pub fn scoped(key: StateKey, mut payload: serde_json::Value) -> Result<Self, Error> {
        let id = key.storage_id()?;
        payload
            .as_object_mut()
            .ok_or_else(|| Error::conflict("scoped state payload is not an object"))?
            .insert(
                "state_key".to_owned(),
                serde_json::to_value(&key).map_err(state::encode)?,
            );
        let result = Self {
            kind: key.target.kind(),
            id,
            payload,
            key: Some(key),
        };
        result.validate()?;
        Ok(result)
    }
    /// # Errors
    /// Rejects mismatched storage identity, payload key, or frozen event body.
    pub fn validate(&self) -> Result<(), Error> {
        if self.id.is_nil() {
            return Err(Error::conflict("nil durable state identity"));
        }
        let Some(key) = &self.key else {
            return Ok(());
        };
        if self.id != key.storage_id()?
            || self.kind != key.target.kind()
            || self.payload.get("state_key")
                != Some(&serde_json::to_value(key).map_err(state::encode)?)
        {
            return Err(Error::conflict("durable state key/id/payload mismatch"));
        }
        match key.target {
            StateTarget::Mutation { .. } => serde_json::from_value::<FrozenMutation>(
                self.payload
                    .get("frozen")
                    .cloned()
                    .ok_or_else(|| Error::conflict("missing frozen mutation body"))?,
            )
            .map_err(state::encode)?
            .validate(key)?,
            StateTarget::MutationDisposition { .. } => {
                serde_json::from_value::<MutationDisposition>(
                    self.payload
                        .get("disposition")
                        .cloned()
                        .ok_or_else(|| Error::conflict("missing mutation disposition"))?,
                )
                .map_err(state::encode)?
                .validate(key)?;
            }
            _ => state::validate_payload(key, &self.payload)?,
        }
        Ok(())
    }
    /// # Errors
    /// Reports canonical key serialization failure. Legacy keys are unchanged.
    pub fn storage_key(&self) -> Result<String, Error> {
        self.key.as_ref().map_or_else(
            || serde_json::to_string(&(self.kind, self.id)).map_err(state::encode),
            StateKey::canonical,
        )
    }
}

/// Local idempotence evidence for a qualified playback event. Event identity
/// belongs to `DurableStateChange::id`, never to a shared `PlayStatsRecord`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlaybackEvent {
    pub song_uuid: uuid::Uuid,
    /// Unix milliseconds, matching the local recorded-play-session projection.
    pub recorded_at: u64,
}

/// A resource already published inside the library. The adapter verifies bytes
/// before accepting the commit. Missing remote resources are not declared ready.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DurableResource {
    pub relative_path: String,
    pub content_hash: String,
    pub byte_length: u64,
}

/// Preconditions use canonical document hashes, not SQLite row versions.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectedRecord {
    pub kind: super::RecordKind,
    pub object_uuid: uuid::Uuid,
    pub content_hash: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibraryBatch {
    pub operation_id: LibraryOperationId,
    /// The local directory identity from `LibraryManifest`, independent of any
    /// authority-owned sync library identity retained in records or state.
    pub library_id: LibraryId,
    pub origin: ChangeOrigin,
    pub records: Vec<PortableRecordEnvelope>,
    #[serde(default)]
    pub state: Vec<DurableStateChange>,
    #[serde(default)]
    pub resources: Vec<DurableResource>,
    #[serde(default)]
    pub expected: Vec<ExpectedRecord>,
}
impl LibraryBatch {
    #[must_use]
    pub const fn new(
        operation_id: uuid::Uuid,
        library_id: LibraryId,
        origin: ChangeOrigin,
        records: Vec<PortableRecordEnvelope>,
    ) -> Self {
        Self {
            operation_id,
            library_id,
            origin,
            records,
            state: Vec::new(),
            resources: Vec::new(),
            expected: Vec::new(),
        }
    }
    /// Validate typed targets, identities, preconditions and resource paths.
    ///
    /// # Errors
    /// Rejects invalid or duplicate identities, unsafe resources and malformed
    /// portable envelopes. Directory ownership is checked by the adapter.
    pub fn validate(&self) -> Result<(), Error> {
        if self.operation_id.is_nil() || self.library_id.as_uuid().is_nil() {
            return Err(Error::conflict("durable operation identity is nil"));
        }
        // Sync metadata describes an authority-owned identity, not this local
        // directory owner. Preserving it does not authorize a live sync session.
        let mut targets = BTreeSet::new();
        for envelope in &self.records {
            envelope.validate()?;
            if envelope.record().object_uuid().is_nil() {
                return Err(Error::conflict("durable record identity is nil"));
            }
            match envelope.record() {
                super::PortableRecord::Song(song) => {
                    super::LibraryRelativePath::validate(song.media_path.as_str())?;
                }
                super::PortableRecord::PlaylistItem(item)
                    if item.song_uuid.is_nil() || item.playlist_uuid.is_nil() =>
                {
                    return Err(Error::conflict("durable member has no parent identity"));
                }
                super::PortableRecord::Playlist(playlist) => {
                    if let Some(asset) = &playlist.cover_asset {
                        asset.validate()?;
                    }
                }
                _ => {}
            }
            let key = envelope.record().object_key()?;
            if !targets.insert(serde_json::to_string(&key).map_err(|e| {
                Error::unavailable_with_source("durable target", "cannot encode target", e)
            })?) {
                return Err(Error::conflict("duplicate typed durable record target"));
            }
        }
        // Historical state may name several bindings. Its specialized recovery
        // use case validates trust; JSON field names cannot establish ownership.
        let mut states = BTreeSet::new();
        for state in &self.state {
            state.validate()?;
            if state.key.as_ref().is_some_and(|key| {
                matches!(key.namespace,
                StateNamespace::LocalUnbound { library_id, .. } if library_id != self.library_id)
            }) {
                return Err(Error::conflict(
                    "unbound state belongs to another local library",
                ));
            }
            if !states.insert(state.storage_key()?) {
                return Err(Error::conflict("duplicate durable state target"));
            }
        }
        let mut resources = BTreeSet::new();
        for resource in &self.resources {
            let path = crate::domain::ids::RelativeMediaPath::new(&resource.relative_path)?;
            if path
                .normalized()
                .split('/')
                .next()
                .is_some_and(|p| p.eq_ignore_ascii_case("echo"))
                && !path.normalized().starts_with("echo/media/")
                && !path.normalized().starts_with("echo/assets/")
            {
                return Err(Error::conflict(
                    "resource is not media or an immutable asset",
                ));
            }
            if path.normalized() != resource.relative_path
                || !resources.insert(path.identity_key().to_owned())
                || resource.relative_path.len() > 4096
                || !valid_digest(&resource.content_hash)
                || (path.normalized().starts_with("echo/assets/")
                    && path.normalized() != format!("echo/assets/{}", resource.content_hash))
            {
                return Err(Error::conflict("resource requires a BLAKE3 digest"));
            }
        }
        let mut expected = BTreeSet::new();
        for item in &self.expected {
            if item.kind == super::RecordKind::Tombstone
                || item.object_uuid.is_nil()
                || !expected.insert((item.kind.dir_name(), item.object_uuid))
                || item
                    .content_hash
                    .as_ref()
                    .is_some_and(|hash| !valid_digest(hash))
            {
                return Err(Error::conflict("invalid or ambiguous typed precondition"));
            }
        }
        Ok(())
    }
    /// Hash the canonical batch payload.
    ///
    /// # Errors
    /// Reports a payload serialization failure.
    pub fn digest(&self) -> Result<String, Error> {
        self.content_hash()
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibraryCommit {
    pub batch: LibraryBatch,
    pub library_commit_seq: u64,
    pub predecessor_digest: Option<String>,
    pub digest: String,
}
impl LibraryCommit {
    /// Bind an immutable payload to its local sequence and predecessor.
    ///
    /// # Errors
    /// Reports a payload serialization failure.
    pub fn calculate_digest(
        batch: &LibraryBatch,
        seq: u64,
        predecessor: Option<&str>,
    ) -> Result<String, Error> {
        (batch, seq, predecessor).content_hash()
    }
    /// Verify the batch and its sequence-bound digest.
    ///
    /// # Errors
    /// Rejects invalid batches, zero sequences or a mismatched digest.
    pub fn validate(&self) -> Result<(), Error> {
        self.batch.validate()?;
        if self.library_commit_seq == 0
            || self
                .predecessor_digest
                .as_deref()
                .map_or(self.library_commit_seq != 1, |hash| {
                    self.library_commit_seq == 1 || !valid_digest(hash)
                })
            || Self::calculate_digest(
                &self.batch,
                self.library_commit_seq,
                self.predecessor_digest.as_deref(),
            )? != self.digest
        {
            return Err(Error::conflict("durable transaction digest mismatch"));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CommitStatus {
    NotSaved,
    SavedPendingRecovery,
    SavedPendingProjection,
    Complete,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CommitOutcome {
    pub operation_id: uuid::Uuid,
    pub library_commit_seq: u64,
    pub status: CommitStatus,
    pub reason: Option<String>,
}

/// A complete coverage image, never an invented historical operation. Its head
/// is the digest of the last real commit included in this image.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LibraryCheckpoint {
    pub library_id: LibraryId,
    pub library_commit_seq: u64,
    pub head_digest: String,
    pub records: Vec<PortableRecordEnvelope>,
    pub state: Vec<DurableStateChange>,
    pub resources: Vec<DurableResource>,
}

/// A verified coverage image followed by an ordered suffix of real commits.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct LibraryReplay {
    pub checkpoint: Option<LibraryCheckpoint>,
    pub commits: Vec<LibraryCommit>,
}
impl LibraryReplay {
    /// Verify that a typed frozen body never changes across local commits.
    /// Dispositions have their own keys and may advance independently.
    ///
    /// # Errors
    /// Rejects a different body under the same complete frozen event key.
    pub fn validate_state_history(&self) -> Result<(), Error> {
        self.validate_state_extension(&[])
    }
    /// # Errors
    /// Rejects rewriting any previously frozen, scoped event body.
    pub fn validate_state_extension(&self, extension: &[DurableStateChange]) -> Result<(), Error> {
        let mut frozen = std::collections::BTreeMap::new();
        for state in self.states().chain(extension) {
            if state
                .key
                .as_ref()
                .is_some_and(|key| matches!(key.target, StateTarget::Mutation { .. }))
            {
                let key = state.storage_key()?;
                if frozen
                    .insert(key, state)
                    .is_some_and(|previous| previous != state)
                {
                    return Err(Error::conflict("immutable frozen mutation body changed"));
                }
            }
        }
        Ok(())
    }
    pub fn states(&self) -> impl Iterator<Item = &DurableStateChange> {
        self.checkpoint
            .iter()
            .flat_map(|c| &c.state)
            .chain(self.commits.iter().flat_map(|c| &c.batch.state))
    }
    #[must_use]
    pub fn head(&self) -> Option<(u64, &str)> {
        self.commits
            .last()
            .map(|c| (c.library_commit_seq, c.digest.as_str()))
            .or_else(|| {
                self.checkpoint
                    .as_ref()
                    .map(|c| (c.library_commit_seq, c.head_digest.as_str()))
            })
    }
}

pub(crate) fn valid_digest(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
}
