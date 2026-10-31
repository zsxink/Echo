//! A complete confirmation retains the exact original LAN confirmation.
use super::*;
use serde_json::Value;

fn reseal(payload: &mut Value, body: &Value) {
    let raw = serde_json::to_string_pretty(body).unwrap();
    let mut hash = blake3::Hasher::new();
    hash.update(b"echo.lan.commit.v1\0");
    hash.update(&1u16.to_be_bytes());
    hash.update(&9u64.to_be_bytes());
    hash.update(&[7; 32]);
    hash.update(&u32::try_from(raw.len()).unwrap().to_be_bytes());
    hash.update(raw.as_bytes());
    payload["original_body"] = json!(raw);
    payload["digest"] = json!(hash.finalize().to_hex().as_str());
}
fn confirmation() -> (StateKey, Value, Value) {
    let namespace = namespace(1);
    let StateNamespace::Bound { binding } = namespace else {
        unreachable!()
    };
    let original = frozen(namespace.clone());
    let mut mutation = original.payload["frozen"]["candidate"].clone();
    mutation["protocol_version"] = json!(1);
    mutation["binding"] = json!(binding);
    let object = serde_json::from_value(mutation["object"].clone()).unwrap();
    let alias = json!({"kind":"song","alias":{"alias":uuid::Uuid::from_u128(55),"canonical":uuid::Uuid::from_u128(5)}});
    let body = json!({"protocol_version":1,"binding":binding,"commit_seq":9,
        "previous_digest":vec![7;32],"mutation":mutation,"aliases":[alias],"resource_refs":[]});
    let key = StateKey {
        namespace,
        target: StateTarget::Confirmed { object },
    };
    let mut payload = json!({"schema":"echo.sync.confirmed.v2","author":mutation["origin_device_id"],"mutation_id":mutation["mutation_id"],
        "version":9,"sequence":9,"payload":mutation["payload"],"tombstone":false,"changed_at_ms":444,
        "object_type":"song","object_uuid":uuid::Uuid::from_u128(5),"previous_digest":blake3::Hash::from_bytes([7;32]).to_hex().as_str()});
    reseal(&mut payload, &body);
    (key, payload, body)
}
#[test]
fn library_durability_confirmed_v2_round_trips_alias_base_predecessor_and_exact_raw_bytes() {
    let (key, payload, body) = confirmation();
    let state = DurableStateChange::scoped(key, payload).unwrap();
    let decoded: DurableStateChange =
        serde_json::from_str(&serde_json::to_string(&state).unwrap()).unwrap();
    decoded.validate().unwrap();
    assert_eq!(decoded, state);
    let raw = decoded.payload["original_body"].as_str().unwrap();
    assert_eq!(serde_json::from_str::<Value>(raw).unwrap(), body);
    assert!(raw.contains('\n'));
    assert_eq!(body["aliases"].as_array().unwrap().len(), 1);
    assert_eq!(
        body["mutation"]["previous_mutation_id"],
        json!(uuid::Uuid::from_u128(9))
    );
}
#[test]
fn library_durability_confirmed_v2_rejects_incomplete_original_even_when_rehashed() {
    let (key, payload, body) = confirmation();
    for field in ["previous_mutation_id", "base_object_version", "origin_seq"] {
        let mut changed = body.clone();
        changed["mutation"].as_object_mut().unwrap().remove(field);
        let mut invalid = payload.clone();
        reseal(&mut invalid, &changed);
        assert!(
            DurableStateChange::scoped(key.clone(), invalid).is_err(),
            "accepted missing {field}"
        );
    }
    for field in ["aliases", "resource_refs"] {
        let mut changed = body.clone();
        changed.as_object_mut().unwrap().remove(field);
        let mut invalid = payload.clone();
        reseal(&mut invalid, &changed);
        assert!(
            DurableStateChange::scoped(key.clone(), invalid).is_err(),
            "accepted missing {field}"
        );
    }
}

#[test]
fn library_durability_confirmed_v2_malformed_mutation_returns_error_without_panicking() {
    let (key, payload, body) = confirmation();
    for value in [
        json!("invalid"),
        json!(42),
        json!(true),
        json!([]),
        Value::Null,
    ] {
        let mut changed = body.clone();
        changed["mutation"] = value;
        let mut invalid = payload.clone();
        reseal(&mut invalid, &changed);
        assert!(DurableStateChange::scoped(key.clone(), invalid).is_err());
    }
}

#[test]
fn library_durability_confirmed_v2_rejects_invalid_typed_alias_even_when_rehashed() {
    let (key, payload, body) = confirmation();
    for (pointer, value) in [
        ("/aliases/0/kind", json!("unknown")),
        (
            "/aliases/0/alias/canonical",
            json!(uuid::Uuid::from_u128(99)),
        ),
        ("/aliases/0/alias/alias", json!(uuid::Uuid::from_u128(5))),
    ] {
        let mut changed = body.clone();
        *changed.pointer_mut(pointer).unwrap() = value;
        let mut invalid = payload.clone();
        reseal(&mut invalid, &changed);
        assert!(
            DurableStateChange::scoped(key.clone(), invalid).is_err(),
            "accepted changed {pointer}"
        );
    }
}
