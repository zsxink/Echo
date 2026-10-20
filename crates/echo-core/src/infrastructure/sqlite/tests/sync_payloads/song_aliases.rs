use super::*;
use crate::application::ports::{
    HostCommitLedgerPort, LocalSyncQueuePort, RemoteApplyPort, RemoteCommit,
};
use crate::domain::sync::{
    AuthorityId, CommitSeq, DeviceId, Epoch, LibraryId, MutationCandidate, MutationId, ObjectKey,
    SyncBinding,
};

fn confirm(db: &SqliteDatabase, binding: SyncBinding, candidate: &MutationCandidate) {
    let staged = db
        .stage_confirmation(binding, candidate)
        .expect("confirmation");
    db.mark_published(
        staged
            .checkpoint
            .unwrap_or_else(|| panic!("rejected {:?}: {}", candidate.object, staged.outcome_json)),
    )
    .expect("publish");
}

fn candidate(device: DeviceId, song: SongId, title: &str) -> MutationCandidate {
    let mut payload = sync_song_payload(song, title);
    payload["blake3_hash"] = serde_json::json!("a".repeat(64));
    MutationCandidate {
        resource_refs: Vec::new(),
        mutation_id: MutationId::new(),
        origin_device_id: device,
        origin_seq: 1,
        previous_mutation_id: None,
        object: ObjectKey::Song(song),
        base_object_version: None,
        payload,
    }
}

fn authorize_peer(host: &SqliteDatabase, binding: SyncBinding, second_device: DeviceId) {
    host.authorize_sync_peer(
        binding,
        second_device,
        "Replica".to_owned(),
        "credential-handle".to_owned(),
    )
    .expect("peer");
}

#[test]
fn concurrent_same_hash_import_aliases_references_and_keeps_reimport_history() {
    let (_directory, host, root) = database();
    let (_replica_directory, replica, replica_root) = database();
    let binding = SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    host.bind_sync_library(binding, true, root)
        .expect("host binding");
    replica
        .bind_sync_library(binding, false, replica_root)
        .expect("replica binding");
    let first_device = DeviceId::from_uuid(host.current_device_id().as_uuid());
    let second_device = DeviceId::new();
    authorize_peer(&host, binding, second_device);
    let canonical = SongId::new();
    let alias = SongId::new();
    let first = candidate(first_device, canonical, "First");
    confirm(&host, binding, &first);
    let mut duplicate = candidate(second_device, alias, "Duplicate");
    duplicate.payload["added_at"] = serde_json::json!(999);
    let immutable = serde_json::to_string(&duplicate).expect("original body");
    confirm(&host, binding, &duplicate);
    assert_eq!(serde_json::to_string(&duplicate).unwrap(), immutable);
    assert!(host
        .stage_confirmation(binding, &duplicate)
        .unwrap()
        .checkpoint
        .is_some());
    let favorite = MutationCandidate {
        mutation_id: MutationId::new(),
        origin_seq: 2,
        previous_mutation_id: Some(duplicate.mutation_id),
        object: ObjectKey::Favorite(alias),
        payload: serde_json::json!({"song_uuid":alias,"is_favorite":true,"favorited_at":15}),
        ..duplicate.clone()
    };
    confirm(&host, binding, &favorite);
    let stats = MutationCandidate {
        mutation_id: MutationId::new(),
        origin_seq: 3,
        previous_mutation_id: Some(favorite.mutation_id),
        object: ObjectKey::PlayStats(alias),
        payload: serde_json::json!({"song_uuid":alias,"revision":1,"updated_by_device_id":second_device.as_uuid(),"hlc":{"wall_secs":1,"counter":0},"by_device":{second_device.as_uuid().to_string():3}}),
        ..favorite
    };
    confirm(&host, binding, &stats);
    let override_change = MutationCandidate {
        mutation_id: MutationId::new(),
        origin_seq: 4,
        previous_mutation_id: Some(stats.mutation_id),
        object: ObjectKey::Override(alias),
        payload: serde_json::json!({"song_uuid":alias,"revision":1,"updated_by_device_id":second_device.as_uuid(),"hlc":{"wall_secs":1,"counter":0},"title":"Override","artist":null,"album":null,"lyrics_text":null}),
        ..stats
    };
    confirm(&host, binding, &override_change);
    let playlist = crate::domain::ids::PlaylistId::new();
    let playlist_change = MutationCandidate {
        mutation_id: MutationId::new(),
        origin_seq: 5,
        previous_mutation_id: Some(override_change.mutation_id),
        object: ObjectKey::Playlist(playlist),
        payload: serde_json::json!({"playlist_uuid":playlist,"display_name":"Playlist","created_at":1,"members":[{"song_uuid":alias,"member_uuid":crate::domain::ids::PlaylistItemId::new(),"position":0,"added_at":1}]}),
        ..override_change
    };
    confirm(&host, binding, &playlist_change);
    assert_canonical_state(&host, canonical, alias);
    apply_all(&host, &replica, binding);
    assert_canonical_state(&replica, canonical, alias);
    let deleted = MutationCandidate {
        mutation_id: MutationId::new(),
        origin_seq: 6,
        previous_mutation_id: Some(playlist_change.mutation_id),
        object: ObjectKey::Song(alias),
        payload: serde_json::json!({"tombstone":true}),
        ..playlist_change
    };
    confirm(&host, binding, &deleted);
    let fresh = SongId::new();
    let reimport = MutationCandidate {
        mutation_id: MutationId::new(),
        origin_seq: 7,
        previous_mutation_id: Some(deleted.mutation_id),
        ..candidate(second_device, fresh, "Reimport")
    };
    confirm(&host, binding, &reimport);
    let stale = MutationCandidate {
        mutation_id: MutationId::new(),
        origin_seq: 8,
        previous_mutation_id: Some(reimport.mutation_id),
        ..duplicate
    };
    let result = host
        .stage_confirmation(binding, &stale)
        .expect("final rejection");
    assert!(result.checkpoint.is_none());
    assert_reimport_history(&host, alias, canonical, fresh);
}

