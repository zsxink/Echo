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
    let bytes = super::cover_fixtures::cover_png();
    app.set_playlist_cover(id, Some(bytes.as_slice()), Some("image/png"))
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

/// The command layer now decodes base64 before calling this service, so the
/// Core validation it delegates to must stay intact: a rejected MIME never
/// replaces the saved cover (DAS-R22-S05).
#[test]
fn playlist_cover_rejects_unsupported_mime_without_replacing_the_saved_cover() {
    let fixture = ScanFixture::new();
    seed_songs(&fixture, 1);
    let app = services(&fixture);
    let id =
        PlaylistId::from_str(&app.create_playlist(fixture.root, "cover mime").unwrap()).unwrap();
    let saved = super::cover_fixtures::cover_png();
    app.set_playlist_cover(id, Some(saved.as_slice()), Some("image/png"))
        .unwrap();
    let before = app.playlists().unwrap()[0].cover_key.clone();
    assert!(before.is_some());

    let error = app
        .set_playlist_cover(id, Some(b"not an image"), Some("image/gif"))
        .expect_err("unsupported MIME must be rejected");
    assert_eq!(error.code(), "validation");
    let after = app.playlists().unwrap()[0].cover_key.clone();
    assert_eq!(before, after);
    assert_eq!(
        fixture
            .deps
            .cover_cache
            .get(after.as_deref().unwrap())
            .unwrap()
            .as_deref(),
        Some(saved.as_slice())
    );
}

/// Regression guard: the Core 5 MiB ceiling must survive the payload-encoding
/// change even though the command-layer length precheck rejects over-long
/// base64 first. `DAS-R22-S04` only covers the frontend picker limit; there is
/// no separate spec scenario for the backend refusal, so this test is
/// deliberately not mapped to one.
#[test]
fn playlist_cover_rejects_oversized_payload_without_replacing_the_saved_cover() {
    let fixture = ScanFixture::new();
    seed_songs(&fixture, 1);
    let app = services(&fixture);
    let id =
        PlaylistId::from_str(&app.create_playlist(fixture.root, "cover size").unwrap()).unwrap();
    let saved = super::cover_fixtures::cover_png();
    app.set_playlist_cover(id, Some(saved.as_slice()), Some("image/png"))
        .unwrap();
    let before = app.playlists().unwrap()[0].cover_key.clone();
    assert!(before.is_some());

    let oversized = vec![0u8; 5 * 1024 * 1024 + 1];
    let error = app
        .set_playlist_cover(id, Some(&oversized), Some("image/png"))
        .expect_err("oversized payload must be rejected");
    assert_eq!(error.code(), "validation");
    let after = app.playlists().unwrap()[0].cover_key.clone();
    assert_eq!(before, after);
    assert_eq!(
        fixture
            .deps
            .cover_cache
            .get(after.as_deref().unwrap())
            .unwrap()
            .as_deref(),
        Some(saved.as_slice())
    );
}

/// Issue #132 guard: a payload that only *claims* to be a PNG must be refused
/// at save time, leaving both the saved cover and the playlist name intact.
#[test]
fn playlist_cover_rejects_fake_image_without_replacing_the_saved_cover() {
    let fixture = ScanFixture::new();
    seed_songs(&fixture, 1);
    let app = services(&fixture);
    let id =
        PlaylistId::from_str(&app.create_playlist(fixture.root, "cover content").unwrap()).unwrap();
    let saved = super::cover_fixtures::cover_png();
    app.set_playlist_cover(id, Some(saved.as_slice()), Some("image/png"))
        .unwrap();
    let before = app.playlists().unwrap()[0].cover_key.clone();
    assert!(before.is_some());

    let error = app
        .set_playlist_cover(
            id,
            Some(b"plain text pretending to be a png"),
            Some("image/png"),
        )
        .expect_err("non-image content must be rejected at save time");
    assert_eq!(error.code(), "validation");
    let after = app.playlists().unwrap();
    assert_eq!(after[0].cover_key, before);
    assert_eq!(after[0].name, "cover content");
    assert_eq!(
        fixture
            .deps
            .cover_cache
            .get(after[0].cover_key.as_deref().unwrap())
            .unwrap()
            .as_deref(),
        Some(saved.as_slice())
    );
}
