//! Artist artwork uses Core durability and regenerates disposable cache bytes.
use super::*;

#[test]
fn library_durability_artist_cover_original_survives_cache_loss_replace_and_clear() {
    let fixture = ScanFixture::new();
    seed_songs(&fixture, 1);
    let app = services(&fixture);
    let artist = app.collections(CatalogCollectionKind::Artist, "").unwrap()[0]
        .artist_key
        .clone();
    let bytes = b"synthetic artist artwork";
    app.set_artist_cover(&artist, Some(bytes), Some("image/png"))
        .unwrap();
    let before = app.collections(CatalogCollectionKind::Artist, "").unwrap()[0]
        .cover_key
        .clone()
        .unwrap();
    fixture.deps.cover_cache.gc(&[]).unwrap();
    assert!(fixture.deps.cover_cache.get(&before).unwrap().is_none());
    let restored = app.collections(CatalogCollectionKind::Artist, "").unwrap();
    assert!(restored[0].has_custom_cover);
    assert_eq!(restored[0].cover_key.as_deref(), Some(before.as_str()));
    assert_eq!(
        fixture.deps.cover_cache.get(&before).unwrap().as_deref(),
        Some(bytes.as_slice())
    );
    app.set_artist_cover(&artist, Some(b"replacement"), Some("image/png"))
        .unwrap();
    let replaced = app.collections(CatalogCollectionKind::Artist, "").unwrap();
    assert_ne!(replaced[0].cover_key.as_deref(), Some(before.as_str()));
    app.set_artist_cover(&artist, None, None).unwrap();
    let cleared = app.collections(CatalogCollectionKind::Artist, "").unwrap();
    assert!(!cleared[0].has_custom_cover);
    assert!(cleared[0].cover_key.is_none());
}
