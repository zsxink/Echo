use crate::domain::sync::{ObjectKey, ResourceDigest, SharedResourceKind, SharedResourceManifest};

fn manifest(path: &str, digest: u8) -> SharedResourceManifest {
    SharedResourceManifest::new(
        SharedResourceKind::Audio,
        ResourceDigest::from_bytes([digest; 32]),
        128,
        "audio/flac".into(),
        path,
    )
    .unwrap()
}

#[test]
fn manifest_validation_rejects_duplicates_and_excessive_declarations() {
    let first = manifest("media/one.flac", 1);
    let duplicate_path = manifest("media/one.flac", 2);
    assert!(super::sync_manifests::validate(std::slice::from_ref(&first)).is_ok());
    assert!(super::sync_manifests::validate(&[first, duplicate_path]).is_err());

    let too_many = (0..33)
        .map(|index| manifest(&format!("media/{index}.flac"), u8::try_from(index).unwrap()))
        .collect::<Vec<_>>();
    assert!(super::sync_manifests::validate(&too_many).is_err());
}

#[test]
fn wire_manifest_requires_canonical_lowercase_hash_and_safe_metadata() {
    let valid = serde_json::json!({
        "resource_refs": [{
            "kind": "audio",
            "content_hash": "01".repeat(32),
            "length": 128,
            "media_type": "audio/flac",
            "relative_path": "media/one.flac"
        }]
    });
    assert_eq!(
        super::sync_manifests::from_commit_json(&valid.to_string())
            .unwrap()
            .len(),
        1
    );

    let uppercase = serde_json::json!({
        "resource_refs": [{
            "kind": "audio",
            "content_hash": "AB".repeat(32),
            "length": 128,
            "media_type": "audio/flac",
            "relative_path": "media/one.flac"
        }]
    });
    assert!(super::sync_manifests::from_commit_json(&uppercase.to_string()).is_err());

    let unsafe_path = serde_json::json!({
        "resource_refs": [{
            "kind": "audio",
            "content_hash": "01".repeat(32),
            "length": 128,
            "media_type": "audio/flac",
            "relative_path": "../outside.flac"
        }]
    });
    assert!(super::sync_manifests::from_commit_json(&unsafe_path.to_string()).is_err());
}

#[test]
fn every_sync_object_kind_has_a_stable_manifest_identity_namespace() {
    let song = crate::domain::ids::SongId::new();
    let playlist = crate::domain::ids::PlaylistId::new();
    let item = crate::domain::ids::PlaylistItemId::new();
    assert_eq!(
        super::sync_manifests::identity(ObjectKey::Song(song)).0,
        "song"
    );
    assert_eq!(
        super::sync_manifests::identity(ObjectKey::Playlist(playlist)).0,
        "playlist"
    );
    assert_eq!(
        super::sync_manifests::identity(ObjectKey::PlaylistItem(item)).0,
        "playlist-item"
    );
    assert_eq!(
        super::sync_manifests::identity(ObjectKey::LibraryRoot(uuid::Uuid::new_v4())).0,
        "library-root"
    );
    assert_eq!(
        super::sync_manifests::identity(ObjectKey::Favorite(song)).0,
        "favorite"
    );
    assert_eq!(
        super::sync_manifests::identity(ObjectKey::Override(song)).0,
        "override"
    );
    assert_eq!(
        super::sync_manifests::identity(ObjectKey::PlayStats(song)).0,
        "play-stats"
    );
}
