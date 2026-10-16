use super::*;
use crate::application::ports::{
    HostSnapshotRecord, RemoteApplyPort, ReplicaSnapshotApplyPort, ReplicaSnapshotPage,
};
use crate::domain::ids::{PlaylistId, PlaylistItemId};
use crate::domain::sync::{
    AuthorityId, CommitCheckpoint, CommitSeq, DeviceId, Epoch, LibraryId, MutationCandidate,
    MutationId, ObjectKey, ObjectVersion, PlaylistMemberAlias, ResourceDigest, SharedResourceKind,
    SharedResourceManifest, SyncBinding,
};

fn replica() -> (tempfile::TempDir, SqliteDatabase, SyncBinding, DeviceId) {
    let (dir, db, root) = database();
    let binding = SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    db.bind_sync_library(binding, false, root).unwrap();
    (dir, db, binding, DeviceId::new())
}

fn page(binding: SyncBinding) -> ReplicaSnapshotPage {
    ReplicaSnapshotPage {
        snapshot_token: uuid::Uuid::new_v4().simple().to_string(),
        checkpoint: CommitCheckpoint {
            binding,
            sequence: CommitSeq::from_u64(12),
            digest: [7; 32],
        },
        page_index: 0,
        next_page_index: None,
        records: Vec::new(),
    }
}

fn object(key: ObjectKey, payload: serde_json::Value) -> HostSnapshotRecord {
    HostSnapshotRecord::Object {
        candidate: MutationCandidate {
            resource_refs: Vec::new(),
            mutation_id: MutationId::new(),
            origin_device_id: DeviceId::new(),
            origin_seq: 1,
            previous_mutation_id: None,
            object: key,
            base_object_version: None,
            payload,
        },
        version: ObjectVersion::from_commit_seq(CommitSeq::from_u64(4)),
        resources: Vec::new(),
    }
}

fn song_record(id: SongId) -> HostSnapshotRecord {
    object(
        ObjectKey::Song(id),
        serde_json::json!({"song_uuid":id,"relative_path":format!("{id}.flac"),"title":"Snapshot song","added_at":77}),
    )
}

fn count(db: &SqliteDatabase, table: &str) -> i64 {
    db.with_reader(|connection| {
        connection
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .map_err(storage)
    })
    .unwrap()
}

