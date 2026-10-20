use super::*;
use crate::application::ports::{ResourceRetentionPort, RetainedResource};
use crate::domain::sync::{ObjectKey, ResourceDigest, SharedResourceKind, SharedResourceManifest};

fn declaration(bytes: &[u8]) -> SharedResourceManifest {
    SharedResourceManifest::new(
        SharedResourceKind::Audio,
        ResourceDigest::from_bytes(*blake3::hash(bytes).as_bytes()),
        bytes.len() as u64,
        "audio/flac".into(),
        "media/history.flac",
    )
    .unwrap()
}

#[test]
fn pending_import_declarations_and_receipts_survive_catalog_deletion_and_restart() {
    let (directory, database, root) = database();
    let song = song(root, "media/history.flac", "History", "Artist");
    let manifest = declaration(b"offline import");
    database
        .set_local_sync_resources(ObjectKey::Song(song.id()), vec![manifest.clone()])
        .unwrap();
    SongRepository::upsert(&database, &song).unwrap();
    let id = song.id();
    database
        .with_tx(Box::new(move |tx| tx.delete_song(id)))
        .unwrap();
    assert!(SongRepository::by_id(&database, id).unwrap().is_none());
    assert!(database
        .local_sync_resource_manifests(root)
        .unwrap()
        .contains(&manifest));
    database.rebuild_resource_holds(root).unwrap();
    let expected = manifest.digest.as_bytes();
    database.with_reader(|connection| {
        let count:i64=connection.query_row("SELECT COUNT(*) FROM sync_resource_holds WHERE local_root_uuid=?1 AND owner_kind='mutation' AND content_hash=?2",params![root.to_string(),expected.as_slice()],|row|row.get(0)).map_err(storage)?;
        assert!(count>0); Ok(())
    }).unwrap();
    let receipt = RetainedResource {
        digest: manifest.digest,
        length: manifest.length,
        path: RelativeMediaPath::new(&format!("echo/media/retained/{}", manifest.digest.to_hex()))
            .unwrap(),
    };
    database
        .record_retained_resource(root, receipt.clone())
        .unwrap();
    let path = directory.path().join("library.sqlite");
    // Fixture uses its own database name; the durable backup exercises opening
    // the exact saved retention rows without depending on fixture internals.
    database.backup_to(&path).unwrap();
    let reopened = SqliteDatabase::open(path).unwrap();
    assert_eq!(
        reopened.retained_resource(root, manifest.digest).unwrap(),
        Some(receipt)
    );
}

#[test]
fn failed_owner_reconstruction_keeps_the_previous_holds_and_blocks_the_caller() {
    let (_directory, database, root) = database();
    let song = song(root, "media/history.flac", "History", "Artist");
    let manifest = declaration(b"offline import");
    database
        .set_local_sync_resources(ObjectKey::Song(song.id()), vec![manifest])
        .unwrap();
    SongRepository::upsert(&database, &song).unwrap();
    database.rebuild_resource_holds(root).unwrap();
    database.writer.run(|connection| {
        connection.execute("UPDATE sync_mutations SET resource_refs_json='[invalid' WHERE status='pending'",[]).map_err(storage)?;Ok(())
    }).unwrap();
    assert!(database.rebuild_resource_holds(root).is_err());
    database
        .with_reader(|connection| {
            let count: i64 = connection
                .query_row(
                    "SELECT COUNT(*) FROM sync_resource_holds WHERE local_root_uuid=?1",
                    params![root.to_string()],
                    |row| row.get(0),
                )
                .map_err(storage)?;
            assert!(count > 0);
            Ok(())
        })
        .unwrap();
}
