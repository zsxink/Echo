use super::*;
use crate::application::ports::{
    RemoteResourceItem, RemoteResourceJournalPort, RemoteResourcePublication, RemoteResourceStage,
    RemoteResourceTarget,
};
use crate::domain::sync::{
    AuthorityId, Epoch, LibraryId, ObjectKey, ResourceDigest, SharedResourceKind,
    SharedResourceManifest, SyncBinding,
};

fn fixture() -> (
    tempfile::TempDir,
    SqliteDatabase,
    LibraryRootId,
    ObjectKey,
    SharedResourceManifest,
) {
    let (directory, database, root) = database();
    let song = song(root, "media/track.flac", "Track", "Artist");
    SongRepository::upsert(&database, &song).expect("insert song");
    let object = ObjectKey::Song(song.id());
    let manifest = manifest(b"initial audio", "media/track.flac");
    database
        .set_local_sync_resources(object, vec![manifest.clone()])
        .expect("declare current resource");
    database
        .bind_sync_library(
            SyncBinding {
                library_id: LibraryId::new(),
                authority_id: AuthorityId::new(),
                epoch: Epoch::from_u64(1),
            },
            false,
            root,
        )
        .expect("bind replica");
    (directory, database, root, object, manifest)
}

fn manifest(bytes: &[u8], path: &str) -> SharedResourceManifest {
    SharedResourceManifest::new(
        SharedResourceKind::Audio,
        ResourceDigest::from_bytes(*blake3::hash(bytes).as_bytes()),
        bytes.len() as u64,
        "audio/flac".to_owned(),
        path,
    )
    .expect("valid resource manifest")
}

fn publication(
    root: LibraryRootId,
    object: ObjectKey,
    manifest: SharedResourceManifest,
    expected_old_digest: Option<ResourceDigest>,
) -> RemoteResourcePublication {
    let operation = OperationId::new();
    RemoteResourcePublication {
        root,
        operation,
        object,
        items: vec![RemoteResourceItem {
            target: RemoteResourceTarget {
                manifest,
                expected_old_digest,
            },
            stage: RemoteResourceStage::Planned,
            staging_path: format!("echo/tmp/{operation}/audio.part"),
            local_path: "media/track.flac".to_owned(),
            old_path: None,
        }],
        completed: false,
    }
}

#[test]
fn remote_resource_publication_is_durable_and_marks_media_ready_only_after_finish() {
    let (_directory, database, root, object, manifest) = fixture();
    assert!(!database
        .remote_resource_media_ready(root, object)
        .expect("initially missing"));
    let pending = database
        .pending_remote_resource_sets(root)
        .expect("pending manifest");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].0, object);
    assert_eq!(pending[0].1[0].expected_old_digest, None);

    let mut publication = publication(root, object, manifest.clone(), None);
    database.prepare(&publication).expect("reserve publication");
    assert_eq!(database.incomplete(root).unwrap(), [publication.operation]);
    assert_eq!(
        RemoteResourceJournalPort::load(&database, root, publication.operation).unwrap(),
        publication
    );
    assert!(database
        .local_remote_resource_path(root, object, &manifest)
        .unwrap()
        .is_none());

    publication.items[0].stage = RemoteResourceStage::Published;
    database
        .save(&publication)
        .expect("persist publication stage");
    database
        .finish(&publication)
        .expect("associate published media");
    assert!(database
        .remote_resource_media_ready(root, object)
        .expect("media ready"));
    assert!(database
        .pending_remote_resource_sets(root)
        .unwrap()
        .is_empty());
    assert_eq!(
        database
            .local_remote_resource_path(root, object, &manifest)
            .unwrap(),
        Some(RelativeMediaPath::new("media/track.flac").unwrap())
    );
    assert!(database.incomplete(root).unwrap().is_empty());
    let completed =
        RemoteResourceJournalPort::load(&database, root, publication.operation).unwrap();
    assert!(completed.completed);
    assert_eq!(completed.items[0].stage, RemoteResourceStage::Completed);
    database
        .finish(&completed)
        .expect("finish retry is idempotent");
}

#[test]
fn remote_resource_replacement_requires_the_exact_previous_digest() {
    let (_directory, database, root, object, initial) = fixture();
    let mut first = publication(root, object, initial.clone(), None);
    database.prepare(&first).unwrap();
    first.items[0].stage = RemoteResourceStage::Published;
    database.save(&first).unwrap();
    database.finish(&first).unwrap();

    let replacement = manifest(b"replacement audio", "media/track.flac");
    database
        .set_local_sync_resources(object, vec![replacement.clone()])
        .unwrap();
    let pending = database.pending_remote_resource_sets(root).unwrap();
    assert_eq!(pending[0].1[0].expected_old_digest, Some(initial.digest));
    assert!(!database.remote_resource_media_ready(root, object).unwrap());

    let wrong_old = ResourceDigest::from_bytes([0x55; 32]);
    let rejected = publication(root, object, replacement.clone(), Some(wrong_old));
    assert!(database.prepare(&rejected).is_err());
    assert!(database.incomplete(root).unwrap().is_empty());

    let mut valid = publication(root, object, replacement, Some(initial.digest));
    database.prepare(&valid).unwrap();
    valid = RemoteResourceJournalPort::load(&database, root, valid.operation).unwrap();
    assert_eq!(valid.items[0].old_path.as_deref(), Some("media/track.flac"));
    valid.items[0].stage = RemoteResourceStage::Published;
    database.save(&valid).unwrap();
    database.finish(&valid).unwrap();
    assert!(database.remote_resource_media_ready(root, object).unwrap());
}

#[test]
fn remote_resource_journal_rejects_stale_root_reused_identity_and_incomplete_finish() {
    let (_directory, database, root, object, initial) = fixture();
    let incomplete = publication(root, object, initial, None);
    database.prepare(&incomplete).unwrap();
    assert!(database.finish(&incomplete).is_err());

    let changed = manifest(b"different bytes", "media/other.flac");
    let reused = RemoteResourcePublication {
        items: vec![RemoteResourceItem {
            target: RemoteResourceTarget {
                manifest: changed,
                expected_old_digest: None,
            },
            ..incomplete.items[0].clone()
        }],
        ..incomplete.clone()
    };
    assert!(database.prepare(&reused).is_err());

    let changed_declaration = manifest(b"new current declaration", "media/track.flac");
    database
        .set_local_sync_resources(object, vec![changed_declaration.clone()])
        .unwrap();
    assert!(RemoteResourceJournalPort::load(&database, root, incomplete.operation).is_ok());
    assert!(database.finish(&incomplete).is_err());

    let other_root = LibraryRootId::new();
    let stale_root = publication(other_root, object, changed_declaration, None);
    assert!(database.prepare(&stale_root).is_err());
    assert_eq!(database.incomplete(root).unwrap(), [incomplete.operation]);
}
