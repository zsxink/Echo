//! Populated user fields and typed relationships survive each supported layer.
use super::*;
use crate::domain::library::{PortableRecord, PortableRecordDocument};

fn records() -> Vec<serde_json::Value> {
    let id = uuid::Uuid::from_u128;
    let asset = json!({"content_hash":"a".repeat(64),"byte_len":12,"mime":"image/png"});
    vec![
        json!({"type":"song","song_uuid":id(5),"media_path":{"normalized":"media/track.flac"},"content_hash":"b".repeat(64),"added_at":11,"title":"Track","artist":"Artist","album":"Album"}),
        json!({"type":"favorite","song_uuid":id(5),"is_favorite":false}),
        json!({"type":"playlist","playlist_uuid":id(3),"display_name":"Playlist","created_at":22,"cover_asset":asset}),
        json!({"type":"playlist_item","item_uuid":id(4),"playlist_uuid":id(3),"song_uuid":id(5),"position":7,"added_at":33}),
        json!({"type":"override","song_uuid":id(5),"title":"Custom","artist":"Custom artist","album":"Custom album","lyrics_text":"[00:01]Original lyrics"}),
        json!({"type":"play_stats","song_uuid":id(5),"by_device":{id(2).to_string():9}}),
        json!({"type":"tombstone","object_uuid":id(9),"deleted_kind":"playlist_item"}),
    ].into_iter().map(|mut value| {
        value["revision"] = json!(7);
        value["updated_by_device_id"] = json!(id(2));
        value["hlc"] = json!({"wall_secs":1234,"counter":5});
        value
    }).collect()
}

#[test]
fn library_durability_portable_populated_v1_v2_commit_round_trip_preserves_all_user_fields() {
    let original = records();
    let mut envelopes = Vec::new();
    for value in &original {
        let v1: PortableRecordDocument = serde_json::from_value(value.clone()).unwrap();
        assert!(matches!(v1, PortableRecordDocument::V1(_)));
        assert_eq!(serde_json::to_value(&v1).unwrap(), *value);
        let envelope = PortableRecordEnvelope::new(v1.into_record(), None).unwrap();
        let v2: PortableRecordDocument =
            serde_json::from_value(serde_json::to_value(&envelope).unwrap()).unwrap();
        assert!(matches!(v2, PortableRecordDocument::V2(_)));
        assert_eq!(serde_json::to_value(v2.record()).unwrap(), *value);
        assert!(v2.sync_metadata().is_none());
        envelopes.push(envelope);
    }
    let mut batch = LibraryBatch::new(
        uuid::Uuid::new_v4(),
        LibraryId::from_uuid(uuid::Uuid::new_v4()),
        ChangeOrigin::Recovery,
        envelopes,
    );
    batch.resources.push(DurableResource {
        relative_path: format!("echo/assets/{}", "a".repeat(64)),
        content_hash: "a".repeat(64),
        byte_length: 12,
    });
    batch.validate().unwrap();
    let commit = LibraryCommit {
        digest: LibraryCommit::calculate_digest(&batch, 1, None).unwrap(),
        batch,
        library_commit_seq: 1,
        predecessor_digest: None,
    };
    let decoded: LibraryCommit =
        serde_json::from_str(&commit.to_canonical_json().unwrap()).unwrap();
    decoded.validate().unwrap();
    assert_eq!(decoded, commit);
    let PortableRecord::PlaylistItem(member) = decoded.batch.records[3].record() else {
        unreachable!()
    };
    assert_eq!(
        member.playlist_uuid,
        decoded.batch.records[2].record().object_uuid()
    );
    assert_eq!(
        member.song_uuid,
        decoded.batch.records[0].record().object_uuid()
    );
    assert_eq!((member.position, member.added_at), (7, Some(33)));
}