fn assert_reimport_history(host: &SqliteDatabase, alias: SongId, canonical: SongId, fresh: SongId) {
    host.with_reader(|connection| {
        let target: String = connection.query_row("SELECT canonical_song_uuid FROM sync_song_aliases WHERE alias_song_uuid=?1",params![alias.to_string()],|row|row.get(0)).map_err(super::super::super::support::storage)?;
        assert_eq!(target,canonical.to_string());
        let fresh_live: bool = connection.query_row("SELECT EXISTS(SELECT 1 FROM songs WHERE uuid=?1 AND availability!='pending_delete')",params![fresh.to_string()],|row|row.get(0)).map_err(super::super::super::support::storage)?;
        assert!(fresh_live);
        Ok(())
    }).expect("history");
}

fn assert_canonical_state(db: &SqliteDatabase, canonical: SongId, alias: SongId) {
    db.with_reader(|connection| {
        let state: (i64, i64, i64) = connection
            .query_row(
                "SELECT is_favorite,play_count,added_at FROM songs WHERE uuid=?1",
                params![canonical.to_string()],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .map_err(super::super::super::support::storage)?;
        assert_eq!(state, (1, 3, 1));
        let mapping: String = connection
            .query_row(
                "SELECT canonical_song_uuid FROM sync_song_aliases WHERE alias_song_uuid=?1",
                params![alias.to_string()],
                |row| row.get(0),
            )
            .map_err(super::super::super::support::storage)?;
        assert_eq!(mapping, canonical.to_string());
        let override_title: String = connection
            .query_row(
                "SELECT title FROM song_overrides WHERE song_uuid=?1",
                params![canonical.to_string()],
                |row| row.get(0),
            )
            .map_err(super::super::super::support::storage)?;
        assert_eq!(override_title, "Override");
        let member_song: String = connection
            .query_row("SELECT song_uuid FROM playlist_songs", [], |row| row.get(0))
            .map_err(super::super::super::support::storage)?;
        assert_eq!(member_song, canonical.to_string());
        let count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM songs WHERE availability!='pending_delete'",
                [],
                |row| row.get(0),
            )
            .map_err(super::super::super::support::storage)?;
        assert_eq!(count, 1);
        Ok(())
    })
    .expect("canonical state");
}

