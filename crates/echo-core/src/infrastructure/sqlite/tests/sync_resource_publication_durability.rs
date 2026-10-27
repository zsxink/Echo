//! Portable exports preserve a completed publication's logical song identity.
use super::*;
use crate::domain::library::PortableRecord;

#[path = "sync_resource_publication_directory.rs"]
mod directory;

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
    assert!(
        database
            .with_reader(|connection| {
                crate::infrastructure::sqlite::durability_backfill::export_library_records(
                    connection, root,
                )
            })
            .is_err()
    );
}

#[test]
fn library_durability_publication_export_preserves_original_managed_declaration_and_completion() {
    let (_directory, database, root, object, manifest) = fixture();
    let mut published = publication(root, object, manifest.clone(), None);
    let targets = vec![published.items[0].target.clone()];
    let version = blake3::hash(&serde_json::to_vec(&targets).unwrap())
        .to_hex()
        .to_string();
    let ObjectKey::Song(id) = object else {
        unreachable!()
    };
    published.items[0].local_path = format!(
        "echo/media/song/{id}/{version}/{}",
        manifest.relative_path.as_str()
    );
    database.prepare(&published).unwrap();
    for completed in [false, true] {
        if completed {
            published.items[0].stage = RemoteResourceStage::Published;
            database.save(&published).unwrap();
            database.finish(&published).unwrap();
        }
        let raw: String = database
            .with_reader(|c| {
                c.query_row(
                    "SELECT publication_json FROM sync_remote_publications WHERE operation_uuid=?1",
                    [published.operation.to_string()],
                    |r| r.get(0),
                )
                .map_err(storage)
            })
            .unwrap();
        let states = database
            .with_reader(|c| {
                crate::infrastructure::sqlite::sync::durability::export_sync_state(c, root)
            })
            .unwrap();
        let state = states
            .iter()
            .find(|s| s.kind == crate::domain::library::durability::StateKind::ResourceLocation)
            .unwrap();
        assert_eq!(state.payload["schema"], "echo.sync.resource-publication.v2");
        assert_eq!(state.payload["original_body"], raw);
        assert_eq!(state.payload["completed"], completed);
        assert_eq!(
            state.payload["publication"]["object"],
            serde_json::to_value(object).unwrap()
        );
        assert_eq!(
            state.payload["publication"]["items"][0]["local_path"],
            published.items[0].local_path
        );
        assert_eq!(
            state.payload["publication"]["items"][0]["target"]["manifest"],
            serde_json::to_value(&manifest).unwrap()
        );
        state.validate().unwrap();
        let round_trip: crate::domain::library::durability::DurableStateChange =
            serde_json::from_str(&serde_json::to_string(state).unwrap()).unwrap();
        assert_eq!(&round_trip, state);
    }
}
