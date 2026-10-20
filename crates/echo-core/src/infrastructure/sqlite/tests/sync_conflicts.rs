use super::*;
use crate::application::ports::{LibraryRepository, LocalSyncQueuePort, PlaylistRepository};
use crate::domain::entities::LibraryRoot;
use crate::domain::sync::{AuthorityId, Epoch, LibraryId};

#[test]
fn favorite_and_play_stats_conflicts_compare_visible_values() {
    assert!(!super::has_user_value_change(
        "favorite",
        &serde_json::json!({"is_favorite": true, "revision": 1}),
        &serde_json::json!({"is_favorite": true, "revision": 2}),
    ));
    assert!(super::has_user_value_change(
        "favorite",
        &serde_json::json!({"is_favorite": true}),
        &serde_json::json!({"is_favorite": false}),
    ));
    assert!(!super::has_user_value_change(
        "play-stats",
        &serde_json::json!({"by_device": {"device-a": 2}}),
        &serde_json::json!({"by_device": {"device-a": 2}}),
    ));
    assert!(super::has_user_value_change(
        "play-stats",
        &serde_json::json!({"by_device": {"device-a": 2}}),
        &serde_json::json!({"by_device": {"device-a": 3}}),
    ));
    assert_eq!(
        super::fields("favorite", &serde_json::json!({"is_favorite": true})),
        serde_json::json!({"is_favorite": true})
    );
}

fn fixture() -> (
    tempfile::TempDir,
    SqliteDatabase,
    LibraryRootId,
    SyncBinding,
    PlaylistId,
    Uuid,
) {
    let dir = tempfile::tempdir().unwrap();
    let db = SqliteDatabase::open(dir.path().join("library.db")).unwrap();
    let root = LibraryRootId::new();
    LibraryRepository::upsert(
        &db,
        &LibraryRoot::new(root, dir.path().join("music"), true, true),
    )
    .unwrap();
    let binding = SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    db.bind_sync_library(binding, true, root).unwrap();
    let playlist = PlaylistId::new();
    PlaylistRepository::create(&db, playlist, root, "Current name").unwrap();
    let conflict = Uuid::new_v4();
    db.writer.run(move |connection| {
        connection.execute("UPDATE sync_mutations SET status='confirmed'", []).map_err(storage)?;
        connection.execute("INSERT INTO sync_conflicts VALUES(?1,'playlist',?2,1,2,?3,?4,NULL)", params![conflict.to_string(), playlist.to_string(), r#"{"display_name":"Recovered name","members":[{"song_uuid":"malicious"}],"relative_path":"private"}"#, r#"{"display_name":"Current name"}"#]).map_err(storage)?;
        Ok(())
    }).unwrap();
    (dir, db, root, binding, playlist, conflict)
}

#[test]
fn recovery_enqueues_a_new_event_and_resolves_once_without_replaying_members() {
    let (_dir, db, root, binding, playlist, conflict) = fixture();
    let views = db.host_sync_conflicts(root, binding, 20).unwrap();
    assert_eq!(views.len(), 1);
    assert_eq!(
        views[0].replaced_fields,
        serde_json::json!({"display_name":"Recovered name"})
    );
    assert_eq!(views[0].recovery, SyncConflictRecovery::Available);
    assert_eq!(
        db.resolve_host_sync_conflict(root, binding, conflict, true)
            .unwrap(),
        SyncConflictResolution::Recovered
    );
    let pending = db.pending_local_mutations(20).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].payload["display_name"], "Recovered name");
    assert_eq!(pending[0].payload["members"], serde_json::json!([]));
    assert_ne!(pending[0].mutation_id.as_uuid(), conflict);
    assert!(PlaylistRepository::members(&db, playlist)
        .unwrap()
        .is_empty());
    assert_eq!(
        db.resolve_host_sync_conflict(root, binding, conflict, true)
            .unwrap(),
        SyncConflictResolution::AlreadyResolved
    );
    assert_eq!(db.pending_local_mutations(20).unwrap().len(), 1);
    assert!(db
        .host_sync_conflicts(root, binding, 20)
        .unwrap()
        .is_empty());
}

#[test]
fn final_tombstone_is_never_recovered_but_history_can_be_dismissed() {
    let (_dir, db, root, binding, playlist, conflict) = fixture();
    db.writer.run(move |connection| {
        connection.execute("INSERT INTO sync_object_versions(object_type,object_uuid,object_version,commit_seq,payload_json,is_tombstone) VALUES('playlist',?1,3,3,NULL,1)", [playlist.to_string()]).map_err(storage)?;
        Ok(())
    }).unwrap();
    assert_eq!(
        db.host_sync_conflicts(root, binding, 20).unwrap()[0].recovery,
        SyncConflictRecovery::FinalDeleted
    );
    assert_eq!(
        db.resolve_host_sync_conflict(root, binding, conflict, true)
            .unwrap(),
        SyncConflictResolution::FinalDeleted
    );
    assert!(db.pending_local_mutations(20).unwrap().is_empty());
    assert_eq!(db.host_sync_conflicts(root, binding, 20).unwrap().len(), 1);
    assert_eq!(
        db.resolve_host_sync_conflict(root, binding, conflict, false)
            .unwrap(),
        SyncConflictResolution::Dismissed
    );
}