fn apply_all(host: &SqliteDatabase, replica: &SqliteDatabase, binding: SyncBinding) {
    let peer = DeviceId::from_uuid(host.current_device_id().as_uuid());
    let anchor = replica.replica_checkpoint(peer).unwrap();
    let commits = host
        .published_commits_after(binding, anchor.sequence, anchor.digest, 100)
        .expect("commits");
    let commits: Vec<_> = commits
        .into_iter()
        .map(|commit| {
            let body: serde_json::Value = serde_json::from_str(&commit.canonical_json).unwrap();
            RemoteCommit {
                sequence: commit.sequence,
                previous_digest: commit.previous_digest,
                digest: commit.digest,
                object: serde_json::from_value(body["mutation"]["object"].clone()).unwrap(),
                origin_device_id: serde_json::from_value(
                    body["mutation"]["origin_device_id"].clone(),
                )
                .unwrap(),
                mutation_id: serde_json::from_value(body["mutation"]["mutation_id"].clone())
                    .unwrap(),
                payload_json: serde_json::to_string(&body["mutation"]["payload"]).unwrap(),
                is_tombstone: body["mutation"]["payload"] == serde_json::json!({"tombstone":true}),
                canonical_json: commit.canonical_json,
            }
        })
        .collect();
    replica
        .apply_remote_batch(binding, peer, anchor, &commits)
        .expect("remote apply");
}

#[test]
fn different_hash_same_path_gets_a_stable_safe_destination() {
    let (_directory, host, root) = database();
    let binding = SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    host.bind_sync_library(binding, true, root).unwrap();
    let device = DeviceId::from_uuid(host.current_device_id().as_uuid());
    let first = candidate(device, SongId::new(), "Same");
    confirm(&host, binding, &first);
    let mut second = MutationCandidate {
        origin_seq: 2,
        previous_mutation_id: Some(first.mutation_id),
        ..candidate(device, SongId::new(), "Same")
    };
    second.payload["blake3_hash"] = serde_json::json!("b".repeat(64));
    confirm(&host, binding, &second);
    host.with_reader(|connection| {
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM songs", [], |row| row.get(0))
            .map_err(super::super::super::support::storage)?;
        assert_eq!(count, 2);
        let identity = super::super::super::sync_manifests::identity(second.object).1;
        let path: String = connection
            .query_row(
                "SELECT relative_path FROM songs WHERE uuid=?1",
                params![identity],
                |row| row.get(0),
            )
            .map_err(super::super::super::support::storage)?;
        // Relative paths are device-local facts. `Same.flac` already belongs to
        // the local song, so the peer's object cannot take it: the projection
        // reserves a deterministic pending destination that resource
        // publication later fills with this object's verified bytes. The
        // reservation is derived from the object identity, so replaying the
        // same commit lands on the same row instead of accumulating names.
        assert_eq!(
            path,
            format!("echo/media/pending/{identity}-0"),
            "conflicting remote object must keep its reserved destination"
        );
        let local: String = connection
            .query_row(
                "SELECT relative_path FROM songs WHERE uuid=?1",
                params![super::super::super::sync_manifests::identity(first.object).1],
                |row| row.get(0),
            )
            .map_err(super::super::super::support::storage)?;
        assert_eq!(local, "Same.flac", "the local path keeps its owner");
        Ok(())
    })
    .unwrap();
}

