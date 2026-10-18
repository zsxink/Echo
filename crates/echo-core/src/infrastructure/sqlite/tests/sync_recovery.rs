use super::*;
use crate::application::ports::{
    HostCommitLedgerPort, HostPeerAuthorizationPort, LibraryRepository, LocalSyncQueuePort,
    SongRepository,
};
use crate::domain::entities::{LibraryRoot, Song};
use crate::domain::ids::{RelativeMediaPath, Revision, SongId};
use crate::domain::sync::{AuthorityId, CommitSeq, DeviceId, Epoch, LibraryId};

fn host() -> (
    tempfile::TempDir,
    SqliteDatabase,
    LibraryRootId,
    SyncBinding,
) {
    let directory = tempfile::tempdir().unwrap();
    let database = SqliteDatabase::open(directory.path().join("echo.db")).unwrap();
    let root = LibraryRootId::new();
    LibraryRepository::upsert(
        &database,
        &LibraryRoot::new(root, directory.path().join("library"), true, true),
    )
    .unwrap();
    let binding = SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    database.bind_sync_library(binding, true, root).unwrap();
    (directory, database, root, binding)
}

fn replacement(binding: SyncBinding) -> SyncBinding {
    SyncBinding {
        authority_id: AuthorityId::new(),
        epoch: Epoch::from_u64(binding.epoch.as_u64() + 1),
        ..binding
    }
}

fn add_song(database: &SqliteDatabase, root: LibraryRootId) -> SongId {
    let id = SongId::new();
    let mut song = Song::new(
        id,
        root,
        RelativeMediaPath::new(&format!("{id}.flac")).unwrap(),
        Revision::INITIAL,
    );
    song.apply_scan_facts(
        "a".repeat(64),
        4,
        1,
        crate::domain::media::AudioFormat::Flac,
    );
    SongRepository::upsert(database, &song).unwrap();
    id
}

#[test]
fn explicit_recovery_preserves_pending_conflicts_and_fences_old_authorization() {
    let (directory, database, root, binding) = host();
    let song = add_song(&database, root);
    let pending = database.pending_local_mutations(20).unwrap();
    let conflict = Uuid::new_v4();
    database
        .writer
        .run(move |connection| {
            connection
                .execute(
                    "INSERT INTO sync_conflicts VALUES(?1,'song',?2,1,2,?3,?4,NULL)",
                    params![
                        conflict.to_string(),
                        song.to_string(),
                        "{\"title\":\"loser\"}",
                        "{\"title\":\"winner\"}"
                    ],
                )
                .map_err(storage)?;
            Ok(())
        })
        .unwrap();
    let peer = DeviceId::new();
    database
        .authorize_sync_peer(binding, peer, "Old peer".into(), "old-handle".into())
        .unwrap();
    let next = replacement(binding);
    let archive = database.rotate_host_epoch(root, binding, next).unwrap();
    assert_eq!((archive.pending_count, archive.conflict_count), (1, 1));
    assert_eq!(
        database
            .current_sync_configuration()
            .unwrap()
            .unwrap()
            .binding,
        next
    );
    assert!(!database.peer_is_authorized(binding, peer).unwrap());
    assert!(!database.peer_is_authorized(next, peer).unwrap());
    assert!(database.pending_local_mutations(20).unwrap().is_empty());
    assert!(database
        .published_commits_after(binding, CommitSeq::GENESIS, [0; 32], 20)
        .is_err());
    let candidates = database
        .host_recovery_candidates(archive.recovery_id)
        .unwrap();
    assert_eq!(candidates.len(), 2);
    assert!(candidates.iter().any(|candidate| candidate.identity
        == pending[0].mutation_id.as_uuid().to_string()
        && serde_json::from_str::<serde_json::Value>(&candidate.payload_json).unwrap()
            == pending[0].payload));
    assert!(candidates
        .iter()
        .any(|candidate| candidate.identity == conflict.to_string()
            && candidate.competing_payload_json.as_deref() == Some("{\"title\":\"winner\"}")));
    assert!(SongRepository::by_id(&database, song).unwrap().is_some());
    drop(database);
    let database = SqliteDatabase::open(directory.path().join("echo.db")).unwrap();
    assert_eq!(
        database.host_recovery_archives(root).unwrap().as_slice(),
        std::slice::from_ref(&archive)
    );
    assert_eq!(
        database
            .host_recovery_candidates(archive.recovery_id)
            .unwrap(),
        candidates
    );
}