#[test]
fn wrong_root_epoch_or_role_cannot_read_or_resolve_history() {
    let (_dir, db, root, binding, _playlist, conflict) = fixture();
    assert!(db
        .host_sync_conflicts(LibraryRootId::new(), binding, 20)
        .is_err());
    assert!(db
        .resolve_host_sync_conflict(
            root,
            SyncBinding {
                authority_id: AuthorityId::new(),
                ..binding
            },
            conflict,
            true
        )
        .is_err());
    db.writer
        .run(|connection| {
            connection
                .execute("UPDATE sync_binding SET role='replica'", [])
                .map_err(storage)?;
            Ok(())
        })
        .unwrap();
    assert!(db.host_sync_conflicts(root, binding, 20).is_err());
    assert!(db
        .resolve_host_sync_conflict(root, binding, conflict, false)
        .is_err());
}

#[test]
fn pending_local_edit_is_preserved_and_failed_recovery_stays_unresolved() {
    let (_dir, db, root, binding, playlist, conflict) = fixture();
    PlaylistRepository::rename(&db, playlist, "Local edit").unwrap();
    assert_eq!(
        db.resolve_host_sync_conflict(root, binding, conflict, true)
            .unwrap(),
        SyncConflictResolution::PendingEdit
    );
    let pending = db.pending_local_mutations(20).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].payload["display_name"], "Local edit");
    assert_eq!(db.host_sync_conflicts(root, binding, 20).unwrap().len(), 1);
}

#[test]
fn override_recovery_uses_new_local_provenance_and_cannot_restore_deleted_song() {
    use crate::application::ports::SongRepository;
    use crate::domain::entities::Song;
    use crate::domain::ids::{RelativeMediaPath, Revision, SongId};
    use crate::domain::library::{DeviceId, HybridLogicalClock, OverrideRecord};

    let (_dir, db, root, binding, _playlist, _conflict) = fixture();
    let song = SongId::new();
    SongRepository::upsert(
        &db,
        &Song::new(
            song,
            root,
            RelativeMediaPath::new("song.flac").unwrap(),
            Revision::INITIAL,
        ),
    )
    .unwrap();
    let old_device = DeviceId::new();
    let current = serde_json::to_value(OverrideRecord {
        song_uuid: song.as_uuid(),
        revision: Revision::INITIAL,
        updated_by_device_id: old_device,
        hlc: HybridLogicalClock::new(1, 0),
        title: Some("Accepted".into()),
        artist: None,
        album: None,
        lyrics_text: None,
    })
    .unwrap();
    let conflict = Uuid::new_v4();
    db.writer.run(move |connection| {
        connection.execute("UPDATE sync_mutations SET status='confirmed'", []).map_err(storage)?;
        super::super::sync_projection::project_confirmed_song_state(connection, "override", &song.to_string(), &current, Some(&root.to_string()))?;
        connection.execute("INSERT INTO sync_object_versions VALUES('override',?1,2,2,?2,0,0)", params![song.to_string(), current.to_string()]).map_err(storage)?;
        connection.execute("INSERT INTO sync_conflicts VALUES(?1,'override',?2,1,2,?3,?4,NULL)", params![conflict.to_string(), song.to_string(), r#"{"title":"Recovered","artist":null,"album":null,"lyrics_text":"Lyrics","song_uuid":"foreign-id","revision":99999}"#, current.to_string()]).map_err(storage)?;
        Ok(())
    }).unwrap();
    assert_eq!(
        db.resolve_host_sync_conflict(root, binding, conflict, true)
            .unwrap(),
        SyncConflictResolution::Recovered
    );
    let pending = db.pending_local_mutations(20).unwrap();
    assert_eq!(pending.len(), 1);
    let recovered: OverrideRecord = serde_json::from_value(pending[0].payload.clone()).unwrap();
    assert_eq!(recovered.song_uuid, song.as_uuid());
    assert_eq!(recovered.title.as_deref(), Some("Recovered"));
    assert_eq!(recovered.lyrics_text.as_deref(), Some("Lyrics"));
    assert_ne!(recovered.updated_by_device_id, old_device);
    assert_ne!(recovered.revision.as_u64(), 99999);
    assert!(recovered.hlc.wall_secs > 1);

    let next = Uuid::new_v4();
    db.writer.run(move |connection| {
        connection.execute("UPDATE sync_mutations SET status='confirmed'", []).map_err(storage)?;
        connection.execute("INSERT INTO tombstones VALUES('song',?1,1,1)", [song.to_string()]).map_err(storage)?;
        connection.execute("INSERT INTO sync_conflicts SELECT ?1,object_type,object_uuid,replaced_version,accepted_version,replaced_payload_json,accepted_payload_json,NULL FROM sync_conflicts WHERE conflict_id=?2", params![next.to_string(), conflict.to_string()]).map_err(storage)?;
        Ok(())
    }).unwrap();
    assert_eq!(
        db.resolve_host_sync_conflict(root, binding, next, true)
            .unwrap(),
        SyncConflictResolution::FinalDeleted
    );
    assert!(db.pending_local_mutations(20).unwrap().is_empty());
}