#[test]
fn alias_confirmation_preserves_pending_queue_bytes_and_effective_favorite() {
    use crate::application::ports::{LocalSyncQueuePort, SongRepository};
    let (_host_directory, host, root) = database();
    let (_replica_directory, replica, replica_root) = database();
    let binding = SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    host.bind_sync_library(binding, true, root).unwrap();
    replica
        .bind_sync_library(binding, false, replica_root)
        .unwrap();
    let device = DeviceId::from_uuid(host.current_device_id().as_uuid());
    let canonical = SongId::new();
    let alias = SongId::new();
    let first = candidate(device, canonical, "First");
    confirm(&host, binding, &first);
    let duplicate = MutationCandidate {
        origin_seq: 2,
        previous_mutation_id: Some(first.mutation_id),
        ..candidate(device, alias, "Duplicate")
    };
    confirm(&host, binding, &duplicate);
    let favorite = MutationCandidate {
        mutation_id: MutationId::new(),
        origin_seq: 3,
        previous_mutation_id: Some(duplicate.mutation_id),
        object: ObjectKey::Favorite(alias),
        payload: serde_json::json!({"song_uuid":alias,"is_favorite":true,"favorited_at":1}),
        ..duplicate
    };
    confirm(&host, binding, &favorite);
    replica.writer.run(move |connection| {
        connection.execute("INSERT INTO songs(uuid,library_root_uuid,relative_path,normalized_relative_path,title_sort,artist_sort,album_sort,added_at,availability,created_at,updated_at) VALUES(?1,?2,'Duplicate.flac','duplicate.flac','','','',999,'available',1,1)",params![alias.to_string(),replica_root.to_string()]).map_err(super::super::super::support::storage)?;
        Ok(())
    }).unwrap();
    SongRepository::set_favorite(&replica, alias, false).unwrap();
    let before = serde_json::to_string(&replica.pending_local_mutations(100).unwrap()).unwrap();
    apply_all(&host, &replica, binding);
    assert_eq!(
        before,
        serde_json::to_string(&replica.pending_local_mutations(100).unwrap()).unwrap()
    );
    let song = SongRepository::by_id(&replica, canonical).unwrap().unwrap();
    assert!(!song.favorite());
    let alias_row = SongRepository::by_id(&replica, alias).unwrap().unwrap();
    assert_eq!(alias_row.root(), replica_root);
    assert_eq!(
        alias_row.availability(),
        crate::domain::entities::SongAvailability::PendingDelete
    );
}

#[test]
fn alias_confirmation_keeps_the_replicas_available_local_audio() {
    let (_host_directory, host, root) = database();
    let (_replica_directory, replica, replica_root) = database();
    let binding = SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    host.bind_sync_library(binding, true, root).unwrap();
    replica
        .bind_sync_library(binding, false, replica_root)
        .unwrap();
    let host_device = DeviceId::from_uuid(host.current_device_id().as_uuid());
    let replica_device = DeviceId::from_uuid(replica.current_device_id().as_uuid());
    authorize_peer(&host, binding, replica_device);
    let canonical = SongId::new();
    let first = candidate(host_device, canonical, "Host");
    confirm(&host, binding, &first);

    let mut local = song(replica_root, "local/audio.flac", "Local", "Artist");
    local.apply_scan_facts(
        "a".repeat(64),
        17,
        123,
        crate::domain::media::AudioFormat::Flac,
    );
    SongRepository::upsert(&replica, &local).unwrap();
    SongRepository::set_favorite(&replica, local.id(), true).unwrap();
    let pending = replica.pending_local_mutations(100).unwrap();
    confirm(&host, binding, &pending[0]);
    let queued_favorite = serde_json::to_string(&pending[1]).unwrap();
    apply_all(&host, &replica, binding);

    let merged = SongRepository::by_id(&replica, canonical).unwrap().unwrap();
    assert_eq!(merged.availability(), SongAvailability::Available);
    assert_eq!(merged.path().display(), "local/audio.flac");
    assert_eq!(merged.file_mtime_ns(), Some(123));
    assert!(merged.favorite());
    assert_eq!(
        replica
            .query_active_songs("", SongSort::default(), None, 100)
            .unwrap()
            .items
            .len(),
        1
    );
    assert_eq!(
        serde_json::to_string(&replica.pending_local_mutations(100).unwrap()[0]).unwrap(),
        queued_favorite
    );
    assert_eq!(
        SongRepository::by_id(&host, canonical)
            .unwrap()
            .unwrap()
            .path()
            .display(),
        "Host.flac"
    );
}

