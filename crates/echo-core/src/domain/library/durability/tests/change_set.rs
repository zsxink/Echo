//! Domain change-set validation against a verified directory image.
use super::super::*;
use crate::domain::library::{PortableRecord, RecordKind};
use serde_json::json;

fn record(kind: &str, fields: serde_json::Value) -> PortableRecordEnvelope {
    let mut value = json!({"type":kind,"revision":1,
        "updated_by_device_id":uuid::Uuid::from_u128(2),
        "hlc":{"wall_secs":1,"counter":0}});
    let serde_json::Value::Object(fields) = fields else {
        panic!("fixture fields must be an object")
    };
    value.as_object_mut().unwrap().extend(fields);
    PortableRecordEnvelope::new(serde_json::from_value(value).unwrap(), None).unwrap()
}
fn playlist() -> PortableRecordEnvelope {
    record(
        "playlist",
        json!({"playlist_uuid":uuid::Uuid::from_u128(3),"display_name":"Empty"}),
    )
}
fn member() -> PortableRecordEnvelope {
    record(
        "playlist_item",
        json!({"item_uuid":uuid::Uuid::from_u128(4),
        "playlist_uuid":uuid::Uuid::from_u128(3),"song_uuid":uuid::Uuid::from_u128(5),"position":7}),
    )
}
fn song() -> PortableRecordEnvelope {
    record(
        "song",
        json!({"song_uuid":uuid::Uuid::from_u128(5),"media_path":crate::domain::library::LibraryRelativePath::new("media/a.flac").unwrap(),
        "content_hash":"a".repeat(64),"added_at":12,"title":"Title","artist":null,"album":null}),
    )
}
fn batch(records: Vec<PortableRecordEnvelope>) -> LibraryBatch {
    LibraryBatch::new(
        uuid::Uuid::from_u128(10),
        LibraryId::from_uuid(uuid::Uuid::from_u128(11)),
        ChangeOrigin::LocalEdit,
        records,
    )
}
fn baseline() -> LibraryCheckpoint {
    LibraryCheckpoint {
        library_id: batch(vec![]).library_id,
        library_commit_seq: 1,
        head_digest: "a".repeat(64),
        records: vec![playlist(), song()],
        state: vec![],
        resources: vec![],
    }
}
#[test]
fn library_durability_change_set_rejects_cross_library_and_wrong_typed_version() {
    let image = baseline();
    let mut change = batch(vec![playlist()]);
    change.validate_against(&image).unwrap();
    change.library_id = LibraryId::from_uuid(uuid::Uuid::from_u128(99));
    assert!(change.validate_against(&image).is_err());
    change.library_id = image.library_id;
    change.expected.push(ExpectedRecord {
        kind: RecordKind::Song,
        object_uuid: uuid::Uuid::from_u128(3),
        content_hash: Some(playlist().content_hash().unwrap()),
    });
    assert!(change.validate_against(&image).is_err());
    change.expected[0].kind = RecordKind::Playlist;
    change.validate_against(&image).unwrap();
    change.expected[0].content_hash = Some("b".repeat(64));
    assert!(change.validate_against(&image).is_err());
}
#[test]
fn library_durability_change_set_rejects_missing_or_deleted_dependencies() {
    let mut image = baseline();
    let change = batch(vec![member()]);
    change.validate_against(&image).unwrap();
    image
        .records
        .retain(|r| !matches!(r.record(), PortableRecord::Song(_)));
    assert!(change.validate_against(&image).is_err());
    let mut complete = batch(vec![member(), song()]);
    complete.validate_against(&image).unwrap();
    complete.records.push(record(
        "tombstone",
        json!({"object_uuid":uuid::Uuid::from_u128(3),"deleted_kind":"playlist"}),
    ));
    assert!(complete.validate_against(&image).is_err());
}
#[test]
fn library_durability_change_set_rejects_duplicate_targets_and_missing_assets() {
    let mut change = batch(vec![playlist(), playlist()]);
    assert!(change.validate_against(&baseline()).is_err());
    let mut value = serde_json::to_value(playlist().into_record()).unwrap();
    value["cover_asset"] = json!({"content_hash":"c".repeat(64),"byte_len":8,"mime":"image/png"});
    change.records =
        vec![PortableRecordEnvelope::new(serde_json::from_value(value).unwrap(), None).unwrap()];
    assert!(change.validate_against(&baseline()).is_err());
    change.resources.push(DurableResource {
        relative_path: format!("echo/assets/{}", "c".repeat(64)),
        content_hash: "c".repeat(64),
        byte_length: 8,
    });
    change.validate_against(&baseline()).unwrap();
}
#[test]
fn library_durability_change_set_rejects_invalid_original_asset_mime() {
    for mime in [
        "",
        "image",
        "image/",
        "/png",
        "image/png\r\nx: y",
        "image/p ng",
    ] {
        let mut value = serde_json::to_value(playlist().into_record()).unwrap();
        value["cover_asset"] = json!({"content_hash":"c".repeat(64),"byte_len":8,"mime":mime});
        let change = batch(vec![PortableRecordEnvelope::new(
            serde_json::from_value(value).unwrap(),
            None,
        )
        .unwrap()]);
        assert!(change.validate().is_err(), "accepted invalid MIME {mime:?}");
    }
}

#[test]
fn library_durability_change_set_requires_canonical_original_asset_hash() {
    for hash in ["C".repeat(64), "g".repeat(64), "c".repeat(63)] {
        let mut value = serde_json::to_value(playlist().into_record()).unwrap();
        value["cover_asset"] = json!({"content_hash":hash,"byte_len":8,"mime":"image/png"});
        let change = batch(vec![PortableRecordEnvelope::new(
            serde_json::from_value(value).unwrap(),
            None,
        )
        .unwrap()]);
        assert!(
            change.validate().is_err(),
            "accepted noncanonical hash {hash}"
        );
    }
}

#[test]
fn library_durability_change_set_round_trip_preserves_identity_and_after_images() {
    let change = batch(vec![member(), song(), playlist()]);
    change.validate_against(&baseline()).unwrap();
    let encoded = change.to_canonical_json().unwrap();
    let decoded: LibraryBatch = serde_json::from_str(&encoded).unwrap();
    assert_eq!(change, decoded);
    assert_eq!(change.digest().unwrap(), decoded.digest().unwrap());
    let mut changed = decoded;
    changed.origin = ChangeOrigin::Recovery;
    assert_ne!(change.digest().unwrap(), changed.digest().unwrap());
    changed = change.clone();
    changed.operation_id = uuid::Uuid::from_u128(100);
    assert_ne!(change.digest().unwrap(), changed.digest().unwrap());
}
#[test]
fn library_durability_change_set_rejects_malformed_predecessor_even_with_matching_digest() {
    for (seq, previous) in [
        (1, Some("a".repeat(64))),
        (2, None),
        (2, Some("bad".into())),
    ] {
        let change = batch(vec![]);
        let commit = LibraryCommit {
            digest: LibraryCommit::calculate_digest(&change, seq, previous.as_deref()).unwrap(),
            batch: change,
            library_commit_seq: seq,
            predecessor_digest: previous,
        };
        assert!(commit.validate().is_err());
    }
}
