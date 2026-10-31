//! Reconciliation reads receipts and the verified ledger before it decides.
//!
//! This module never writes anything. It answers one question per retained
//! historical event: does a complete piece of directory evidence prove the host
//! already applied (or finally rejected) it? Everything else is unprovable and
//! therefore stays pending unless the user explicitly keeps it.

use super::{AdoptionEvidence, AdoptionEvidenceKind, AdoptionState, IntentVerdict};
use crate::domain::library::durability::{
    DurableStateChange, FrozenMutation, StateKey, StateNamespace, StateTarget,
};
use crate::domain::sync::{DeviceId, MutationId, SyncBinding};
use crate::error::Error;
use serde_json::Value;

/// A ledger entry whose complete chain was verified for one binding.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VerifiedLedgerCommit {
    pub binding: SyncBinding,
    pub mutation_id: MutationId,
    pub digest: [u8; 32],
    pub sequence: u64,
}

/// One retained historical local event, with the exact bytes the directory
/// stored. `frozen` is the original body and is never rewritten.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HistoricalIntent {
    pub key: StateKey,
    pub frozen: FrozenMutation,
}

impl HistoricalIntent {
    /// # Errors
    /// Rejects a state that is not a scoped frozen local mutation.
    pub fn from_state(state: &DurableStateChange) -> Result<Option<Self>, Error> {
        let Some(key) = state.key.clone() else {
            return Ok(None);
        };
        if !matches!(key.target, StateTarget::Mutation { .. }) {
            return Ok(None);
        }
        state.validate()?;
        let frozen: FrozenMutation = serde_json::from_value(state.payload["frozen"].clone())
            .map_err(|error| {
                Error::unavailable_with_source(
                    "adoption decision",
                    "historical intent has no frozen body",
                    error,
                )
            })?;
        Ok(Some(Self { key, frozen }))
    }

    #[must_use]
    pub const fn author(&self) -> DeviceId {
        self.frozen.candidate.origin_device_id
    }

    #[must_use]
    pub const fn mutation_id(&self) -> MutationId {
        self.frozen.candidate.mutation_id
    }

    /// The object kind the event addressed, in the storage vocabulary.
    #[must_use]
    pub const fn object_type(&self) -> (&'static str, uuid::Uuid) {
        super::super::state::object_identity(self.frozen.candidate.object)
    }

    /// The old event's immutable body digest, as its key recorded it.
    #[must_use]
    pub fn body_digest(&self) -> Option<&str> {
        match &self.key.target {
            StateTarget::Mutation { body_digest, .. } => Some(body_digest),
            _ => None,
        }
    }
}

/// Evidence usable by a single reconciliation pass.
pub struct Reconciliation<'a> {
    /// Every retained fact in this root, in directory order.
    pub states: &'a [DurableStateChange],
    /// Current confirmed objects, aliases, and tombstones already reconciled
    /// against the new authority's anchored snapshot. Unverified directory JSON
    /// must not be passed here.
    pub authoritative_state: &'a [DurableStateChange],
    /// Host commits whose canonical body verified against the anchor chain for
    /// their recorded binding. A matching entry proves acceptance even when
    /// the original ACK was lost.
    pub ledger: &'a [VerifiedLedgerCommit],
    /// The binding the new authority event will be written under.
    pub new_binding: SyncBinding,
    /// The device that is performing the explicit re-pairing right now.
    pub new_author: DeviceId,
}

