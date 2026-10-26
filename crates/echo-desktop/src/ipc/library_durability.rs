//! Stable, user-visible library persistence outcomes at the IPC boundary.
use serde::{Deserialize, Serialize};

/// Machine-readable reason for an incomplete persistence or recovery outcome.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LibraryDurabilityReasonDto {
    OperationRejected,
    RecoveryRequired,
    ProjectionPending,
    UnresolvedProjection,
    LegacyUnverified,
}

/// The durable state of a library operation. `operationId` is retained for
/// states that refer to one operation so callers can resume the same intent.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    tag = "kind"
)]
pub enum LibraryDurabilityOutcomeDto {
    NotSaved {
        reason: LibraryDurabilityReasonDto,
    },
    SavedPendingRecovery {
        operation_id: Option<String>,
        reason: LibraryDurabilityReasonDto,
    },
    SavedPendingProjection {
        operation_id: Option<String>,
        reason: LibraryDurabilityReasonDto,
    },
    Complete {
        operation_id: Option<String>,
    },
    RecoveryPartial {
        reason: LibraryDurabilityReasonDto,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_outcome_has_stable_tag_identity_and_classified_reason() {
        let outcomes = [
            LibraryDurabilityOutcomeDto::NotSaved {
                reason: LibraryDurabilityReasonDto::OperationRejected,
            },
            LibraryDurabilityOutcomeDto::SavedPendingRecovery {
                operation_id: Some("op-1".into()),
                reason: LibraryDurabilityReasonDto::RecoveryRequired,
            },
            LibraryDurabilityOutcomeDto::SavedPendingProjection {
                operation_id: Some("op-1".into()),
                reason: LibraryDurabilityReasonDto::ProjectionPending,
            },
            LibraryDurabilityOutcomeDto::Complete {
                operation_id: Some("op-1".into()),
            },
            LibraryDurabilityOutcomeDto::RecoveryPartial {
                reason: LibraryDurabilityReasonDto::UnresolvedProjection,
            },
        ];
        let values: Vec<_> = outcomes
            .iter()
            .map(|outcome| serde_json::to_value(outcome).unwrap())
            .collect();
        assert_eq!(
            values
                .iter()
                .map(|value| value["kind"].as_str().unwrap())
                .collect::<Vec<_>>(),
            vec![
                "notSaved",
                "savedPendingRecovery",
                "savedPendingProjection",
                "complete",
                "recoveryPartial"
            ]
        );
        // Each state carries the reason that classified it; a state without a
        // reason must not invent one.
        assert_eq!(values[0]["reason"], "operationRejected");
        assert_eq!(values[1]["reason"], "recoveryRequired");
        assert_eq!(values[2]["reason"], "projectionPending");
        assert_eq!(values[4]["reason"], "unresolvedProjection");
        assert!(values[0].get("reason").is_some());
        assert!(values[3].get("reason").is_none(), "complete has no reason");
        // Operation identity survives for every state that names one, so the
        // UI can resume the same intent rather than start a second one.
        assert_eq!(values[1]["operationId"], "op-1");
        assert_eq!(values[2]["operationId"], "op-1");
        assert_eq!(values[3]["operationId"], "op-1");
        assert!(values[0].get("operationId").is_none());
        assert!(values[4].get("operationId").is_none());
        // Every state round-trips byte-for-byte through serde.
        for (value, outcome) in values.iter().zip(&outcomes) {
            assert_eq!(
                &serde_json::from_value::<LibraryDurabilityOutcomeDto>(value.clone()).unwrap(),
                outcome
            );
        }
    }

    /// The generated TypeScript declares `operationId: string | null` as a
    /// required field on `complete`, so serde must emit an explicit `null`
    /// rather than omitting the key — otherwise the generated type would
    /// disagree with the wire shape for a status probe that has no operation.
    #[test]
    fn absent_operation_identity_serializes_as_explicit_null() {
        let value =
            serde_json::to_value(LibraryDurabilityOutcomeDto::Complete { operation_id: None })
                .unwrap();
        assert_eq!(value["kind"], "complete");
        assert!(value["operationId"].is_null());
        assert_eq!(
            serde_json::from_value::<LibraryDurabilityOutcomeDto>(value).unwrap(),
            LibraryDurabilityOutcomeDto::Complete { operation_id: None }
        );
    }
}