#[test]
fn duplicate_audio_keeps_differing_sidecars_as_durable_alias_evidence() {
    use crate::domain::sync::{ResourceDigest, SharedResourceKind, SharedResourceManifest};
    let (_directory, host, root) = database();
    let binding = SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    host.bind_sync_library(binding, true, root).unwrap();
    let device = DeviceId::from_uuid(host.current_device_id().as_uuid());
    let canonical = SongId::new();
    let alias = SongId::new();
    let original = SharedResourceManifest::new(
        SharedResourceKind::LyricsSidecar,
        ResourceDigest::from_bytes([1; 32]),
        1,
        "text/plain".to_owned(),
        "media/first.lrc",
    )
    .unwrap();
    let distinct = SharedResourceManifest::new(
        SharedResourceKind::LyricsSidecar,
        ResourceDigest::from_bytes([2; 32]),
        2,
        "text/plain".to_owned(),
        "media/second.lrc",
    )
    .unwrap();
    let first = MutationCandidate {
        resource_refs: vec![original.clone()],
        ..candidate(device, canonical, "First")
    };
    confirm(&host, binding, &first);
    let second = MutationCandidate {
        origin_seq: 2,
        previous_mutation_id: Some(first.mutation_id),
        resource_refs: vec![distinct.clone()],
        ..candidate(device, alias, "Second")
    };
    confirm(&host, binding, &second);
    host.with_reader(|connection| {
        assert_eq!(
            super::super::super::sync_manifests::load(connection, "song", &canonical.to_string())?,
            vec![original]
        );
        assert_eq!(
            super::super::super::sync_manifests::load(connection, "song", &alias.to_string())?,
            vec![distinct]
        );
        Ok(())
    })
    .unwrap();
}

#[test]
fn song_alias_keeps_colliding_local_member_identity_until_playlist_confirmation() {
    let (_directory, db, root) = database();
    let canonical = SongId::new();
    let alias = SongId::new();
    let playlist = crate::domain::ids::PlaylistId::new();
    let canonical_member = crate::domain::ids::PlaylistItemId::new();
    let alias_member = crate::domain::ids::PlaylistItemId::new();
    db.writer.run(move |connection| {
        let tx=connection.transaction().map_err(super::super::super::support::storage)?;
        for (id,path) in [(canonical,"canonical.flac"),(alias,"alias.flac")] {
            tx.execute("INSERT INTO songs(uuid,library_root_uuid,relative_path,normalized_relative_path,title_sort,artist_sort,album_sort,added_at,availability,created_at,updated_at) VALUES(?1,?2,?3,?3,'','','',1,'available',1,1)",params![id.to_string(),root.to_string(),path]).map_err(super::super::super::support::storage)?;
        }
        tx.execute("INSERT INTO playlists(uuid,library_root_uuid,display_name,normalized_name_key,created_at,updated_at) VALUES(?1,?2,'Playlist','playlist',1,1)",params![playlist.to_string(),root.to_string()]).map_err(super::super::super::support::storage)?;
        for (id,member,position) in [(canonical,canonical_member,0),(alias,alias_member,1)] {
            tx.execute("INSERT INTO playlist_songs(playlist_uuid,song_uuid,member_uuid,position,added_at) VALUES(?1,?2,?3,?4,1)",params![playlist.to_string(),id.to_string(),member.to_string(),position]).map_err(super::super::super::support::storage)?;
        }
        super::super::super::sync_song_aliases::persist(&tx,&alias.to_string(),&canonical.to_string(),1)?;
        super::super::super::sync_song_aliases::project(&tx,&alias.to_string(),&canonical.to_string())?;
        let retained:String=tx.query_row("SELECT member_uuid FROM playlist_songs WHERE song_uuid=?1",params![alias.to_string()],|row|row.get(0)).map_err(super::super::super::support::storage)?;
        assert_eq!(retained,alias_member.to_string());
        tx.commit().map_err(super::super::super::support::storage)
    }).unwrap();
}