impl Reconciliation<'_> {
    /// Decide one historical intent without trusting any status string.
    ///
    /// # Errors
    /// Reports malformed evidence instead of silently downgrading it.
    pub fn verdict(
        &self,
        intent: &HistoricalIntent,
    ) -> Result<(IntentVerdict, Option<AdoptionEvidence>), Error> {
        if let Some(evidence) = self.receipt(intent)? {
            return Ok(evidence);
        }
        Ok(self.ledger_verdict(intent))
    }

    fn receipt(
        &self,
        intent: &HistoricalIntent,
    ) -> Result<Option<(IntentVerdict, Option<AdoptionEvidence>)>, Error> {
        let (author, mutation_id) = (intent.author(), intent.mutation_id());
        let old_binding = match intent.key.namespace {
            StateNamespace::Bound { binding } => Some(binding),
            StateNamespace::Library { .. }
            | StateNamespace::LocalUnbound { .. }
            | StateNamespace::LegacyEvidence { .. } => None,
        };
        for state in self.states.iter().filter(|state| {
            matches!(
                state.key.as_ref().map(|key| &key.target),
                Some(StateTarget::Receipt { author: a, mutation_id: m })
                    if *a == author && *m == mutation_id
            )
        }) {
            // Receipts are only evidence when the strict v2 shape validates and
            // the receipt belongs to the frozen event's original binding. The
            // new binding is intentionally unrelated after a re-pair.
            state.validate()?;
            let Some(expected_binding) = old_binding else {
                continue;
            };
            let Some(key) = &state.key else { continue };
            if !matches!(key.namespace, StateNamespace::Bound { binding } if binding == expected_binding)
            {
                continue;
            }
            let Some(sequence) = state.payload.get("sequence").and_then(Value::as_u64) else {
                // A negative receipt legitimately carries no sequence; anything
                // else is incomplete and therefore not proof.
                if rejected(&state.payload) {
                    return Ok(Some((
                        IntentVerdict::Terminal,
                        Some(AdoptionEvidence {
                            kind: AdoptionEvidenceKind::Receipt,
                            sequence: None,
                            digest: None,
                        }),
                    )));
                }
                continue;
            };
            return Ok(Some((
                IntentVerdict::Accepted,
                Some(AdoptionEvidence {
                    kind: AdoptionEvidenceKind::Receipt,
                    sequence: Some(sequence),
                    digest: None,
                }),
            )));
        }
        Ok(None)
    }

    fn ledger_verdict(
        &self,
        intent: &HistoricalIntent,
    ) -> (IntentVerdict, Option<AdoptionEvidence>) {
        let mutation_id = intent.mutation_id();
        let old_binding = match intent.key.namespace {
            StateNamespace::Bound { binding } => Some(binding),
            StateNamespace::Library { .. }
            | StateNamespace::LocalUnbound { .. }
            | StateNamespace::LegacyEvidence { .. } => None,
        };
        self.ledger
            .iter()
            .filter(|entry| entry.mutation_id == mutation_id && Some(entry.binding) == old_binding)
            .min_by_key(|entry| entry.sequence)
            .map_or((IntentVerdict::Unprovable, None), |entry| {
                (
                    IntentVerdict::Accepted,
                    Some(AdoptionEvidence {
                        kind: AdoptionEvidenceKind::Ledger,
                        sequence: Some(entry.sequence),
                        digest: Some(blake3::Hash::from(entry.digest).to_hex().to_string()),
                    }),
                )
            })
    }

    /// Build the record for one intent. `keep` is the user's explicit decision
    /// to preserve an unprovable intent as a new authorized event.
    ///
    /// # Errors
    /// Rejects a `keep` request on an intent the evidence already resolved, and
    /// malformed original bodies.
    pub fn record(&self, intent: &HistoricalIntent, keep: bool) -> Result<AdoptionState, Error> {
        let (verdict, evidence) = self.verdict(intent)?;
        let keeps = keep && verdict == IntentVerdict::Unprovable;
        if keeps {
            self.check_authoritative_state(intent)?;
        }
        let (object_type, object_uuid) = intent.object_type();
        let body_digest = intent
            .body_digest()
            .ok_or_else(|| Error::conflict("historical intent has no body digest"))?
            .to_owned();
        let key = adoption_key(self.new_binding, intent);
        let mut record = AdoptionState {
            old_author: intent.author(),
            old_mutation_id: intent.mutation_id(),
            old_body_digest: body_digest,
            old_object_type: object_type.to_owned(),
            old_object_uuid: object_uuid,
            old_frozen_at_ms: intent.frozen.frozen_at_ms,
            old_operation: intent.frozen.operation,
            original_body: intent.frozen.original_body.clone(),
            original_resources_body: intent.frozen.original_resources_body.clone(),
            verdict: if keeps {
                IntentVerdict::Unprovable
            } else {
                verdict
            },
            evidence,
            user_kept_intent: keeps,
            new_binding: self.new_binding,
            new_author: self.new_author,
            new_mutation_id: None,
        };
        if keeps && object_type != "play-stats" {
            // D5: a play-stats intent is never replaced by the new author's
            // event, so only the non-stat kinds receive a new identity here.
            record.new_mutation_id = Some(AdoptionState::new_mutation_id(&key)?);
        }
        record.validate(&key)?;
        Ok(record)
    }

    fn check_authoritative_state(&self, intent: &HistoricalIntent) -> Result<(), Error> {
        let object = intent.frozen.candidate.object;
        for state in self.authoritative_state {
            let Some(key) = &state.key else { continue };
            if !matches!(key.namespace, StateNamespace::Bound { binding } if binding == self.new_binding)
            {
                continue;
            }
            state.validate()?;
            match &key.target {
                StateTarget::Confirmed { object: confirmed }
                    if *confirmed == object
                        && state.payload.get("tombstone").and_then(Value::as_bool)
                            == Some(true) =>
                {
                    return Err(Error::conflict(
                        "authoritative tombstone prevents adoption of the historical intent",
                    ));
                }
                StateTarget::SongAlias { alias } if matches!(object, crate::domain::sync::ObjectKey::Song(id) | crate::domain::sync::ObjectKey::Favorite(id) | crate::domain::sync::ObjectKey::Override(id) | crate::domain::sync::ObjectKey::PlayStats(id) if id == *alias) =>
                {
                    return Err(Error::conflict(
                        "authoritative song alias requires explicit conflict handling",
                    ));
                }
                StateTarget::MemberAlias { alias, .. } if matches!(object, crate::domain::sync::ObjectKey::PlaylistItem(id) if id == *alias) =>
                {
                    return Err(Error::conflict(
                        "authoritative member alias requires explicit conflict handling",
                    ));
                }
                _ => {}
            }
        }
        Ok(())
    }

    /// Build the typed directory fact for an explicit reconciliation choice.
    /// The caller must include this fact in the same `LibraryBatch` as any
    /// associated user data before projecting it into SQLite.
    ///
    /// # Errors
    /// Rejects malformed evidence or an invalid adoption record.
    pub fn state_change(
        &self,
        intent: &HistoricalIntent,
        keep: bool,
    ) -> Result<DurableStateChange, Error> {
        let key = adoption_key(self.new_binding, intent);
        let record = self.record(intent, keep)?;
        DurableStateChange::scoped(
            key,
            serde_json::json!({"schema":"echo.sync.adoption.v1","adoption":record}),
        )
    }
}