#[test]
fn staged_pages_survive_restart_and_finalize_atomically_without_echo() {
    let (dir, db, binding, peer) = replica();
    let song = SongId::new();
    let mut first = page(binding);
    first.next_page_index = Some(1);
    let favorite = object(
        ObjectKey::Favorite(song),
        serde_json::json!({"song_uuid":song,"is_favorite":true,"favorited_at":50}),
    );
    first.records.push(favorite);
    let staged = db.apply_snapshot_page(peer, &first).unwrap();
    assert_eq!(staged.checkpoint, None);
    assert_eq!(count(&db, "sync_object_versions"), 0);
    assert_eq!(count(&db, "sync_peer_cursors"), 0);
    assert_eq!(db.apply_snapshot_page(peer, &first).unwrap().applied, 0);
    assert!(db
        .apply_remote_batch(
            binding,
            peer,
            CommitCheckpoint {
                binding,
                sequence: CommitSeq::GENESIS,
                digest: [0; 32]
            },
            &[]
        )
        .is_err());
    drop(db);
    let db = SqliteDatabase::open(dir.path().join("echo.db")).unwrap();
    let playlist = PlaylistId::new();
    let member = PlaylistItemId::new();
    let alias = SongId::new();
    let mut song_value = song_record(song);
    let manifest = SharedResourceManifest::new(
        SharedResourceKind::Audio,
        ResourceDigest::from_bytes([9; 32]),
        44,
        "audio/flac".into(),
        "media/snapshot.flac",
    )
    .unwrap();
    if let HostSnapshotRecord::Object { resources, .. } = &mut song_value {
        resources.push(manifest);
    }
    let last = ReplicaSnapshotPage {
        page_index: 1,
        next_page_index: None,
        records: vec![
            song_value,
            object(
                ObjectKey::Playlist(playlist),
                serde_json::json!({"playlist_uuid":playlist,"display_name":"Snapshot list","created_at":12,"members":[{"song_uuid":song,"member_uuid":member,"position":0,"added_at":13}]}),
            ),
            HostSnapshotRecord::SongAlias {
                alias,
                canonical: song,
            },
            HostSnapshotRecord::PlaylistMemberAlias(PlaylistMemberAlias {
                playlist_id: playlist,
                alias_member_id: PlaylistItemId::new(),
                canonical_member_id: member,
            }),
        ],
        ..first.clone()
    };
    let applied = db.apply_snapshot_page(peer, &last).unwrap();
    assert_eq!(applied.checkpoint, Some(last.checkpoint));
    assert_eq!(count(&db, "songs"), 1);
    assert_eq!(count(&db, "playlists"), 1);
    assert_eq!(count(&db, "playlist_songs"), 1);
    assert_eq!(count(&db, "sync_resource_refs"), 1);
    assert_eq!(count(&db, "sync_song_aliases"), 1);
    assert_eq!(count(&db, "sync_member_aliases"), 1);
    assert_eq!(count(&db, "sync_outbox"), 0);
    assert_eq!(count(&db, "sync_mutations"), 0);
    assert_confirmed_song(&db);
    assert_eq!(db.apply_snapshot_page(peer, &last).unwrap().applied, 0);
    assert_eq!(db.apply_snapshot_page(peer, &first).unwrap().applied, 0);
    assert!(db.discard_snapshot(peer, &last.snapshot_token).is_err());
    let cursor = db
        .apply_remote_batch(binding, peer, last.checkpoint, &[])
        .unwrap();
    assert_eq!(cursor.sequence, last.checkpoint.sequence);
    assert_eq!(cursor.digest, last.checkpoint.digest);
}