/// Exercise the real SQLite transaction ports and actual temporary files,
/// including a remote alias confirmation in the middle of the undo window.
struct DeleteAliasRace {
    stack: ScanStack,
    deps: ScanDeps,
    clock: ManualClock,
    host: SqliteDatabase,
    binding: SyncBinding,
    canonical: SongId,
    alias: SongId,
    operation: crate::domain::ids::OperationId,
}

fn alias_during_delete() -> DeleteAliasRace {
    use crate::application::delete::DeleteSongs;
    let stack = ScanStack::new();
    let mut deps = stack.deps();
    let clock = ManualClock::new();
    // Intent timestamps come from the SQLite writer's wall clock. Align the
    // controllable use-case clock so the deadline satisfies its durable CHECK.
    let wall_now = u64::try_from(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
    )
    .unwrap();
    clock.advance_ms(wall_now.saturating_sub(clock.wall_ms()));
    deps.clock = Arc::new(clock.clone());
    let database_directory = stack.library_dir.parent().unwrap();
    let host = SqliteDatabase::open(database_directory.join("host.db")).unwrap();
    let host_root = LibraryRootId::new();
    LibraryRepository::upsert(
        &host,
        &LibraryRoot::new(host_root, database_directory.join("host"), true, true),
    )
    .unwrap();
    let binding = SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    host.bind_sync_library(binding, true, host_root).unwrap();
    stack
        .database
        .bind_sync_library(binding, false, stack.root)
        .unwrap();

    stack.write("media/alias.flac", b"identical audio");
    stack.write("media/alias.lrc", b"original sidecar");
    stack.write("media/canonical.flac", b"canonical media remains intact");
    let mut alias = song(stack.root, "media/alias.flac", "Alias", "Artist");
    let hash = deps.hasher.hash(stack.root, alias.path()).unwrap();
    alias.apply_scan_facts(hash.clone(), 15, 1, crate::domain::media::AudioFormat::Flac);
    SongRepository::upsert(stack.database.as_ref(), &alias).unwrap();
    let operation = DeleteSongs::new(&deps)
        .delete(stack.root, alias.id())
        .unwrap()
        .operation;
    stack.database.with_reader(|connection| {
        let open: i64 = connection.query_row("SELECT COUNT(*) FROM sync_local_delete_intents WHERE intent_id=?1 AND state='open'", params![operation.to_string()], |row| row.get(0)).map_err(super::super::super::support::storage)?;
        assert_eq!(open, 1, "the alias must arrive during a persisted open delete intent");
        Ok(())
    }).unwrap();

    let device = DeviceId::from_uuid(host.current_device_id().as_uuid());
    let canonical = SongId::new();
    let mut first = candidate(device, canonical, "Canonical");
    first.payload["relative_path"] = serde_json::json!("media/canonical.flac");
    first.payload["blake3_hash"] = serde_json::json!(hash);
    confirm(&host, binding, &first);
    let duplicate = MutationCandidate {
        origin_seq: 2,
        previous_mutation_id: Some(first.mutation_id),
        ..candidate(device, alias.id(), "Alias")
    };
    let mut duplicate = duplicate;
    duplicate.payload["blake3_hash"] = first.payload["blake3_hash"].clone();
    confirm(&host, binding, &duplicate);
    apply_all(&host, stack.database.as_ref(), binding);
    SongRepository::set_availability(
        stack.database.as_ref(),
        canonical,
        SongAvailability::Available,
    )
    .unwrap();
    DeleteAliasRace {
        stack,
        deps,
        clock,
        host,
        binding,
        canonical,
        alias: alias.id(),
        operation,
    }
}

