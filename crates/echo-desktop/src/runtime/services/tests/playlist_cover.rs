//! Playlist originals remain recoverable when application cover caches disappear.
use super::*;

#[test]
fn library_durability_playlist_cover_original_survives_cache_loss_and_clear() {
    use echo_core::domain::library::{PortableRecord, RecordKind};
    let fixture = ScanFixture::new();
    seed_songs(&fixture, 1);
    let app = services(&fixture);
    let id =
        PlaylistId::from_str(&app.create_playlist(fixture.root, "durable cover").unwrap()).unwrap();
    let bytes = b"synthetic original cover";
    app.set_playlist_cover(id, Some(bytes), Some("image/png"))
        .unwrap();
    let record = fixture
        .deps
        .control
        .read_record(fixture.root, RecordKind::Playlist, &id.to_string())
        .unwrap()
        .unwrap();
    let PortableRecord::Playlist(record) = record else {
        panic!("playlist expected")
    };
    let asset = record
        .cover_asset
        .expect("custom cover needs a durable original reference");
    assert_eq!(asset.byte_len, bytes.len() as u64);
    assert_eq!(asset.mime, "image/png");
    let durable = fixture.deps.control.durability().unwrap();
    assert_eq!(
        durable
            .read_asset(fixture.root, &asset.content_hash)
            .unwrap()
            .as_deref(),
        Some(bytes.as_slice())
    );
    let before = app.playlists().unwrap()[0].cover_key.clone().unwrap();
    fixture.deps.cover_cache.gc(&[]).unwrap();
    assert!(fixture.deps.cover_cache.get(&before).unwrap().is_none());
    let after = app.playlists().unwrap()[0].cover_key.clone().unwrap();
    assert_eq!(before, after);
    assert_eq!(
        fixture.deps.cover_cache.get(&after).unwrap().as_deref(),
        Some(bytes.as_slice())
    );
    app.rename_playlist(id, "cover retained").unwrap();
    assert_eq!(
        app.playlists().unwrap()[0].cover_key.as_deref(),
        Some(after.as_str())
    );
    app.set_playlist_cover(id, None, None).unwrap();
    assert!(!app.playlists().unwrap()[0].has_custom_cover);
    fixture.deps.cover_cache.gc(&[]).unwrap();
    assert!(app.playlists().unwrap()[0].cover_key.is_none());
}
