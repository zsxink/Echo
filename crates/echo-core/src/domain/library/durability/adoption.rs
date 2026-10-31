//! One persisted reconciliation/adoption record per historical intent (LD-07).
//!
//! An adoption is evidence of a decision, never a permission to resend an old
//! body. It records what the reconciliation could prove about the original
//! frozen event (author, mutation id, body digest, object, frozen change time)
//! and, for an intent the user explicitly keeps, the single new authorized
//! event that now carries the same body under a new binding/author. The old
//! event stays byte-identical in the directory.
//!
//! The record is committed in the same library batch as the replacement
//! event, so an interrupted transition either left both or neither. Re-running
//! it finds the committed record in the replay and reuses its deterministic
//! `new_mutation_id` instead of allocating a second event.
mod decision;
use super::frozen::MutationOperation;
use super::{state::encode, valid_digest, StateKey, StateNamespace, StateTarget};
use crate::domain::sync::{DeviceId, MutationId, SyncBinding};
use crate::error::Error;
pub use decision::{adoption_key, HistoricalIntent, Reconciliation, VerifiedLedgerCommit};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// What the reconciliation could prove about one historical local intent.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum IntentVerdict {
    /// A complete receipt or a self-verifying ledger commit proves the host
    /// applied this event. It must never be applied or uploaded again.
    Accepted,
    /// A complete negative receipt proves the host rejected it for good.
    Terminal,
    /// No trusted evidence exists. The intent stays pending unless the user
    /// explicitly keeps it, at which point exactly one new event is created.
    Unprovable,
}

impl IntentVerdict {
    /// Whether this verdict permits a replacement upload event.
    #[must_use]
    pub const fn replaces_intent(self) -> bool {
        matches!(self, Self::Unprovable)
    }
}

/// Which retained fact proved an `Accepted`/`Terminal` verdict.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdoptionEvidenceKind {
    Receipt,
    Ledger,
}

/// The proof supporting a proven verdict. Absent for `Unprovable`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdoptionEvidence {
    pub kind: AdoptionEvidenceKind,
    /// Ledger or receipt sequence the proof was read from.
    pub sequence: Option<u64>,
    /// Lower-case hex digest of the verified ledger commit, when available.
    pub digest: Option<String>,
}

/// A typed, directory-committed record of one historical intent.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AdoptionState {
    pub old_author: DeviceId,
    pub old_mutation_id: MutationId,
    pub old_body_digest: String,
    pub old_object_type: String,
    pub old_object_uuid: Uuid,
    /// Original frozen change time; a replacement keeps it unchanged.
    pub old_frozen_at_ms: u64,
    /// Original operation, replayed verbatim by the replacement event.
    pub old_operation: MutationOperation,
    /// Original bytes, preserved verbatim so a resent body never changes.
    pub original_body: String,
    pub original_resources_body: String,
    pub verdict: IntentVerdict,
    pub evidence: Option<AdoptionEvidence>,
    /// The user's explicit decision to carry an unprovable intent forward.
    /// False leaves the original event pending under its frozen identity.
    pub user_kept_intent: bool,
    pub new_binding: SyncBinding,
    pub new_author: DeviceId,
    /// Deterministic replacement identity; present exactly when the user keeps
    /// an unprovable intent under this new binding/author.
    pub new_mutation_id: Option<MutationId>,
}

impl AdoptionState {
    /// # Errors
    /// Rejects a mapping that does not belong to its typed key, binding, or
    /// verdict.
    pub fn validate(&self, key: &StateKey) -> Result<(), Error> {
        let StateNamespace::Bound { binding } = key.namespace else {
            return Err(Error::conflict("adoption requires its new binding scope"));
        };
        let proven = self.evidence.is_some();
        if key.target
            != (StateTarget::Adoption {
                old_author: self.old_author,
                old_mutation_id: self.old_mutation_id,
            })
            || binding != self.new_binding
            || self.old_author.as_uuid().is_nil()
            || self.old_mutation_id.as_uuid().is_nil()
            || self.new_author.as_uuid().is_nil()
            || self.old_object_uuid.is_nil()
            || !matches!(
                self.old_object_type.as_str(),
                "song" | "playlist" | "playlist-item" | "library-root" | "favorite" | "override"
                    | "play-stats"
            )
            || !valid_digest(&self.old_body_digest)
            || serde_json::from_str::<Value>(&self.original_body).is_err()
            || serde_json::from_str::<Vec<Value>>(&self.original_resources_body).is_err()
            // A verdict and its proof move together: proven facts never carry a
            // replacement event, and only an explicitly kept unknown intent
            // may be materialized under the new author.
            || proven == self.verdict.replaces_intent()
            // A replacement exists exactly when the user keeps the intent and the
            // object is not an old author's play-stats bucket. Design D5 forbids
            // adopting an unconfirmed bucket as the new author's event: the
            // counts and the old event stay pending under the original author.
            || self.new_mutation_id.is_some()
                != (self.verdict.replaces_intent()
                    && self.user_kept_intent
                    && self.old_object_type != "play-stats")
            || (self.user_kept_intent && !self.verdict.replaces_intent())
            || self
                .new_mutation_id
                .is_some_and(|id| id.as_uuid().is_nil() || id == self.old_mutation_id)
            || self
                .evidence
                .as_ref()
                .is_some_and(|evidence| {
                    evidence
                        .digest
                        .as_deref()
                        .is_some_and(|digest| !valid_digest(digest))
                })
        {
            return Err(Error::conflict("invalid adoption mapping"));
        }
        if let Some(id) = self.new_mutation_id {
            if id != Self::new_mutation_id(key)? {
                return Err(Error::conflict(
                    "adoption replacement event is not the stable transition identity",
                ));
            }
        }
        Ok(())
    }

    /// The deterministic identity of the replacement event.
    ///
    /// # Errors
    /// Reports canonical encoding failures.
    pub fn new_mutation_id(key: &StateKey) -> Result<MutationId, Error> {
        let mut digest = blake3::Hasher::new();
        digest.update(b"echo.sync.adoption-event.v1\0");
        digest.update(key.canonical()?.as_bytes());
        let mut bytes = [0; 16];
        bytes.copy_from_slice(&digest.finalize().as_bytes()[..16]);
        Ok(MutationId::from_uuid(Uuid::from_bytes(bytes)))
    }
}

pub(super) fn validate(key: &StateKey, payload: &Value) -> Result<(), Error> {
    adoption(payload)?.validate(key)
}

/// Read the typed mapping back out of a committed state payload.
///
/// # Errors
/// Rejects a payload without a well-formed adoption mapping.
pub fn adoption(payload: &Value) -> Result<AdoptionState, Error> {
    serde_json::from_value(
        payload
            .get("adoption")
            .cloned()
            .ok_or_else(|| Error::conflict("missing adoption mapping"))?,
    )
    .map_err(encode)
}