fn rejected(payload: &Value) -> bool {
    payload
        .get("outcome")
        .and_then(|outcome| outcome.get("outcome_json"))
        .and_then(Value::as_str)
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .and_then(|outcome| outcome.get("accepted").and_then(Value::as_bool))
        == Some(false)
}

/// The stable key of the adoption record for one old event under a new binding.
#[must_use]
pub const fn adoption_key(binding: SyncBinding, intent: &HistoricalIntent) -> StateKey {
    StateKey {
        namespace: StateNamespace::Bound { binding },
        target: StateTarget::Adoption {
            old_author: intent.author(),
            old_mutation_id: intent.mutation_id(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::{
        ids::{Revision, SongId},
        library::{durability::MutationOperation, PortableSerialize},
        sync::{AuthorityId, Epoch, LibraryId, ObjectKey},
    };
    use serde_json::json;

    fn binding(authority: AuthorityId) -> SyncBinding {
        SyncBinding {
            library_id: LibraryId::from_uuid(uuid::Uuid::from_u128(101)),
            authority_id: authority,
            epoch: Epoch::INITIAL,
        }
    }

    fn intent(namespace: StateNamespace, object: ObjectKey) -> HistoricalIntent {
        let author = DeviceId::from_uuid(uuid::Uuid::from_u128(1));
        let mutation_id = MutationId::from_uuid(uuid::Uuid::from_u128(2));
        let payload = json!({"by_device": {author.as_uuid().to_string(): 5}});
        let frozen = FrozenMutation {
            candidate: crate::domain::sync::MutationCandidate {
                mutation_id,
                origin_device_id: author,
                origin_seq: 4,
                previous_mutation_id: None,
                object,
                base_object_version: None,
                payload: payload.clone(),
                resource_refs: vec![],
            },
            operation: MutationOperation::Upsert,
            local_revision: Revision::from_u64(3),
            frozen_at_ms: 1234,
            original_body: serde_json::to_string(&payload).unwrap(),
            original_resources_body: "[]".into(),
            payload_digest: "a".repeat(64),
        };
        let key = StateKey {
            namespace,
            target: StateTarget::Mutation {
                author,
                mutation_id,
                body_digest: frozen.content_hash().unwrap(),
            },
        };
        let state = DurableStateChange::scoped(
            key,
            json!({"schema":"echo.sync.frozen-mutation.v2","frozen":frozen}),
        )
        .unwrap();
        HistoricalIntent::from_state(&state).unwrap().unwrap()
    }

    #[test]
    fn library_durability_adoption_only_trusts_evidence_from_the_frozen_binding() {
        let old_binding = binding(AuthorityId::from_uuid(uuid::Uuid::from_u128(11)));
        let new_binding = binding(AuthorityId::from_uuid(uuid::Uuid::from_u128(12)));
        let old = intent(
            StateNamespace::Bound {
                binding: old_binding,
            },
            ObjectKey::Favorite(SongId::from_uuid(uuid::Uuid::from_u128(55))),
        );
        let wrong_binding = VerifiedLedgerCommit {
            binding: new_binding,
            mutation_id: old.mutation_id(),
            digest: [9; 32],
            sequence: 8,
        };
        let reconciliation = Reconciliation {
            states: &[],
            authoritative_state: &[],
            ledger: &[wrong_binding],
            new_binding,
            new_author: DeviceId::from_uuid(uuid::Uuid::from_u128(22)),
        };
        assert_eq!(
            reconciliation.verdict(&old).unwrap().0,
            IntentVerdict::Unprovable
        );

        let trusted = VerifiedLedgerCommit {
            binding: old_binding,
            ..wrong_binding
        };
        let reconciliation = Reconciliation {
            states: &[],
            authoritative_state: &[],
            ledger: &[trusted],
            new_binding,
            new_author: DeviceId::from_uuid(uuid::Uuid::from_u128(22)),
        };
        let (verdict, evidence) = reconciliation.verdict(&old).unwrap();
        assert_eq!(verdict, IntentVerdict::Accepted);
        assert_eq!(evidence.unwrap().sequence, Some(8));
    }

    #[test]
    fn library_durability_adoption_uses_old_receipt_after_repair_and_rejects_new_scope() {
        let old_binding = binding(AuthorityId::from_uuid(uuid::Uuid::from_u128(11)));
        let new_binding = binding(AuthorityId::from_uuid(uuid::Uuid::from_u128(12)));
        let old = intent(
            StateNamespace::Bound {
                binding: old_binding,
            },
            ObjectKey::Favorite(SongId::from_uuid(uuid::Uuid::from_u128(55))),
        );
        let outcome = json!({"accepted": true});
        let receipt_state = |receipt_binding| {
            let confirmation = json!({
                "mutation_id": old.mutation_id(),
                "outcome_json": serde_json::to_string(&outcome).unwrap(),
                "checkpoint": {
                    "binding": receipt_binding,
                    "sequence": 9,
                    "digest": vec![7_u8; 32],
                }
            });
            let key = StateKey {
                namespace: StateNamespace::Bound {
                    binding: receipt_binding,
                },
                target: StateTarget::Receipt {
                    author: old.author(),
                    mutation_id: old.mutation_id(),
                },
            };
            let payload = json!({
                "schema": "echo.sync.receipt.v2",
                "author": old.author(),
                "mutation_id": old.mutation_id(),
                "payload_digest": "b".repeat(64),
                "outcome": confirmation,
                "original_outcome": serde_json::to_string(&confirmation).unwrap(),
                "sequence": 9,
                "received_at": 100,
            });
            let checkpoint = serde_json::from_value::<crate::domain::sync::CommitCheckpoint>(
                payload["outcome"]["checkpoint"].clone(),
            );
            assert!(checkpoint.is_ok(), "{checkpoint:?}");
            let outcome_json =
                serde_json::from_str::<Value>(payload["outcome"]["outcome_json"].as_str().unwrap());
            assert_eq!(outcome_json.unwrap()["accepted"], true);
            assert_eq!(
                serde_json::from_str::<Value>(payload["original_outcome"].as_str().unwrap())
                    .unwrap(),
                payload["outcome"]
            );
            DurableStateChange::scoped(key, payload).unwrap()
        };

        let old_receipt = receipt_state(old_binding);
        let reconciliation = Reconciliation {
            states: &[old_receipt],
            authoritative_state: &[],
            ledger: &[],
            new_binding,
            new_author: DeviceId::from_uuid(uuid::Uuid::from_u128(22)),
        };
        assert_eq!(
            reconciliation.verdict(&old).unwrap().0,
            IntentVerdict::Accepted
        );

        // A receipt that is internally valid for the *new* binding cannot be
        // used to resolve an event frozen under the old one.
        let new_receipt = receipt_state(new_binding);
        let reconciliation = Reconciliation {
            states: &[new_receipt],
            authoritative_state: &[],
            ledger: &[],
            new_binding,
            new_author: DeviceId::from_uuid(uuid::Uuid::from_u128(22)),
        };
        assert_eq!(
            reconciliation.verdict(&old).unwrap().0,
            IntentVerdict::Unprovable
        );
    }

    #[test]
    fn library_durability_adoption_keeps_unprovable_play_stats_under_old_author() {
        let old_binding = binding(AuthorityId::from_uuid(uuid::Uuid::from_u128(11)));
        let new_binding = binding(AuthorityId::from_uuid(uuid::Uuid::from_u128(12)));
        let old_author = DeviceId::from_uuid(uuid::Uuid::from_u128(1));
        let new_author = DeviceId::from_uuid(uuid::Uuid::from_u128(22));
        let song = SongId::from_uuid(uuid::Uuid::from_u128(55));
        let old = intent(
            StateNamespace::Bound {
                binding: old_binding,
            },
            ObjectKey::PlayStats(song),
        );
        let reconciliation = Reconciliation {
            states: &[],
            authoritative_state: &[],
            ledger: &[],
            new_binding,
            new_author,
        };
        let first = reconciliation.record(&old, true).unwrap();
        let second = reconciliation.record(&old, true).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.new_mutation_id, None);
        assert_eq!(first.old_frozen_at_ms, 1234);
        assert_eq!(first.old_author, old_author);

        let state = reconciliation.state_change(&old, true).unwrap();
        assert_eq!(
            state.payload["adoption"],
            serde_json::to_value(first).unwrap()
        );
        let persisted = super::super::adoption(&state.payload).unwrap();
        assert_eq!(persisted.old_object_uuid, song.as_uuid());
        assert_eq!(persisted.new_author, new_author);
        assert_eq!(persisted.new_mutation_id, None);

        let not_kept = reconciliation.state_change(&old, false).unwrap();
        let not_kept = super::super::adoption(&not_kept.payload).unwrap();
        assert!(!not_kept.user_kept_intent);
        assert_eq!(not_kept.new_mutation_id, None);
    }

    #[test]
    fn library_durability_adoption_refuses_authoritative_alias_conflicts() {
        let old_binding = binding(AuthorityId::from_uuid(uuid::Uuid::from_u128(11)));
        let new_binding = binding(AuthorityId::from_uuid(uuid::Uuid::from_u128(12)));
        let song = SongId::from_uuid(uuid::Uuid::from_u128(55));
        let old = intent(
            StateNamespace::Bound {
                binding: old_binding,
            },
            ObjectKey::Favorite(song),
        );
        let alias = DurableStateChange::scoped(
            StateKey {
                namespace: StateNamespace::Bound {
                    binding: new_binding,
                },
                target: StateTarget::SongAlias { alias: song },
            },
            json!({"alias": song}),
        )
        .unwrap();
        let reconciliation = Reconciliation {
            states: &[],
            authoritative_state: &[alias],
            ledger: &[],
            new_binding,
            new_author: DeviceId::from_uuid(uuid::Uuid::from_u128(22)),
        };

        assert!(reconciliation.record(&old, true).is_err());
        assert!(reconciliation.record(&old, false).is_ok());
    }
}
