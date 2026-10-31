//! Strict receipt bodies do not retroactively invalidate historical evidence.
use super::*;

fn receipt() -> (StateKey, serde_json::Value) {
    let author = DeviceId::from_uuid(uuid::Uuid::from_u128(3));
    let mutation_id = MutationId::from_uuid(uuid::Uuid::from_u128(4));
    let outcome = json!({"mutation_id":mutation_id,
        "outcome_json":"{\"accepted\":false,\"reason\":\"WrongObject\"}","checkpoint":null});
    (
        StateKey {
            namespace: namespace(1),
            target: StateTarget::Receipt {
                author,
                mutation_id,
            },
        },
        json!({"schema":"echo.sync.receipt.v2","author":author,"mutation_id":mutation_id,
            "payload_digest":"a".repeat(64),"outcome":outcome,
            "original_outcome":serde_json::to_string(&outcome).unwrap(),
            "received_at":42,"sequence":null}),
    )
}

#[test]
fn library_durability_receipt_v2_rejects_incomplete_or_inconsistent_outcome() {
    let (key, payload) = receipt();
    DurableStateChange::scoped(key.clone(), payload.clone()).unwrap();
    for field in [
        "original_outcome",
        "payload_digest",
        "received_at",
        "outcome",
        "sequence",
    ] {
        let mut invalid = payload.clone();
        invalid.as_object_mut().unwrap().remove(field);
        assert!(
            DurableStateChange::scoped(key.clone(), invalid).is_err(),
            "accepted missing {field}"
        );
    }
    for (field, value) in [
        ("original_outcome", json!("{}")),
        ("payload_digest", json!("bad")),
        ("sequence", json!(1)),
    ] {
        let mut invalid = payload.clone();
        invalid[field] = value;
        assert!(
            DurableStateChange::scoped(key.clone(), invalid).is_err(),
            "accepted changed {field}"
        );
    }
    let mut missing_checkpoint = payload.clone();
    missing_checkpoint["outcome"]
        .as_object_mut()
        .unwrap()
        .remove("checkpoint");
    missing_checkpoint["original_outcome"] =
        json!(serde_json::to_string(&missing_checkpoint["outcome"]).unwrap());
    assert!(DurableStateChange::scoped(key.clone(), missing_checkpoint).is_err());
    let mut invalid = payload;
    invalid["outcome"]["outcome_json"] = json!("{\"accepted\":true}");
    invalid["original_outcome"] = json!(serde_json::to_string(&invalid["outcome"]).unwrap());
    assert!(DurableStateChange::scoped(key, invalid).is_err());
}

#[test]
fn library_durability_receipt_v2_accepted_checkpoint_matches_its_original_binding() {
    let (key, mut payload) = receipt();
    let StateNamespace::Bound { binding } = key.namespace else {
        panic!()
    };
    payload["sequence"] = json!(7);
    payload["outcome"]["outcome_json"] = json!("{\"accepted\":true}");
    payload["outcome"]["checkpoint"] =
        json!({"binding":binding,"sequence":7,"digest":vec![1_u8; 32]});
    payload["original_outcome"] = json!(serde_json::to_string(&payload["outcome"]).unwrap());
    let state = DurableStateChange::scoped(key.clone(), payload.clone()).unwrap();
    let decoded: DurableStateChange =
        serde_json::from_str(&state.to_canonical_json().unwrap()).unwrap();
    decoded.validate().unwrap();
    assert_eq!(decoded, state);
    for field in [
        "binding",
        "sequence",
        "author",
        "mutation_id",
        "outcome_mutation_id",
    ] {
        let mut invalid = payload.clone();
        match field {
            "binding" => invalid["outcome"]["checkpoint"]["binding"]["epoch"] = json!(2),
            "sequence" => invalid["sequence"] = json!(8),
            "author" => invalid["author"] = json!(DeviceId::new()),
            "mutation_id" => invalid["mutation_id"] = json!(MutationId::new()),
            "outcome_mutation_id" => invalid["outcome"]["mutation_id"] = json!(MutationId::new()),
            _ => unreachable!(),
        }
        invalid["original_outcome"] = json!(serde_json::to_string(&invalid["outcome"]).unwrap());
        assert!(
            DurableStateChange::scoped(key.clone(), invalid).is_err(),
            "accepted changed {field}"
        );
    }
}

#[test]
fn library_durability_receipt_v1_incomplete_bound_evidence_is_preserved() {
    let (key, mut payload) = receipt();
    payload["schema"] = json!("echo.sync.receipt.v1");
    payload.as_object_mut().unwrap().remove("original_outcome");
    let state = DurableStateChange::scoped(key, payload).unwrap();
    let encoded = state.to_canonical_json().unwrap();
    let decoded: DurableStateChange = serde_json::from_str(&encoded).unwrap();
    decoded.validate().unwrap();
    assert_eq!(decoded, state);
    assert!(decoded.payload.get("original_outcome").is_none());
}
