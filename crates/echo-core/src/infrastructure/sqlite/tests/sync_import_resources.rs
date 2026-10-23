fn bind_empty_import_host(stack: &ScanStack) {
    let binding = crate::domain::sync::SyncBinding {
        library_id: crate::domain::sync::LibraryId::new(),
        authority_id: crate::domain::sync::AuthorityId::new(),
        epoch: crate::domain::sync::Epoch::INITIAL,
    };
    stack
        .database
        .bind_sync_library(binding, true, stack.root)
        .unwrap();
    assert_eq!(
        stack
            .database
            .ensure_host_sync_baseline(binding, stack.root)
            .unwrap(),
        0
    );
}

#[test]
fn sync_import_after_baseline_freezes_verified_audio_and_sidecar() {
    use crate::application::ports::LocalSyncQueuePort;
    use crate::domain::sync::{ObjectKey, SharedResourceKind};
    let stack = ScanStack::new();
    bind_empty_import_host(&stack);
    let audio = b"new audio after baseline";
    let lyrics = b"[00:01.00]new sidecar";
    seed_import_source(
        &stack,
        "media/Artist/Artist - Track.flac",
        audio,
        "Artist",
        "Track",
    );
    let sources = crate::application::testing::FakeImportSources::new();
    sources.add("new", "Track.flac", audio);
    sources.add_sidecar("new", "Track.lrc", lyrics);
    let deps = stack.deps();
    let report = crate::application::import::PlanImport::new(&deps, &sources)
        .run(
            stack.root,
            &[crate::application::ports::ImportSource::new("new").unwrap()],
        )
        .unwrap();
    let crate::application::import::ImportOutcome::Imported { song, target, .. } =
        &report.results[0]
    else {
        panic!("expected successful import: {:?}", report.results);
    };
    let queue = stack.database.pending_local_mutations(32).unwrap();
    let mutation = queue
        .iter()
        .find(|mutation| mutation.object == ObjectKey::Song(*song))
        .unwrap();
    assert_eq!(mutation.resource_refs.len(), 2);
    let audio_ref = mutation
        .resource_refs
        .iter()
        .find(|resource| resource.kind == SharedResourceKind::Audio)
        .unwrap();
    assert_eq!(
        audio_ref.digest.to_hex(),
        blake3::hash(audio).to_hex().to_string()
    );
    assert_eq!(audio_ref.length, u64::try_from(audio.len()).unwrap());
    assert_eq!(audio_ref.relative_path.as_str(), target.normalized());
    assert_eq!(audio_ref.media_type, "audio/flac");
    let sidecar = mutation
        .resource_refs
        .iter()
        .find(|resource| resource.kind == SharedResourceKind::LyricsSidecar)
        .unwrap();
    assert_eq!(
        sidecar.digest.to_hex(),
        blake3::hash(lyrics).to_hex().to_string()
    );
    assert_eq!(sidecar.length, u64::try_from(lyrics.len()).unwrap());
    assert_eq!(
        sidecar.relative_path.as_str(),
        "media/Artist/Artist - Track.lrc"
    );
    assert_eq!(mutation.payload["blake3_hash"], audio_ref.digest.to_hex());
    let frozen = mutation.clone();
    std::fs::write(
        stack.library_dir.join(target.display()),
        b"changed after commit",
    )
    .unwrap();
    let persisted = stack.database.pending_local_mutations(32).unwrap();
    assert_eq!(
        persisted
            .iter()
            .find(|entry| entry.mutation_id == frozen.mutation_id)
            .unwrap()
            .resource_refs,
        frozen.resource_refs
    );
}

