//! Root switching retains independent local queues and authority histories.

use super::*;
use crate::application::ports::{HostCommitLedgerPort, LocalSyncQueuePort};
use crate::domain::sync::{
    AuthorityId, DeviceId, Epoch, LibraryId, SyncBinding, SyncSessionIntent,
};

fn binding() -> SyncBinding {
    SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    }
}

fn add_root(database: &SqliteDatabase, directory: &std::path::Path) -> LibraryRootId {
    let root = LibraryRootId::new();
    LibraryRepository::upsert(
        database,
        &LibraryRoot::new(root, directory.into(), false, true),
    )
    .unwrap();
    root
}

#[test]
#[allow(clippy::too_many_lines)]
fn root_switching_preserves_pending_peers_and_independent_commit_chains() {
    let (directory, database, a) = database();
    let b = add_root(&database, &directory.path().join("b"));
    let a_binding = binding();
    let b_binding = binding();
    let a_song = song(a, "a.flac", "A", "artist");
    let b_song = song(b, "b.flac", "B", "artist");
    SongRepository::upsert(&database, &a_song).unwrap();
    database.bind_sync_library(a_binding, true, a).unwrap();
    // Only edits made with a sync context are queueable (SYN-R06-S04); the
    // standalone upsert above was local data only.
    SongRepository::set_favorite(&database, a_song.id(), true).unwrap();
    let a_first = database.pending_local_mutations(32).unwrap()[0].clone();
    assert_eq!(a_first.origin_seq, 1);
    assert_eq!(a_first.previous_mutation_id, None);
    let a_checkpoint = database
        .stage_confirmation(a_binding, &a_first)
        .unwrap()
        .checkpoint
        .unwrap();
    database.mark_published(a_checkpoint).unwrap();
    let peer = DeviceId::new();
    database
        .authorize_sync_peer(a_binding, peer, "A peer".into(), "vault-a".into())
        .unwrap();
    database
        .set_sync_session_intent(SyncSessionIntent::Enabled)
        .unwrap();

    // An unbound root's edit only touches local data; it never enters a sync
    // queue (SYN-R06-S04) and never exchanges the current authority ledger.
    SongRepository::upsert(&database, &b_song).unwrap();
    assert!(database.pending_local_mutations(32).unwrap().is_empty());
    database.bind_sync_library(b_binding, true, b).unwrap();
    assert_eq!(
        database
            .current_sync_configuration()
            .unwrap()
            .unwrap()
            .session_intent,
        SyncSessionIntent::Stopped
    );
    database
        .set_sync_session_intent(SyncSessionIntent::Paused)
        .unwrap();
    assert!(database.authorized_sync_peer_handles().unwrap().is_empty());
    assert_eq!(
        database
            .latest_published_sync_checkpoint(b_binding)
            .unwrap()
            .sequence
            .as_u64(),
        0
    );
    let b_first = database.pending_local_mutations(32).unwrap();
    assert!(b_first.is_empty());
    SongRepository::set_favorite(&database, b_song.id(), true).unwrap();
    let b_first = database.pending_local_mutations(32).unwrap()[0].clone();
    assert_eq!(b_first.origin_seq, 1);
    assert_eq!(b_first.previous_mutation_id, None);
    let b_checkpoint = database
        .stage_confirmation(b_binding, &b_first)
        .unwrap()
        .checkpoint
        .unwrap();
    database.mark_published(b_checkpoint).unwrap();
    database
        .authorize_sync_peer(b_binding, peer, "B peer".into(), "vault-b".into())
        .unwrap();
    database
        .set_sync_session_intent(SyncSessionIntent::Paused)
        .unwrap();
    // A previously bound root keeps its own queue while B is live: the edit is
    // invisible from B's queue but is captured under A's archived context.
    SongRepository::set_favorite(&database, a_song.id(), false).unwrap();
    assert!(database.pending_local_mutations(32).unwrap().is_empty());

    // Idempotent bind must commit restoration even when B is currently live.
    database.bind_sync_library(a_binding, true, a).unwrap();
    assert_eq!(
        database
            .current_sync_configuration()
            .unwrap()
            .unwrap()
            .session_intent,
        SyncSessionIntent::Enabled
    );
    assert_eq!(
        database.authorized_sync_peer_handles().unwrap(),
        [(peer, "vault-a".into())]
    );
    let a_queue = database.pending_local_mutations(32).unwrap();
    assert_eq!(a_queue.len(), 1);
    assert_eq!(a_queue[0].origin_seq, 2);
    assert_eq!(a_queue[0].previous_mutation_id, Some(a_first.mutation_id));
    let a_second = database
        .stage_confirmation(a_binding, &a_queue[0])
        .unwrap()
        .checkpoint
        .unwrap();
    assert_eq!(a_second.sequence.as_u64(), 2);
    database.mark_published(a_second).unwrap();
    assert_eq!(
        database
            .latest_published_sync_checkpoint(b_binding)
            .unwrap()
            .sequence
            .as_u64(),
        0
    );

    LibraryRepository::upsert(
        &database,
        &LibraryRoot::new(b, directory.path().join("b"), true, true),
    )
    .unwrap();
    assert_eq!(
        database
            .current_sync_configuration()
            .unwrap()
            .unwrap()
            .session_intent,
        SyncSessionIntent::Paused
    );
    assert_eq!(
        database
            .current_sync_configuration()
            .unwrap()
            .unwrap()
            .session_intent,
        SyncSessionIntent::Paused
    );
    assert_eq!(
        database.authorized_sync_peer_handles().unwrap(),
        [(peer, "vault-b".into())]
    );
    assert_eq!(
        database
            .latest_published_sync_checkpoint(b_binding)
            .unwrap(),
        b_checkpoint
    );
    assert!(database.pending_local_mutations(32).unwrap().is_empty());
    LibraryRepository::upsert(
        &database,
        &LibraryRoot::new(a, directory.path().join("library"), true, true),
    )
    .unwrap();
    assert_eq!(
        database
            .latest_published_sync_checkpoint(a_binding)
            .unwrap(),
        a_second
    );
    let path = database.path().to_owned();
    drop(database);
    let reopened = SqliteDatabase::open(path).unwrap();
    assert_eq!(
        reopened
            .current_sync_configuration()
            .unwrap()
            .unwrap()
            .binding,
        a_binding
    );
    assert_eq!(
        reopened
            .latest_published_sync_checkpoint(a_binding)
            .unwrap(),
        a_second
    );
}