fn assert_confirmed_song(db: &SqliteDatabase) {
    db.with_reader(|connection| {
        let value: (i64, String, i64) = connection
            .query_row(
                "SELECT is_favorite,availability,added_at FROM songs",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(storage)?;
        assert_eq!(value, (1, "missing".into(), 77));
        let version: i64 = connection
            .query_row(
                "SELECT object_version FROM sync_object_versions WHERE object_type='song'",
                [],
                |row| row.get(0),
            )
            .map_err(storage)?;
        assert_eq!(
            version, 4,
            "object version must not be inferred from snapshot H"
        );
        Ok(())
    })
    .unwrap();
}

#[test]
fn rejects_changed_pages_checkpoint_gaps_and_cross_page_duplicates() {
    let (_dir, db, binding, peer) = replica();
    let mut first = page(binding);
    first.next_page_index = Some(1);
    first.records.push(song_record(SongId::new()));
    db.apply_snapshot_page(peer, &first).unwrap();
    let mut changed = first.clone();
    changed.records.clear();
    assert!(db.apply_snapshot_page(peer, &changed).is_err());
    let mut next = ReplicaSnapshotPage {
        page_index: 1,
        next_page_index: None,
        records: Vec::new(),
        ..first.clone()
    };
    next.checkpoint.digest = [8; 32];
    assert!(db.apply_snapshot_page(peer, &next).is_err());
    next.checkpoint = first.checkpoint;
    next.page_index = 2;
    assert!(db.apply_snapshot_page(peer, &next).is_err());
    next.page_index = 1;
    next.records = first.records.clone();
    assert!(db.apply_snapshot_page(peer, &next).is_err());
    assert_eq!(count(&db, "sync_replica_snapshot_pages"), 1);
    assert_eq!(count(&db, "sync_object_versions"), 0);
    db.discard_snapshot(peer, &first.snapshot_token).unwrap();
    assert_eq!(count(&db, "sync_replica_snapshot_pages"), 0);
    assert_eq!(count(&db, "sync_replica_snapshot_records"), 0);
    let empty = page(binding);
    assert_eq!(
        db.apply_snapshot_page(peer, &empty).unwrap().checkpoint,
        Some(empty.checkpoint)
    );
}

#[test]
fn final_projection_failure_rolls_back_all_records_and_cursor() {
    let (_dir, db, binding, peer) = replica();
    let mut invalid = page(binding);
    invalid.records = vec![
        song_record(SongId::new()),
        object(
            ObjectKey::Song(SongId::new()),
            serde_json::json!({"title":"no identity or path"}),
        ),
    ];
    assert!(db.apply_snapshot_page(peer, &invalid).is_err());
    for table in [
        "songs",
        "sync_object_versions",
        "sync_peer_cursors",
        "sync_replica_snapshot_sessions",
        "sync_replica_snapshot_pages",
    ] {
        assert_eq!(count(&db, table), 0, "{table}");
    }
}

#[test]
fn rejects_nonempty_pending_wrong_role_version_and_oversized_pages() {
    let (_dir, db, binding, peer) = replica();
    let mut value = page(binding);
    let mut wrong = value.clone();
    wrong.checkpoint.binding.authority_id = AuthorityId::new();
    assert!(db.apply_snapshot_page(peer, &wrong).is_err());
    let mut item = song_record(SongId::new());
    if let HostSnapshotRecord::Object { version, .. } = &mut item {
        *version = ObjectVersion::from_commit_seq(CommitSeq::from_u64(13));
    }
    value.records.push(item);
    assert!(db.apply_snapshot_page(peer, &value).is_err());
    value.records = vec![object(
        ObjectKey::Override(SongId::new()),
        serde_json::json!({"text":"x".repeat(16*1024*1024)}),
    )];
    assert!(db.apply_snapshot_page(peer, &value).is_err());
    let connection = Connection::open(db.path()).unwrap();
    connection.execute("INSERT INTO sync_outbox(object_type,object_uuid,revision,operation,payload_json,created_at,local_root_uuid) VALUES('song','local',1,'upsert','{}',1,?1)",params![db.current_sync_configuration().unwrap().unwrap().local_root.to_string()]).unwrap();
    assert!(db.apply_snapshot_page(peer, &page(binding)).is_err());
    assert_eq!(count(&db, "sync_replica_snapshot_sessions"), 0);
}

#[test]
fn cyclic_aliases_and_resource_collision_fail_without_partial_projection() {
    let (_dir, db, binding, peer) = replica();
    let a = SongId::new();
    let b = SongId::new();
    let mut value = page(binding);
    value.records = vec![
        HostSnapshotRecord::SongAlias {
            alias: a,
            canonical: b,
        },
        HostSnapshotRecord::SongAlias {
            alias: b,
            canonical: a,
        },
    ];
    assert!(db.apply_snapshot_page(peer, &value).is_err());
    assert_eq!(count(&db, "sync_song_aliases"), 0);
    let mut left = song_record(a);
    let mut right = song_record(b);
    for (record, length) in [(&mut left, 4), (&mut right, 5)] {
        if let HostSnapshotRecord::Object { resources, .. } = record {
            resources.push(
                SharedResourceManifest::new(
                    SharedResourceKind::Audio,
                    ResourceDigest::from_bytes([4; 32]),
                    length,
                    "audio/flac".into(),
                    "media/snapshot.flac",
                )
                .unwrap(),
            );
        }
    }
    value.records = vec![left, right];
    assert!(db.apply_snapshot_page(peer, &value).is_err());
    assert_eq!(count(&db, "sync_resources"), 0);
    assert_eq!(count(&db, "sync_object_versions"), 0);
}

#[test]
fn same_digest_at_distinct_paths_preserves_each_manifest_and_one_download() {
    let (_dir, db, binding, peer) = replica();
    let mut value = page(binding);
    let mut a = song_record(SongId::new());
    let mut b = song_record(SongId::new());
    for (record, path) in [(&mut a, "media/a.flac"), (&mut b, "media/b.flac")] {
        if let HostSnapshotRecord::Object { resources, .. } = record {
            resources.push(
                SharedResourceManifest::new(
                    SharedResourceKind::Audio,
                    ResourceDigest::from_bytes([4; 32]),
                    4,
                    "audio/flac".into(),
                    path,
                )
                .unwrap(),
            );
        }
    }
    value.records = vec![a, b];
    db.apply_snapshot_page(peer, &value).unwrap();
    assert_eq!(count(&db, "sync_resources"), 1);
    assert_eq!(count(&db, "sync_resource_refs"), 2);
    assert_eq!(count(&db, "sync_resource_manifests"), 2);
}

#[test]
fn final_deleted_song_excludes_cross_page_playlist_member() {
    let (_dir, db, binding, peer) = replica();
    let deleted = SongId::new();
    let live = SongId::new();
    let playlist = PlaylistId::new();
    let mut first = page(binding);
    first.next_page_index = Some(1);
    first.records.push(object(
        ObjectKey::Playlist(playlist),
        serde_json::json!({
            "playlist_uuid":playlist,"display_name":"Frozen list","created_at":1,"members":[
                {"song_uuid":deleted,"member_uuid":PlaylistItemId::new(),"position":0,"added_at":2},
                {"song_uuid":live,"member_uuid":PlaylistItemId::new(),"position":1,"added_at":3}
            ]
        }),
    ));
    db.apply_snapshot_page(peer, &first).unwrap();
    let last = ReplicaSnapshotPage {
        page_index: 1,
        next_page_index: None,
        records: vec![
            song_record(live),
            object(
                ObjectKey::Song(deleted),
                serde_json::json!({"tombstone":true}),
            ),
        ],
        ..first
    };
    db.apply_snapshot_page(peer, &last).unwrap();
    assert_eq!(count(&db, "songs"), 1);
    assert_eq!(count(&db, "playlist_songs"), 1);
    db.with_reader(|connection| {
        let retained: String = connection
            .query_row("SELECT song_uuid FROM playlist_songs", [], |row| row.get(0))
            .map_err(storage)?;
        assert_eq!(retained, live.to_string());
        Ok(())
    })
    .unwrap();
}

#[test]
fn initial_snapshot_merges_local_only_songs_and_keeps_deduplicated_audio_playable() {
    use crate::application::ports::LocalSyncQueuePort;

    let (_directory, db, root) = database();
    let binding = SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    let mut local = song(root, "local/shared.flac", "Shared", "Artist");
    local.apply_scan_facts(
        "a".repeat(64),
        17,
        123,
        crate::domain::media::AudioFormat::Flac,
    );
    SongRepository::upsert(&db, &local).unwrap();
    SongRepository::set_favorite(&db, local.id(), true).unwrap();
    let mut local_only = song(root, "local/only.flac", "Only", "Artist");
    local_only.apply_scan_facts(
        "b".repeat(64),
        18,
        124,
        crate::domain::media::AudioFormat::Flac,
    );
    SongRepository::upsert(&db, &local_only).unwrap();
    let local_only_before = SongRepository::by_id(&db, local_only.id())
        .unwrap()
        .unwrap();
    db.bind_sync_replica_for_enrollment(binding, root).unwrap();
    db.ensure_replica_sync_baseline(binding, root).unwrap();
    let before = serde_json::to_string(&db.pending_local_mutations(100).unwrap()).unwrap();
    let canonical = SongId::new();
    let host_only = SongId::new();
    let mut snapshot = page(binding);
    snapshot.records = vec![
        object(
            ObjectKey::Song(canonical),
            serde_json::json!({"song_uuid":canonical,"relative_path":"host/shared.flac","blake3_hash":"a".repeat(64),"file_size":17,"format":"flac","title":"Host shared","added_at":77}),
        ),
        song_record(host_only),
        HostSnapshotRecord::SongAlias {
            alias: local.id(),
            canonical,
        },
    ];
    db.apply_snapshot_page(DeviceId::new(), &snapshot).unwrap();
    let actual = SongRepository::by_id(&db, canonical).unwrap().unwrap();
    assert_eq!(actual.availability(), SongAvailability::Available);
    assert_eq!(actual.path(), local.path());
    assert!(actual.favorite());
    assert_eq!(
        SongRepository::by_id(&db, local_only.id())
            .unwrap()
            .unwrap(),
        local_only_before
    );
    assert_eq!(
        db.query_active_songs("", SongSort::default(), None, 100)
            .unwrap()
            .items
            .len(),
        2
    );
    assert_eq!(
        SongRepository::by_id(&db, host_only)
            .unwrap()
            .unwrap()
            .availability(),
        SongAvailability::Missing
    );
    assert_eq!(
        serde_json::to_string(&db.pending_local_mutations(100).unwrap()).unwrap(),
        before
    );
}

#[test]
fn initial_snapshot_keeps_different_audio_at_the_same_path_and_publishes_both() {
    use crate::application::remote_resources::{PublishRemoteResources, RemoteResourcePublishDeps};
    use crate::infrastructure::filesystem::{
        Blake3ContentHasher, RootConstrainedFileSystem, RootRegistry,
    };

    let (directory, db, root) = database();
    let library_path = directory.path().join("library");
    std::fs::create_dir_all(library_path.join("media")).unwrap();
    std::fs::write(library_path.join("media/song.flac"), b"peer bytes").unwrap();
    let mut local = song(root, "media/song.flac", "Peer", "Artist");
    local.apply_scan_facts(
        blake3::hash(b"peer bytes").to_hex().to_string(),
        10,
        123,
        crate::domain::media::AudioFormat::Flac,
    );
    SongRepository::upsert(&db, &local).unwrap();
    let binding = SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    db.bind_sync_replica_for_enrollment(binding, root).unwrap();
    db.ensure_replica_sync_baseline(binding, root).unwrap();
    let host_song = SongId::new();
    let manifest = SharedResourceManifest::new(
        SharedResourceKind::Audio,
        ResourceDigest::from_bytes(*blake3::hash(b"host bytes").as_bytes()),
        10,
        "audio/flac".into(),
        "media/song.flac",
    )
    .unwrap();
    let mut record = object(
        ObjectKey::Song(host_song),
        serde_json::json!({"song_uuid":host_song,"relative_path":"media/song.flac","blake3_hash":manifest.digest.to_hex(),"file_size":10,"format":"flac","title":"Host","added_at":77}),
    );
    if let HostSnapshotRecord::Object {
        candidate,
        resources,
        ..
    } = &mut record
    {
        candidate.resource_refs.push(manifest.clone());
        resources.push(manifest.clone());
    }
    let mut snapshot = page(binding);
    snapshot.records.push(record);
    db.apply_snapshot_page(DeviceId::new(), &snapshot).unwrap();
    assert!(SongRepository::by_id(&db, host_song).unwrap().is_some());
    let db = Arc::new(db);
    let registry = RootRegistry::new();
    registry.register(root, &library_path);
    let publisher = PublishRemoteResources::new(RemoteResourcePublishDeps {
        fs: Arc::new(RootConstrainedFileSystem::new(registry.clone())),
        hasher: Arc::new(Blake3ContentHasher::new(registry)),
        journal: db.clone(),
    });
    let targets = db
        .pending_remote_resource_sets(root)
        .unwrap()
        .into_iter()
        .find(|(object, _)| *object == ObjectKey::Song(host_song))
        .unwrap()
        .1;
    let operation = OperationId::new();
    publisher
        .prepare(root, operation, ObjectKey::Song(host_song), targets)
        .unwrap();
    publisher
        .stage(root, operation, 0, &mut std::io::Cursor::new(b"host bytes"))
        .unwrap();
    assert!(publisher.resume(root, operation).unwrap().media_ready);
    let host_local = SongRepository::by_id(db.as_ref(), host_song)
        .unwrap()
        .unwrap();
    assert_eq!(host_local.availability(), SongAvailability::Available);
    assert_eq!(
        std::fs::read(library_path.join(host_local.path().normalized())).unwrap(),
        b"host bytes"
    );
    assert_eq!(
        std::fs::read(library_path.join("media/song.flac")).unwrap(),
        b"peer bytes"
    );
    assert_eq!(
        SongRepository::by_id(db.as_ref(), local.id())
            .unwrap()
            .unwrap()
            .path(),
        local.path()
    );
    assert_eq!(
        db.query_active_songs("", SongSort::default(), None, 100)
            .unwrap()
            .items
            .len(),
        2
    );
}

#[test]
fn snapshot_keeps_same_name_distinct_playlists_and_their_own_members() {
    let (_directory, db, root) = database();
    let local_song = song(root, "local.flac", "Local", "Artist");
    SongRepository::upsert(&db, &local_song).unwrap();
    let local_playlist = PlaylistId::new();
    let local_name = "Ａ".repeat(40);
    db.create(local_playlist, root, &local_name).unwrap();
    db.add_member(local_playlist, local_song.id(), 0).unwrap();
    let local_members = db.members(local_playlist).unwrap();
    let binding = SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    db.bind_sync_replica_for_enrollment(binding, root).unwrap();
    let imported_song = SongId::new();
    let imported_playlist = PlaylistId::new();
    let imported_member = PlaylistItemId::new();
    let mut snapshot = page(binding);
    snapshot.records = vec![
        song_record(imported_song),
        object(
            ObjectKey::Playlist(imported_playlist),
            serde_json::json!({"playlist_uuid":imported_playlist,"display_name":"a".repeat(40),"created_at":77,"members":[{"song_uuid":imported_song,"member_uuid":imported_member,"position":0,"added_at":78}]}),
        ),
    ];
    db.apply_snapshot_page(DeviceId::new(), &snapshot).unwrap();
    assert_eq!(db.list(root).unwrap().len(), 2);
    assert_eq!(
        db.name(local_playlist).unwrap().as_deref(),
        Some(local_name.as_str())
    );
    assert_eq!(db.members(local_playlist).unwrap(), local_members);
    let imported_name = db.name(imported_playlist).unwrap().unwrap();
    assert_ne!(
        crate::domain::text::playlist_name_key(&imported_name),
        crate::domain::text::playlist_name_key(&local_name)
    );
    crate::domain::text::validate_playlist_name(&imported_name).unwrap();
    assert!(imported_name.ends_with(&format!(" ({})", &imported_playlist.to_string()[..8])));
    let imported_members = db.members(imported_playlist).unwrap();
    assert_eq!(imported_members.len(), 1);
    assert_eq!(imported_members[0].song(), imported_song);
    assert_eq!(imported_members[0].id(), imported_member);
    // Replaying the confirmed base keeps the same suffix and membership IDs.
    let imported_payload = serde_json::json!({"playlist_uuid":imported_playlist,"display_name":"a".repeat(40),"created_at":77,"members":[{"song_uuid":imported_song,"member_uuid":imported_member,"position":0,"added_at":78}]});
    db.writer
        .run(move |connection| {
            super::super::sync_projection::project_confirmed_song_state(
                connection,
                "playlist",
                &imported_playlist.to_string(),
                &imported_payload,
                Some(&root.to_string()),
            )
        })
        .unwrap();
    assert_eq!(db.name(imported_playlist).unwrap(), Some(imported_name));
    assert_eq!(db.members(imported_playlist).unwrap(), imported_members);
}
