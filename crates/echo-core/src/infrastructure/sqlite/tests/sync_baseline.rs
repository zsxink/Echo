use super::*;
use crate::application::ports::LocalSyncQueuePort;
use crate::domain::sync::{AuthorityId, Epoch, LibraryId, ObjectKey, SyncBinding};

fn binding() -> SyncBinding {
    SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    }
}

#[test]
fn initial_baseline_preserves_chain_current_state_and_undoable_song() {
    let (directory, database, root) = database();
    let mut imported = song(root, "media/song.flac", "Title", "Artist");
    imported.apply_scan_facts(
        "a".repeat(64),
        17,
        1,
        crate::domain::media::AudioFormat::Flac,
    );
    SongRepository::upsert(&database, &imported).unwrap();
    SongRepository::set_favorite(&database, imported.id(), true).unwrap();
    SongRepository::set_availability(&database, imported.id(), SongAvailability::PendingDelete)
        .unwrap();
    let playlist = PlaylistId::new();
    database.create(playlist, root, "Playlist").unwrap();
    database.add_member(playlist, imported.id(), 0).unwrap();
    let device = database.current_device_id().to_string();
    let uuid = imported.id().to_string();
    database.writer.run(move |connection| {
        connection.execute("INSERT INTO sync_device_play_counts(song_uuid,device_id,play_count) VALUES(?1,?2,23)",params![uuid,device]).map_err(storage)?;
        connection.execute("INSERT INTO sync_local_delete_intents(intent_id,final_mutation_id,object_type,object_uuid,created_at,undo_deadline,state) VALUES(?1,?2,'song',?3,1,2,'open')",params![uuid::Uuid::new_v4().to_string(),uuid::Uuid::new_v4().to_string(),uuid]).map_err(storage)?;
        Ok(())
    }).unwrap();
    let original = database.pending_local_mutations(32).unwrap();
    let binding = binding();
    database.bind_sync_library(binding, true, root).unwrap();
    assert_eq!(
        database.ensure_host_sync_baseline(binding, root).unwrap(),
        4
    );
    let queue = database.pending_local_mutations(32).unwrap();
    assert_eq!(queue.len(), original.len() + 4);
    for (before, after) in original.iter().zip(&queue) {
        assert_eq!(before.mutation_id, after.mutation_id);
        assert_eq!(before.payload, after.payload);
        assert_eq!(before.origin_seq, after.origin_seq);
    }
    for pair in queue.windows(2) {
        assert_eq!(pair[1].origin_seq, pair[0].origin_seq + 1);
        assert_eq!(pair[1].previous_mutation_id, Some(pair[0].mutation_id));
    }
    let baseline = &queue[original.len()..];
    assert_eq!(baseline[0].object, ObjectKey::Song(imported.id()));
    assert_eq!(baseline[1].object, ObjectKey::Favorite(imported.id()));
    let favorite = &baseline[1].payload;
    assert_eq!(favorite["is_favorite"], true);
    let stats = &baseline[2].payload;
    assert_eq!(
        stats["by_device"][database.current_device_id().to_string()],
        23
    );
    assert_eq!(stats["song_uuid"], imported.id().to_string());
    assert_eq!(
        stats["updated_by_device_id"],
        database.current_device_id().to_string()
    );
    assert_eq!(stats["revision"], 0);
    assert_eq!(stats["hlc"]["wall_secs"], 0);
    let playlist_payload = &baseline[3].payload;
    assert_eq!(
        playlist_payload["members"][0]["song_uuid"],
        imported.id().to_string()
    );
    assert!(playlist_payload["members"][0]["member_uuid"]
        .as_str()
        .is_some());
    assert!(baseline
        .iter()
        .all(|mutation| mutation.payload.get("deleted").is_none()));
    assert!(baseline[0].payload.get("availability").is_none());
    assert_eq!(baseline[0].resource_refs.len(), 1);
    assert_eq!(
        database.local_sync_resource_manifests(root).unwrap(),
        baseline[0].resource_refs
    );
    drop(database);
    let database = SqliteDatabase::open(directory.path().join("echo.db")).unwrap();
    assert_eq!(
        database.ensure_host_sync_baseline(binding, root).unwrap(),
        0
    );
    assert_eq!(
        database.pending_local_mutations(32).unwrap().len(),
        queue.len()
    );
}

#[test]
fn failed_baseline_rolls_back_all_snapshots_and_marker_then_retries() {
    let (_directory, database, root) = database();
    let mut imported = song(root, "media/song.flac", "Title", "Artist");
    imported.apply_scan_facts(
        "a".repeat(64),
        17,
        1,
        crate::domain::media::AudioFormat::Flac,
    );
    SongRepository::upsert(&database, &imported).unwrap();
    let binding = binding();
    database.bind_sync_library(binding, true, root).unwrap();
    let original = database.pending_local_mutations(32).unwrap();
    database.writer.run(|connection| {
        connection.execute_batch("CREATE TRIGGER reject_baseline_stats BEFORE INSERT ON sync_mutations WHEN NEW.object_type='play-stats' BEGIN SELECT RAISE(ABORT,'injected failure'); END;").map_err(storage)
    }).unwrap();
    assert!(database.ensure_host_sync_baseline(binding, root).is_err());
    assert_eq!(
        database.pending_local_mutations(32).unwrap().len(),
        original.len()
    );
    database
        .with_reader(|connection| {
            let count: i64 = connection
                .query_row("SELECT COUNT(*) FROM sync_host_baselines", [], |row| {
                    row.get(0)
                })
                .map_err(storage)?;
            assert_eq!(count, 0);
            Ok(())
        })
        .unwrap();
    database
        .writer
        .run(|connection| {
            connection
                .execute_batch("DROP TRIGGER reject_baseline_stats;")
                .map_err(storage)
        })
        .unwrap();
    assert_eq!(
        database.ensure_host_sync_baseline(binding, root).unwrap(),
        3
    );
    assert_eq!(
        database.pending_local_mutations(32).unwrap().len(),
        original.len() + 3
    );
}

