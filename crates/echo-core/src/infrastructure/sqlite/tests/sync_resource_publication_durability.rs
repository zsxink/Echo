//! Portable exports preserve a completed publication's logical song identity.
use super::*;
use crate::domain::library::PortableRecord;

#[test]
fn library_durability_export_uses_declared_logical_path_for_managed_audio() {
    let (_directory, database, root, object, manifest) = fixture();
    let ObjectKey::Song(id) = object else {
        unreachable!()
    };
    let mut published = publication(root, object, manifest.clone(), None);
    let local = format!("echo/media/song/{id}/{}/track.flac", published.operation);
    published.items[0].local_path = local.clone();
    database.prepare(&published).unwrap();
    published.items[0].stage = RemoteResourceStage::Published;
    database.save(&published).unwrap();
    database.finish(&published).unwrap();
    let before = SongRepository::by_id(&database, id).unwrap().unwrap();
    assert_eq!(before.path().normalized(), local);
    let exported = database
        .with_reader(|connection| {
            crate::infrastructure::sqlite::durability_backfill::export_library_records(
                connection, root,
            )
        })
        .unwrap();
    let record = exported
        .iter()
        .find_map(|envelope| match envelope.record() {
            PortableRecord::Song(song) => Some(song),
            _ => None,
        })
        .unwrap();
    assert_eq!(record.song_uuid, id.as_uuid());
    assert_eq!(record.media_path.as_str(), manifest.relative_path.as_str());
    assert_eq!(record.content_hash, manifest.digest.to_hex());
    assert_eq!(
        SongRepository::by_id(&database, id).unwrap().unwrap(),
        before
    );
}

#[test]
fn library_durability_export_does_not_accept_undeclared_control_path() {
    let (_directory, database, root, object, _) = fixture();
    let ObjectKey::Song(id) = object else {
        unreachable!()
    };
    let mut song = SongRepository::by_id(&database, id).unwrap().unwrap();
    song.relink(RelativeMediaPath::new("echo/media/undeclared.flac").unwrap());
    SongRepository::upsert(&database, &song).unwrap();
    assert!(database
        .with_reader(|connection| {
            crate::infrastructure::sqlite::durability_backfill::export_library_records(
                connection, root,
            )
        })
        .is_err());
}