#[test]
fn independent_replica_enrollment_preserves_unbound_offline_edits() {
    let (directory, database, a) = database();
    let a_binding = binding();
    database.bind_sync_library(a_binding, true, a).unwrap();
    let b = add_root(&database, &directory.path().join("b"));
    let b_song = song(b, "offline.flac", "Offline", "artist");
    SongRepository::upsert(&database, &b_song).unwrap();
    SongRepository::set_favorite(&database, b_song.id(), true).unwrap();
    database.select_sync_context(b).unwrap();
    assert!(database.current_sync_configuration().unwrap().is_none());
    // An unbound library keeps those edits as local data only; enrollment must
    // not resurrect a queue that was never allowed to exist (SYN-R06-S04).
    let offline = database.pending_local_mutations(32).unwrap();
    assert!(
        offline.is_empty(),
        "an unbound library retains no sync queue"
    );
    let b_binding = binding();
    database.select_sync_context(a).unwrap();
    assert!(database.bind_empty_sync_replica(b_binding, b).is_err());
    assert_eq!(
        database
            .current_sync_configuration()
            .unwrap()
            .unwrap()
            .binding,
        a_binding
    );
    database
        .bind_sync_replica_for_enrollment(b_binding, b)
        .unwrap();
    let configured = database.current_sync_configuration().unwrap().unwrap();
    assert!(!configured.host);
    assert_eq!(configured.local_root, b);
    assert_eq!(database.pending_local_mutations(32).unwrap(), offline);
    assert!(SongRepository::by_id(&database, b_song.id())
        .unwrap()
        .unwrap()
        .favorite());
    let c = add_root(&database, &directory.path().join("c"));
    database.bind_empty_sync_replica(binding(), c).unwrap();
    assert!(database.pending_local_mutations(32).unwrap().is_empty());
    database.select_sync_context(a).unwrap();
    assert_eq!(
        database
            .current_sync_configuration()
            .unwrap()
            .unwrap()
            .binding,
        a_binding
    );
}

