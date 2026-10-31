//! New publication evidence is strict; old directory bytes remain evidence.
use super::*;
use crate::domain::sync::{ResourceDigest, SharedResourceKind, SharedResourceManifest};

fn publication() -> (StateKey, serde_json::Value) {
    let root = LibraryRootId::new();
    let operation = uuid::Uuid::new_v4();
    let owner = SongId::new();
    let manifest = SharedResourceManifest::new(
        SharedResourceKind::Audio,
        ResourceDigest::from_bytes([4; 32]),
        12,
        "audio/flac".into(),
        "media/track.flac",
    )
    .unwrap();
    let body = json!({"root":root,"operation":operation,"object":ObjectKey::Song(owner),"items":[{
        "target":{"manifest":manifest,"expected_old_digest":null},"stage":"Planned",
        "staging_path":format!("echo/tmp/{}/0",operation.simple()),"local_path":"media/track.flac","old_path":null}],"completed":false});
    let key = StateKey {
        namespace: StateNamespace::LegacyEvidence { source_root: root },
        target: StateTarget::Publication {
            operation_id: operation,
        },
    };
    (
        key,
        json!({"schema":"echo.sync.resource-publication.v2","operation":operation,"publication":body,"original_body":body.to_string(),"completed":false}),
    )
}

#[test]
fn library_durability_publication_v2_rejects_inconsistent_identity_paths_and_completion() {
    let (key, payload) = publication();
    DurableStateChange::scoped(key.clone(), payload.clone()).unwrap();
    for (pointer, value) in [
        ("/root", json!(LibraryRootId::new())),
        ("/operation", json!(uuid::Uuid::new_v4())),
        (
            "/object",
            json!(ObjectKey::Song(SongId::from_uuid(uuid::Uuid::nil()))),
        ),
        ("/completed", json!(true)),
        ("/items/0/stage", json!("Completed")),
        ("/items/0/local_path", json!("echo/state/confirmed/a.json")),
        (
            "/items/0/staging_path",
            json!(format!("echo/tmp/{}/0", uuid::Uuid::new_v4().simple())),
        ),
        (
            "/items/0/target/manifest/media_type",
            json!("audio/invalid type"),
        ),
        (
            "/items/0/target/manifest/relative_path/normalized",
            json!("../outside.flac"),
        ),
    ] {
        let mut invalid = payload.clone();
        *invalid["publication"].pointer_mut(pointer).unwrap() = value;
        invalid["original_body"] = json!(invalid["publication"].to_string());
        assert!(
            DurableStateChange::scoped(key.clone(), invalid).is_err(),
            "accepted changed {pointer}"
        );
    }
}

#[test]
fn library_durability_publication_v2_requires_complete_original_fields() {
    let (key, payload) = publication();
    for pointer in [
        "",
        "/items/0",
        "/items/0/target",
        "/items/0/target/manifest",
    ] {
        let fields: Vec<_> = payload["publication"]
            .pointer(pointer)
            .unwrap()
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect();
        for field in fields {
            let mut invalid = payload.clone();
            invalid["publication"]
                .pointer_mut(pointer)
                .unwrap()
                .as_object_mut()
                .unwrap()
                .remove(&field);
            invalid["original_body"] = json!(invalid["publication"].to_string());
            assert!(
                DurableStateChange::scoped(key.clone(), invalid).is_err(),
                "accepted missing {pointer}/{field}"
            );
        }
    }
    for field in ["publication", "original_body", "completed"] {
        let mut invalid = payload.clone();
        invalid.as_object_mut().unwrap().remove(field);
        assert!(
            DurableStateChange::scoped(key.clone(), invalid).is_err(),
            "accepted missing {field}"
        );
    }
}

