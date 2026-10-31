//! Frozen local event bodies are immutable; progress and outcomes live elsewhere.
use super::{
    state::{digest_valid, encode, object_identity},
    StateKey, StateTarget,
};
use crate::domain::library::PortableSerialize;
use crate::domain::{
    ids::Revision,
    sync::{DeviceId, MutationCandidate, MutationId, SharedResourceManifest},
};
use crate::error::Error;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationOperation {
    Upsert,
    Delete,
}

/// Exact persisted local event, including facts absent from a transport DTO.
/// An old event with unknown binding remains evidence, never an upload request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrozenMutation {
    pub candidate: MutationCandidate,
    pub operation: MutationOperation,
    pub local_revision: Revision,
    pub frozen_at_ms: u64,
    pub original_body: String,
    pub original_resources_body: String,
    pub payload_digest: String,
}
impl FrozenMutation {
    /// # Errors
    /// Rejects changed event identities, invalid revisions, or mismatched body.
    pub fn validate(&self, key: &StateKey) -> Result<(), Error> {
        let candidate = &self.candidate;
        if key.target
            != (StateTarget::Mutation {
                author: candidate.origin_device_id,
                mutation_id: candidate.mutation_id,
                body_digest: self.content_hash()?,
            })
            || object_identity(candidate.object).1.is_nil()
            || candidate.origin_seq == 0
            || self.local_revision.as_u64() == 0
            || candidate.previous_mutation_id == Some(candidate.mutation_id)
            || candidate
                .previous_mutation_id
                .is_some_and(|id| id.as_uuid().is_nil())
            || serde_json::from_str::<Value>(&self.original_body).map_err(encode)?
                != candidate.payload
            || serde_json::from_str::<Vec<SharedResourceManifest>>(&self.original_resources_body)
                .map_err(encode)?
                != candidate.resource_refs
        {
            return Err(Error::conflict(
                "frozen mutation key or original body mismatch",
            ));
        }
        digest_valid(&self.payload_digest)?;
        for resource in &candidate.resource_refs {
            crate::domain::sync::SharedResourceManifest::new(
                resource.kind,
                resource.digest,
                resource.length,
                resource.media_type.clone(),
                resource.relative_path.as_str(),
            )?;
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MutationStatus {
    Pending,
    Confirmed,
    Rejected,
}

/// Mutable projection of an event's disposition, without copying its body.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MutationDisposition {
    pub author: DeviceId,
    pub mutation_id: MutationId,
    pub frozen_body_digest: String,
    pub status: MutationStatus,
    pub outcome: Option<Value>,
}
impl MutationDisposition {
    /// # Errors
    /// Rejects a disposition attached to another author/event.
    pub fn validate(&self, key: &StateKey) -> Result<(), Error> {
        if key.target
            != (StateTarget::MutationDisposition {
                author: self.author,
                mutation_id: self.mutation_id,
                body_digest: self.frozen_body_digest.clone(),
            })
        {
            return Err(Error::conflict("mutation disposition key mismatch"));
        }
        Ok(())
    }
}