#[test]
fn alias_confirmation_during_delete_undo_keeps_alias_hidden_and_original_file_evidence() {
    use crate::application::delete::RestoreDeletedOperation;
    use crate::application::ports::{LocalSyncQueuePort, OperationJournalRepository};
    let DeleteAliasRace {
        stack,
        deps,
        canonical,
        alias,
        operation,
        ..
    } = alias_during_delete();
    let before = stack.database.items(operation).unwrap();
    let queued =
        serde_json::to_string(&stack.database.pending_local_mutations(100).unwrap()).unwrap();
    assert_eq!(
        RestoreDeletedOperation::new(&deps)
            .restore(stack.root, operation)
            .unwrap(),
        alias
    );
    let after = stack.database.items(operation).unwrap();
    for (before, after) in before.iter().zip(&after) {
        assert_eq!(after.state, OperationState::Restored);
        assert_eq!(after.target_path, before.target_path);
        assert_eq!(after.staging_path, before.staging_path);
        assert_eq!(after.expected_hash, before.expected_hash);
        assert!(deps
            .fs
            .path_exists(stack.root, &before.target_path)
            .unwrap());
        assert!(!deps
            .fs
            .path_exists(stack.root, before.staging_path.as_ref().unwrap())
            .unwrap());
    }
    assert_eq!(
        std::fs::read(stack.library_dir.join("media/alias.flac")).unwrap(),
        b"identical audio"
    );
    assert_eq!(
        std::fs::read(stack.library_dir.join("media/alias.lrc")).unwrap(),
        b"original sidecar"
    );
    assert_eq!(
        std::fs::read(stack.library_dir.join("media/canonical.flac")).unwrap(),
        b"canonical media remains intact"
    );
    let alias_row = SongRepository::by_id(stack.database.as_ref(), alias)
        .unwrap()
        .unwrap();
    assert_eq!(alias_row.availability(), SongAvailability::PendingDelete);
    assert!(alias_row.path().display().starts_with(".echo-sync-alias/"));
    let active = stack
        .database
        .query_active_songs("", SongSort::default(), None, 100)
        .unwrap();
    assert_eq!(
        active.items.iter().map(Song::id).collect::<Vec<_>>(),
        vec![canonical]
    );
    assert_eq!(
        queued,
        serde_json::to_string(&stack.database.pending_local_mutations(100).unwrap()).unwrap()
    );
    stack
        .database
        .with_reader(|connection| {
            let state: String = connection
                .query_row(
                    "SELECT state FROM sync_local_delete_intents WHERE intent_id=?1",
                    params![operation.to_string()],
                    |row| row.get(0),
                )
                .map_err(super::super::super::support::storage)?;
            assert_eq!(state, "cancelled");
            Ok(())
        })
        .unwrap();
}

#[test]
fn alias_confirmation_during_delete_finalizes_original_files_and_normalizes_shared_tombstone() {
    use crate::application::ports::{LocalSyncQueuePort, OperationJournalRepository};
    let DeleteAliasRace {
        stack,
        deps,
        clock,
        host,
        binding,
        canonical,
        alias,
        operation,
    } = alias_during_delete();
    let before = stack.database.items(operation).unwrap();
    clock.advance_ms(10_001);
    let trash = FakeTrash::new();
    let report = FinalizeExpiredDeletes::new(&deps, &trash)
        .run(stack.root)
        .unwrap();
    assert_eq!(report.finalized, vec![operation]);
    assert_eq!(trash.calls(), vec![operation]);
    for (before, after) in before.iter().zip(stack.database.items(operation).unwrap()) {
        assert_eq!(after.state, OperationState::DatabaseFinalized);
        assert_eq!(after.target_path, before.target_path);
        assert_eq!(after.staging_path, before.staging_path);
        assert_eq!(after.expected_hash, before.expected_hash);
        assert_ne!(after.target_path.display(), "media/canonical.flac");
    }
    assert_eq!(
        std::fs::read(stack.library_dir.join("media/canonical.flac")).unwrap(),
        b"canonical media remains intact"
    );
    assert_eq!(
        SongRepository::by_id(stack.database.as_ref(), canonical)
            .unwrap()
            .unwrap()
            .availability(),
        SongAvailability::Available
    );
    let replica_device = DeviceId::from_uuid(stack.database.current_device_id().as_uuid());
    authorize_peer(&host, binding, replica_device);
    let queued = stack.database.pending_local_mutations(100).unwrap();
    let tombstone = queued
        .iter()
        .find(|candidate| candidate.payload == serde_json::json!({"tombstone":true}))
        .unwrap();
    assert_eq!(tombstone.object, ObjectKey::Song(alias));
    for mutation in &queued {
        confirm(&host, binding, mutation);
    }
    host.with_reader(|connection| {
        assert!(super::super::super::sync_projection::is_tombstoned(
            connection,
            "song",
            &canonical.to_string()
        )?);
        let alias_target =
            super::super::super::sync_song_aliases::resolve(connection, &alias.to_string())?;
        assert_eq!(alias_target, canonical.to_string());
        Ok(())
    })
    .unwrap();
    // Reimport uses a fresh identity after the old canonical lifetime ended.
    let fresh = SongId::new();
    let last = queued.last().unwrap();
    let mut reimport = candidate(replica_device, fresh, "Reimport");
    reimport.origin_seq = last.origin_seq + 1;
    reimport.previous_mutation_id = Some(last.mutation_id);
    reimport.payload["blake3_hash"] = queued[0].payload["blake3_hash"].clone();
    confirm(&host, binding, &reimport);
    host.with_reader(|connection| {
        assert_eq!(
            super::super::super::sync_song_aliases::resolve(connection, &fresh.to_string())?,
            fresh.to_string()
        );
        Ok(())
    })
    .unwrap();
}