#[test]
fn sync_import_rollback_leaves_no_song_resource_or_queued_snapshot_and_recovers() {
    use crate::application::ports::LocalSyncQueuePort;
    use crate::domain::sync::ObjectKey;
    let stack = ScanStack::new();
    bind_empty_import_host(&stack);
    let before = stack.database.pending_local_mutations(32).unwrap();
    let audio = b"recover this published audio";
    seed_import_source(
        &stack,
        "media/Artist/Artist - Recover.flac",
        audio,
        "Artist",
        "Recover",
    );
    let sources = crate::application::testing::FakeImportSources::new();
    sources.add("recover", "Recover.flac", audio);
    let mut deps = stack.deps();
    deps.uow = Arc::new(RollbackAfterWrites {
        database: Arc::clone(&stack.database),
    });
    let report = crate::application::import::PlanImport::new(&deps, &sources)
        .run(
            stack.root,
            &[crate::application::ports::ImportSource::new("recover").unwrap()],
        )
        .unwrap();
    let crate::application::import::ImportOutcome::SavedPending { operation, song: reserved, code, .. } = &report.results[0] else {
        panic!("the directory owns the saved import even though SQLite rolled back");
    };
    assert_eq!(*code, "library_saved_pending_projection");
    let durable = deps.control.durability().unwrap();
    let saved = durable.read_operation(stack.root, operation.as_uuid()).unwrap().unwrap();
    assert!(saved.batch.records.iter().any(|record| matches!(record.record(), crate::domain::library::PortableRecord::Song(song) if song.song_uuid == reserved.as_uuid())));
    assert!(stack.database.all_in_root(stack.root).unwrap().is_empty());
    assert_eq!(
        stack.database.pending_local_mutations(32).unwrap().len(),
        before.len()
    );
    stack
        .database
        .with_reader(|connection| {
            for table in [
                "sync_resources",
                "sync_resource_refs",
                "sync_resource_manifests",
            ] {
                let count: i64 = connection
                    .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                        row.get(0)
                    })
                    .map_err(storage)?;
                assert_eq!(count, 0, "{table} rolls back with the song");
            }
            Ok(())
        })
        .unwrap();
    assert!(stack
        .library_dir
        .join("media/Artist/Artist - Recover.flac")
        .is_file());
    deps.uow = stack.database.clone();
    crate::application::recover::RecoverOperations::new(&deps)
        .run(stack.root)
        .unwrap();
    let songs = stack.database.all_in_root(stack.root).unwrap();
    assert_eq!(songs.len(), 1);
    assert_eq!(songs[0].id(), *reserved, "recovery reuses the saved identity");
    assert_eq!(durable.read_operation(stack.root, operation.as_uuid()).unwrap().unwrap(), saved,
        "recovery preserves the frozen directory operation");
    let queue = stack.database.pending_local_mutations(32).unwrap();
    let mutation = queue
        .iter()
        .find(|entry| entry.object == ObjectKey::Song(songs[0].id()))
        .unwrap();
    assert_eq!(mutation.resource_refs.len(), 1);
    assert_eq!(
        mutation.resource_refs[0].digest.to_hex(),
        blake3::hash(audio).to_hex().to_string()
    );
}

#[test]
fn sync_import_transaction_refuses_resource_that_differs_from_song_scan_facts() {
    use crate::domain::sync::{ResourceDigest, SharedResourceKind, SharedResourceManifest};
    let stack = ScanStack::new();
    bind_empty_import_host(&stack);
    let mut song = song(stack.root, "media/Track.flac", "Track", "Artist");
    song.apply_scan_facts(
        "ab".repeat(32),
        9,
        1,
        crate::domain::media::AudioFormat::Flac,
    );
    let resource = SharedResourceManifest::new(
        SharedResourceKind::Audio,
        ResourceDigest::from_bytes([0xcd; 32]),
        9,
        "audio/flac".to_owned(),
        "media/Track.flac",
    )
    .unwrap();
    assert!(stack
        .database
        .with_tx(Box::new(move |tx| {
            tx.set_song_file_resources(&song, &[resource])?;
            tx.upsert_song(&song)
        }))
        .is_err());
    assert!(stack.database.all_in_root(stack.root).unwrap().is_empty());
    assert!(stack
        .database
        .local_sync_resource_manifests(stack.root)
        .unwrap()
        .is_empty());
}

#[test]
fn sync_import_scan_after_baseline_queues_playable_audio_declaration() {
    use crate::application::ports::LocalSyncQueuePort;
    use crate::domain::sync::{ObjectKey, SharedResourceKind};
    let stack = ScanStack::new();
    bind_empty_import_host(&stack);
    let audio = b"external drop into media";
    stack.write("dropped.flac", audio);
    seed_import_source(&stack, "media/dropped.flac", audio, "Artist", "Dropped");
    let deps = stack.deps();
    StartScan::new(&deps, &ScanSupervisor::new())
        .run(stack.root)
        .unwrap();
    let songs = stack.database.all_in_root(stack.root).unwrap();
    assert_eq!(songs.len(), 1);
    let queue = stack.database.pending_local_mutations(32).unwrap();
    let mutation = queue
        .iter()
        .find(|entry| entry.object == ObjectKey::Song(songs[0].id()))
        .unwrap();
    assert_eq!(mutation.resource_refs.len(), 1);
    let resource = &mutation.resource_refs[0];
    assert_eq!(resource.kind, SharedResourceKind::Audio);
    assert_eq!(
        resource.digest.to_hex(),
        blake3::hash(audio).to_hex().to_string()
    );
    assert_eq!(resource.length, u64::try_from(audio.len()).unwrap());
    assert_eq!(resource.relative_path.as_str(), "media/dropped.flac");
}