#[test]
fn corrupt_context_owner_cannot_be_adopted_by_select_or_bind() {
    let (directory, database, a) = database();
    let b = add_root(&database, &directory.path().join("b"));
    let original = binding();
    database.bind_sync_library(original, true, a).unwrap();
    database
        .writer
        .run(|connection| {
            connection
                .execute("UPDATE sync_binding SET local_root_uuid=NULL", [])
                .map_err(storage)?;
            Ok(())
        })
        .unwrap();
    assert!(database.select_sync_context(a).is_err());
    assert!(database.select_sync_context(b).is_err());
    assert!(database.bind_sync_library(original, true, a).is_err());
    assert!(database.bind_empty_sync_replica(binding(), b).is_err());
    let owner: Option<String> = database
        .with_reader(|connection| {
            connection
                .query_row("SELECT local_root_uuid FROM sync_binding", [], |row| {
                    row.get(0)
                })
                .map_err(storage)
        })
        .unwrap();
    assert_eq!(owner, None);
}

#[test]
fn local_delete_and_resource_journals_retain_canonical_root_ownership() {
    use crate::application::ports::{
        RemoteResourceItem, RemoteResourceJournalPort, RemoteResourcePublication,
        RemoteResourceStage, RemoteResourceTarget,
    };
    use crate::domain::sync::{
        ObjectKey, ResourceDigest, SharedResourceKind, SharedResourceManifest,
    };

    let (directory, database, a) = database();
    let b = add_root(&database, &directory.path().join("b"));
    let a_song = song(a, "shared.flac", "A", "artist");
    let b_song = song(b, "shared.flac", "B", "artist");
    SongRepository::upsert(&database, &a_song).unwrap();
    SongRepository::upsert(&database, &b_song).unwrap();
    let intent = OperationId::new();
    let song_id = a_song.id();
    database.writer.run(move |connection| {
        connection.execute("INSERT INTO sync_local_delete_intents(intent_id,final_mutation_id,object_type,object_uuid,created_at,undo_deadline,state) VALUES(?1,?2,'song',?3,1,2,'open')", params![intent.to_string(),uuid::Uuid::new_v4().to_string(),song_id.to_string()]).map_err(storage)?;
        Ok(())
    }).unwrap();

    let mut operations = Vec::new();
    for (root, song) in [(a, a_song.id()), (b, b_song.id())] {
        database.bind_sync_library(binding(), true, root).unwrap();
        let object = ObjectKey::Song(song);
        let resource = SharedResourceManifest::new(
            SharedResourceKind::Audio,
            ResourceDigest::from_bytes([1; 32]),
            1,
            "audio/flac".into(),
            "shared.flac",
        )
        .unwrap();
        database
            .set_local_sync_resources(object, vec![resource.clone()])
            .unwrap();
        let operation = OperationId::new();
        let publication = RemoteResourcePublication {
            root,
            operation,
            object,
            completed: false,
            items: vec![RemoteResourceItem {
                target: RemoteResourceTarget {
                    manifest: resource,
                    expected_old_digest: None,
                },
                stage: RemoteResourceStage::Planned,
                staging_path: format!("echo/tmp/{operation}/audio.part"),
                local_path: "shared.flac".into(),
                old_path: None,
            }],
        };
        database.prepare(&publication).unwrap();
        operations.push(operation);
    }
    // Identical relative paths in distinct roots do not contend for claims.
    assert_eq!(database.incomplete(a).unwrap(), [operations[0]]);
    assert_eq!(database.incomplete(b).unwrap(), [operations[1]]);
    assert!(RemoteResourceJournalPort::load(&database, b, operations[0]).is_err());
    database.select_sync_context(a).unwrap();
    assert_eq!(database.incomplete(a).unwrap(), [operations[0]]);
    let owner: String = database.with_reader(move |connection| {
        connection.query_row("SELECT s.library_root_uuid FROM sync_local_delete_intents i JOIN songs s ON s.uuid=i.object_uuid WHERE i.intent_id=?1 AND i.state='open'", params![intent.to_string()], |row| row.get(0)).map_err(storage)
    }).unwrap();
    assert_eq!(owner, a.to_string());
}