#[test]
fn canonical_tombstone_holds_alias_delete_resources_for_explicit_new_identity_recovery() {
    use crate::application::delete::RestoreDeletedOperation;
    use crate::application::ports::OperationJournalRepository;
    let DeleteAliasRace {
        stack,
        deps,
        clock,
        host,
        binding,
        canonical,
        alias,
        operation,
    } = alias_during_delete();
    let original = stack.database.items(operation).unwrap();
    let last = host
        .published_commits_after(binding, CommitSeq::GENESIS, [0; 32], 10)
        .unwrap()
        .pop()
        .unwrap();
    let last: serde_json::Value = serde_json::from_str(&last.canonical_json).unwrap();
    let delete = MutationCandidate {
        mutation_id: MutationId::new(),
        origin_device_id: DeviceId::from_uuid(host.current_device_id().as_uuid()),
        origin_seq: 3,
        previous_mutation_id: Some(
            serde_json::from_value(last["mutation"]["mutation_id"].clone()).unwrap(),
        ),
        object: ObjectKey::Song(canonical),
        base_object_version: None,
        payload: serde_json::json!({"tombstone":true}),
        resource_refs: Vec::new(),
    };
    confirm(&host, binding, &delete);
    apply_all(&host, stack.database.as_ref(), binding);
    assert!(RestoreDeletedOperation::new(&deps)
        .restore(stack.root, operation)
        .is_err());
    assert!(SongRepository::set_availability(
        stack.database.as_ref(),
        alias,
        SongAvailability::Available
    )
    .is_err());
    clock.advance_ms(10_001);
    let trash = FakeTrash::new();
    assert!(FinalizeExpiredDeletes::new(&deps, &trash)
        .run(stack.root)
        .unwrap()
        .finalized
        .is_empty());
    assert!(trash.calls().is_empty());
    for (before, held) in original
        .iter()
        .zip(stack.database.items(operation).unwrap())
    {
        assert_eq!(held.state, OperationState::FailedRecoverable);
        assert_eq!(held.target_path, before.target_path);
        assert_eq!(held.staging_path, before.staging_path);
        assert_eq!(held.expected_hash, before.expected_hash);
        assert!(deps
            .fs
            .path_exists(stack.root, held.staging_path.as_ref().unwrap())
            .unwrap());
        assert!(!deps.fs.path_exists(stack.root, &held.target_path).unwrap());
    }
    assert_eq!(
        std::fs::read(stack.library_dir.join("media/canonical.flac")).unwrap(),
        b"canonical media remains intact"
    );
}