#[test]
fn baseline_rejects_replica_and_foreign_binding() {
    let (_directory, database, root) = database();
    let binding = binding();
    database.bind_sync_library(binding, false, root).unwrap();
    assert!(database.ensure_host_sync_baseline(binding, root).is_err());
    assert!(database
        .ensure_host_sync_baseline(
            SyncBinding {
                authority_id: AuthorityId::new(),
                ..binding
            },
            root
        )
        .is_err());
    assert!(database.pending_local_mutations(8).unwrap().is_empty());
}

#[test]
fn baseline_requires_audio_scan_facts_and_captures_legacy_plays() {
    let (_directory, database, root) = database();
    let mut imported = song(root, "media/song.flac", "Title", "Artist");
    SongRepository::upsert(&database, &imported).unwrap();
    SongRepository::set_play_count(&database, imported.id(), 9).unwrap();
    let binding = binding();
    database.bind_sync_library(binding, true, root).unwrap();
    assert!(database.ensure_host_sync_baseline(binding, root).is_err());
    assert_eq!(database.pending_local_mutations(16).unwrap().len(), 1);
    imported.apply_scan_facts(
        "b".repeat(64),
        17,
        1,
        crate::domain::media::AudioFormat::Flac,
    );
    SongRepository::upsert(&database, &imported).unwrap();
    assert_eq!(
        database.ensure_host_sync_baseline(binding, root).unwrap(),
        3
    );
    let queue = database.pending_local_mutations(16).unwrap();
    assert_eq!(
        queue.last().unwrap().payload["by_device"][database.current_device_id().to_string()],
        9
    );
    assert_eq!(
        database
            .device_play_counts(imported.id())
            .unwrap()
            .values()
            .sum::<u64>(),
        9
    );
}

#[test]
fn baseline_keeps_pending_tombstones_and_excludes_another_root() {
    let (directory, database, root) = database();
    let imported = song(root, "media/deleted.flac", "Deleted", "Artist");
    SongRepository::upsert(&database, &imported).unwrap();
    let doomed = imported.id();
    database
        .with_tx(Box::new(move |tx| tx.delete_song(doomed)))
        .unwrap();
    let other = LibraryRootId::new();
    LibraryRepository::upsert(
        &database,
        &LibraryRoot::new(other, directory.path().join("other"), false, true),
    )
    .unwrap();
    SongRepository::upsert(
        &database,
        &song(other, "media/foreign.flac", "Foreign", "Artist"),
    )
    .unwrap();
    let original = database.pending_local_mutations(16).unwrap();
    let binding = binding();
    database.bind_sync_library(binding, true, root).unwrap();
    assert_eq!(
        database.ensure_host_sync_baseline(binding, root).unwrap(),
        0
    );
    assert_eq!(database.pending_local_mutations(16).unwrap(), original);
    assert!(database
        .local_sync_resource_manifests(root)
        .unwrap()
        .is_empty());
}

#[test]
fn host_baseline_confirmation_preserves_the_existing_playable_catalog() {
    use crate::application::ports::HostCommitLedgerPort;

    let (_directory, database, root) = database();
    let mut originals = Vec::new();
    for (path, hash) in [("local/one.flac", 'a'), ("local/two.flac", 'b')] {
        let mut imported = song(root, path, "Title", "Artist");
        imported.apply_scan_facts(
            hash.to_string().repeat(64),
            17,
            123,
            crate::domain::media::AudioFormat::Flac,
        );
        SongRepository::upsert(&database, &imported).unwrap();
        SongRepository::set_favorite(&database, imported.id(), true).unwrap();
        originals.push(imported);
    }
    let binding = binding();
    database.bind_sync_library(binding, true, root).unwrap();
    database.ensure_host_sync_baseline(binding, root).unwrap();
    for mutation in database.pending_local_mutations(100).unwrap() {
        let staged = database.stage_confirmation(binding, &mutation).unwrap();
        database
            .mark_published(staged.checkpoint.expect("local mutation accepted"))
            .unwrap();
    }
    assert!(database.pending_local_mutations(100).unwrap().is_empty());
    assert_eq!(
        database
            .query_active_songs("", SongSort::default(), None, 100)
            .unwrap()
            .items
            .len(),
        originals.len()
    );
    for original in originals {
        let actual = SongRepository::by_id(&database, original.id())
            .unwrap()
            .unwrap();
        assert_eq!(actual.availability(), SongAvailability::Available);
        assert_eq!(actual.path(), original.path());
        assert_eq!(actual.file_mtime_ns(), Some(123));
        assert!(actual.favorite());
    }
}
