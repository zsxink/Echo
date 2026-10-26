//! Artwork identity and original declarations are independent of cache state.
use super::*;
use crate::domain::library::PortableAssetReference;

#[test]
fn library_durability_artist_cover_validates_stable_identity_library_and_original() {
    let library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let asset = PortableAssetReference {
        content_hash: "a".repeat(64),
        byte_len: 7,
        mime: "image/png".to_owned(),
    };
    let artwork = ArtistCoverState::new(library, "歌手".into(), Some(asset.clone())).unwrap();
    let change = artwork.clone().into_change().unwrap();
    assert_eq!(ArtistCoverState::from_change(&change).unwrap(), artwork);
    let clear = ArtistCoverState::new(library, "歌手".into(), None).unwrap();
    assert_eq!(clear.id(), change.id);
    assert_ne!(
        ArtistCoverState::new(
            LibraryId::from_uuid(uuid::Uuid::new_v4()),
            "歌手".into(),
            None
        )
        .unwrap()
        .id(),
        change.id
    );
    let mut invalid = change.clone();
    invalid.payload["artist_key"] = json!("another artist");
    assert!(invalid.validate().is_err());
    assert!(ArtistCoverState::new(library, " ARTIST ".into(), None).is_err());
    let mut batch = LibraryBatch::new(
        uuid::Uuid::new_v4(),
        library,
        ChangeOrigin::LocalEdit,
        vec![],
    );
    batch.state.push(change);
    assert!(
        batch.validate().is_err(),
        "the original must be declared in this commit"
    );
    batch.resources.push(DurableResource {
        relative_path: format!("echo/assets/{}", asset.content_hash),
        content_hash: asset.content_hash,
        byte_length: asset.byte_len,
    });
    batch.validate().unwrap();
    batch.library_id = LibraryId::from_uuid(uuid::Uuid::new_v4());
    assert!(
        batch.validate().is_err(),
        "private artwork cannot cross libraries"
    );
    batch.library_id = library;
    batch.state = vec![clear.into_change().unwrap()];
    batch.resources.clear();
    batch.validate().unwrap();
}