#[test]
fn recovery_failure_rolls_back_every_archive_and_live_row() {
    let (_directory, database, root, binding) = host();
    add_song(&database, root);
    let original = database.pending_local_mutations(20).unwrap();
    database.writer.run(|connection| connection.execute_batch("CREATE TRIGGER fail_host_recovery BEFORE INSERT ON sync_binding BEGIN SELECT RAISE(ABORT,'injected recovery failure'); END;").map_err(storage)).unwrap();
    assert!(database
        .rotate_host_epoch(root, binding, replacement(binding))
        .is_err());
    assert_eq!(
        database
            .current_sync_configuration()
            .unwrap()
            .unwrap()
            .binding,
        binding
    );
    assert_eq!(database.pending_local_mutations(20).unwrap(), original);
    assert!(database.host_recovery_archives(root).unwrap().is_empty());
}

#[test]
fn recovery_rejects_stale_same_epoch_or_another_library_without_touching_state() {
    let (_directory, database, root, binding) = host();
    let same_epoch = SyncBinding {
        authority_id: AuthorityId::new(),
        ..binding
    };
    let other_library = SyncBinding {
        library_id: LibraryId::new(),
        ..replacement(binding)
    };
    for next in [binding, same_epoch, other_library] {
        assert!(database.rotate_host_epoch(root, binding, next).is_err());
    }
    assert_eq!(
        database
            .current_sync_configuration()
            .unwrap()
            .unwrap()
            .binding,
        binding
    );
    assert!(database.host_recovery_archives(root).unwrap().is_empty());
}

#[test]
fn recovered_tombstones_are_new_epoch_baseline_events_and_aliases_survive() {
    let (_directory, database, root, binding) = host();
    let deleted = SongId::new();
    let canonical = SongId::new();
    let alias = SongId::new();
    database
        .writer
        .run(move |connection| {
            connection
                .execute(
                    "INSERT INTO sync_object_versions VALUES('song',?1,6,6,NULL,1)",
                    params![deleted.to_string()],
                )
                .map_err(storage)?;
            connection
                .execute(
                    "INSERT INTO sync_song_aliases VALUES(?1,?2,4)",
                    params![alias.to_string(), canonical.to_string()],
                )
                .map_err(storage)?;
            Ok(())
        })
        .unwrap();
    let next = replacement(binding);
    let archive = database.rotate_host_epoch(root, binding, next).unwrap();
    assert_eq!(database.ensure_host_sync_baseline(next, root).unwrap(), 1);
    assert_eq!(database.ensure_host_sync_baseline(next, root).unwrap(), 0);
    let pending = database.pending_local_mutations(20).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(
        pending[0].object,
        crate::domain::sync::ObjectKey::Song(deleted)
    );
    assert_eq!(pending[0].payload, serde_json::json!({"tombstone":true}));
    assert_eq!(pending[0].origin_seq, 1);
    database.writer.run(move |connection| {
        let saved: (String,i64) = connection.query_row("SELECT canonical_song_uuid,commit_seq FROM sync_song_aliases WHERE alias_song_uuid=?1",params![alias.to_string()],|row|Ok((row.get(0)?,row.get(1)?))).map_err(storage)?;
        assert_eq!(saved,(canonical.to_string(),0));
        let tombstones: i64=connection.query_row("SELECT COUNT(*) FROM recovery_sync_object_versions WHERE recovery_id=?1 AND is_tombstone=1",params![archive.recovery_id.to_string()],|row|row.get(0)).map_err(storage)?;
        assert_eq!(tombstones,1);
        Ok(())
    }).unwrap();
}

#[test]
fn online_backup_contains_matching_catalog_pending_and_ledger_without_independent_checkpoint() {
    let (directory, database, root, binding) = host();
    let song = add_song(&database, root);
    let pending = database.pending_local_mutations(20).unwrap();
    let staged = database.stage_confirmation(binding, &pending[0]).unwrap();
    let checkpoint = staged.checkpoint.unwrap();
    database.mark_published(checkpoint).unwrap();
    let destination = directory.path().join("host-backup.db");
    database.backup_to(&destination).unwrap();
    SongRepository::set_favorite(&database, song, true).unwrap();
    let restored = SqliteDatabase::open(destination).unwrap();
    assert_eq!(
        restored.latest_published_sync_checkpoint(binding).unwrap(),
        checkpoint
    );
    assert_eq!(restored.pending_local_mutations(20).unwrap().len(), 0);
    assert!(!SongRepository::by_id(&restored, song)
        .unwrap()
        .unwrap()
        .favorite());
    assert_eq!(
        restored
            .current_sync_configuration()
            .unwrap()
            .unwrap()
            .binding,
        binding
    );
    // The independent checkpoint has no SQLite table and cannot roll back with
    // this snapshot. Startup must compare the external newer head before serve.
    restored.quick_check().unwrap();
}