#[test]
fn library_durability_publication_v1_and_unknown_schema_keep_original_evidence() {
    let (key, mut payload) = publication();
    payload["publication"]["items"][0]["local_path"] = json!("unknown historical layout");
    for schema in [
        "echo.sync.resource-publication.v1",
        "echo.sync.resource-publication.v999",
    ] {
        payload["schema"] = json!(schema);
        let state = DurableStateChange::scoped(key.clone(), payload.clone()).unwrap();
        let raw = serde_json::to_string(&state).unwrap();
        let decoded: DurableStateChange = serde_json::from_str(&raw).unwrap();
        decoded.validate().unwrap();
        assert_eq!(raw, serde_json::to_string(&decoded).unwrap());
    }
}

#[test]
fn library_durability_publication_v2_checks_managed_owner_declaration_version_and_original_bytes() {
    let (key, mut payload) = publication();
    let ObjectKey::Song(owner) =
        serde_json::from_value(payload["publication"]["object"].clone()).unwrap()
    else {
        unreachable!()
    };
    let manifest: SharedResourceManifest =
        serde_json::from_value(payload["publication"]["items"][0]["target"]["manifest"].clone())
            .unwrap();
    let declaration = format!(
        "[{{\"manifest\":{},\"expected_old_digest\":null}}]",
        serde_json::to_string(&manifest).unwrap()
    );
    let version = blake3::hash(declaration.as_bytes()).to_hex().to_string();
    let valid = format!("echo/media/song/{owner}/{version}/media/track.flac");
    payload["publication"]["items"][0]["local_path"] = json!(valid);
    payload["original_body"] =
        json!(serde_json::to_string_pretty(&payload["publication"]).unwrap());
    let state = DurableStateChange::scoped(key.clone(), payload.clone()).unwrap();
    let raw = serde_json::to_string(&state).unwrap();
    let decoded: DurableStateChange = serde_json::from_str(&raw).unwrap();
    assert_eq!(decoded, state);
    decoded.validate().unwrap();
    for path in [
        format!(
            "echo/media/song/{}/{version}/media/track.flac",
            SongId::new()
        ),
        format!("echo/media/playlist/{owner}/{version}/media/track.flac"),
        format!(
            "echo/media/song/{owner}/{}/media/track.flac",
            "b".repeat(64)
        ),
        format!("echo/media/song/{owner}/{version}/media/another.flac"),
        "../outside.flac".to_owned(),
        "/absolute.flac".to_owned(),
    ] {
        let mut invalid = payload.clone();
        invalid["publication"]["items"][0]["local_path"] = json!(path);
        invalid["original_body"] = json!(invalid["publication"].to_string());
        assert!(DurableStateChange::scoped(key.clone(), invalid).is_err());
    }
    payload["original_body"] = json!("{}");
    assert!(DurableStateChange::scoped(key, payload).is_err());
}

#[test]
fn library_durability_publication_v2_rejects_normalized_collisions_and_unknown_fields() {
    let (key, payload) = publication();
    let mut duplicate = payload.clone();
    let mut second = duplicate["publication"]["items"][0].clone();
    second["local_path"] = json!("media/TRACK.flac");
    second["target"]["manifest"]["relative_path"]["normalized"] = json!("media/TRACK.flac");
    second["staging_path"] = json!(format!(
        "{}/1",
        payload["publication"]["items"][0]["staging_path"]
            .as_str()
            .unwrap()
            .rsplit_once('/')
            .unwrap()
            .0
    ));
    duplicate["publication"]["items"]
        .as_array_mut()
        .unwrap()
        .push(second);
    duplicate["original_body"] = json!(duplicate["publication"].to_string());
    assert!(DurableStateChange::scoped(key.clone(), duplicate).is_err());
    for pointer in [
        "",
        "/items/0",
        "/items/0/target",
        "/items/0/target/manifest",
        "/items/0/target/manifest/relative_path",
    ] {
        let mut invalid = payload.clone();
        invalid["publication"]
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("future_field".into(), json!(true));
        invalid["original_body"] = json!(invalid["publication"].to_string());
        assert!(
            DurableStateChange::scoped(key.clone(), invalid).is_err(),
            "accepted unknown field in {pointer}"
        );
    }
}
