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

fn held(database: &SqliteDatabase, root: LibraryRootId) -> Vec<Vec<u8>> {
    database.with_reader(|connection| {
        let mut statement = connection.prepare("SELECT DISTINCT content_hash FROM sync_resource_holds WHERE local_root_uuid=?1 ORDER BY content_hash").map_err(storage)?;
        let values = statement.query_map([root.to_string()], |row| row.get(0)).map_err(storage)?;
        values.collect::<Result<_, _>>().map_err(storage)
    }).unwrap()
}

#[test]
#[allow(clippy::too_many_lines)]
fn resource_retention_directory_replay_survives_compaction_and_context_cleanup_without_echo() {
    use crate::application::ports::{
        ControlPlanePort, LibraryDurabilityPort, LibraryProjectionPort,
    };
    use crate::domain::library::durability::{
        ChangeOrigin, DurableResource, DurableStateChange, LibraryBatch, StateKind,
    };
    use crate::domain::library::{LibraryId, LibraryManifest};
    use crate::infrastructure::filesystem::{RootControlPlane, RootRegistry};

    let (directory, database, root) = database();
    let path = directory.path().join("library");
    std::fs::create_dir(&path).unwrap();
    let registry = RootRegistry::new();
    registry.register(root, &path);
    let control = RootControlPlane::new(registry);
    let library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    control
        .write_manifest(root, &LibraryManifest::new(library, "retention"))
        .unwrap();
    let mut resources = Vec::new();
    for bytes in [
        b"frozen candidate original".as_slice(),
        b"losing conflict original".as_slice(),
    ] {
        let hash = control.write_asset(root, bytes).unwrap();
        resources.push(DurableResource {
            relative_path: format!("echo/assets/{hash}"),
            content_hash: hash,
            byte_length: bytes.len() as u64,
        });
    }
    resources.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    let evidence = vec![
        DurableStateChange::new(
            StateKind::Pending,
            uuid::Uuid::new_v4(),
            serde_json::json!({"schema":"retained.candidate.v1","terminal":"rejected"}),
        ),
        DurableStateChange::new(
            StateKind::Conflict,
            uuid::Uuid::new_v4(),
            serde_json::json!({"schema":"retained.conflict.v1","original":"losing value"}),
        ),
    ];
    for index in 0..34 {
        let mut batch = LibraryBatch::new(
            uuid::Uuid::new_v4(),
            library,
            ChangeOrigin::Recovery,
            vec![],
        );
        if index == 0 {
            batch.resources = resources.clone();
            batch.state = evidence.clone();
        }
        control.commit(root, &batch).unwrap();
    }
    let replay = control.recover(root).unwrap();
    assert!(
        replay.checkpoint.is_some(),
        "the test must recover compacted evidence"
    );
    database.project_library_commits(root, &replay).unwrap();
    let retained = database.read_recovery_records(root).unwrap();
    assert_eq!(retained.resources, resources);
    assert!(evidence.iter().all(|state| retained.state.contains(state)));
    database.rebuild_resource_holds(root).unwrap();
    let expected = held(&database, root);
    assert_eq!(expected.len(), 2);

    // Switching away clears the live protocol context, not directory evidence.
    let other = LibraryRootId::new();
    LibraryRepository::upsert(
        &database,
        &LibraryRoot::new(other, directory.path().join("other"), false, true),
    )
    .unwrap();
    database.select_sync_context(other).unwrap();
    database.rebuild_resource_holds(root).unwrap();
    assert_eq!(held(&database, root), expected);
    assert!(held(&database, other).is_empty());
    database.project_library_commits(root, &replay).unwrap();
    database.rebuild_resource_holds(root).unwrap();
    assert_eq!(held(&database, root), expected);
    database.with_reader(|connection| {
        let counts: (i64, i64) = connection.query_row("SELECT (SELECT COUNT(*) FROM sync_mutations),(SELECT COUNT(*) FROM sync_outbox)", [], |row| Ok((row.get(0)?, row.get(1)?))).map_err(storage)?;
        assert_eq!(counts, (0, 0), "replay and hold reconstruction must not mint edits");
        Ok(())
    }).unwrap();

    database.writer.run(move |connection| {
        connection.execute("UPDATE library_recovery_records SET envelope_json='{}' WHERE library_root_uuid=?1 AND row_kind='resource'", [root.to_string()]).map_err(storage)?;
        Ok(())
    }).unwrap();
    assert!(database.rebuild_resource_holds(root).is_err());
    assert_eq!(
        held(&database, root),
        expected,
        "failed reconstruction preserves the old hold set"
    );
}

#[test]
fn resource_retention_epoch_rotation_keeps_terminal_candidates_and_confirmed_originals() {
    use crate::application::ports::{
        HostCommitLedgerPort, HostEpochRecoveryPort, LocalSyncQueuePort,
    };
    use crate::domain::sync::{AuthorityId, Epoch, LibraryId, SyncBinding, SyncSessionIntent};

    let (directory, database, root) = database();
    let binding = SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    database.bind_sync_library(binding, true, root).unwrap();
    let mut expected = Vec::new();
    for (path, bytes) in [
        ("media/confirmed.flac", b"confirmed original".as_slice()),
        (
            "media/terminal.flac",
            b"terminal candidate original".as_slice(),
        ),
    ] {
        let mut song = song(root, path, path, "Artist");
        let mut resource = declaration(bytes);
        resource.relative_path = crate::domain::library::LibraryRelativePath::new(path).unwrap();
        song.apply_scan_facts(
            resource.digest.to_hex(),
            resource.length,
            1,
            crate::domain::media::AudioFormat::Flac,
        );
        database
            .set_local_sync_resources(ObjectKey::Song(song.id()), vec![resource.clone()])
            .unwrap();
        SongRepository::upsert(&database, &song).unwrap();
        expected.push(resource.digest.as_bytes().to_vec());
    }
    let candidates = database.pending_local_mutations(20).unwrap();
    let staged = database
        .stage_confirmation(binding, &candidates[0])
        .unwrap();
    database.mark_published(staged.checkpoint.unwrap()).unwrap();
    let confirmed = candidates[0].mutation_id.as_uuid().to_string();
    database
        .writer
        .run(move |connection| {
            connection
                .execute(
                    "DELETE FROM sync_mutations WHERE mutation_id=?1",
                    [confirmed],
                )
                .map_err(storage)?;
            connection
                .execute("UPDATE sync_mutations SET status='rejected'", [])
                .map_err(storage)?;
            connection
                .execute(
                    "DELETE FROM songs WHERE library_root_uuid=?1",
                    [root.to_string()],
                )
                .map_err(storage)?;
            Ok(())
        })
        .unwrap();
    database
        .set_sync_session_intent(SyncSessionIntent::Stopped)
        .unwrap();
    expected.sort();
    database.rebuild_resource_holds(root).unwrap();
    assert_eq!(
        held(&database, root),
        expected,
        "terminal evidence still owns its original bytes"
    );
    let replacement = SyncBinding {
        authority_id: AuthorityId::new(),
        epoch: Epoch::from_u64(2),
        ..binding
    };
    database
        .rotate_host_epoch(root, binding, replacement)
        .unwrap();
    database.rebuild_resource_holds(root).unwrap();
    assert_eq!(held(&database, root), expected);
    let other = LibraryRootId::new();
    LibraryRepository::upsert(
        &database,
        &LibraryRoot::new(other, directory.path().join("other"), false, true),
    )
    .unwrap();
    database.select_sync_context(other).unwrap();
    database.rebuild_resource_holds(root).unwrap();
    assert_eq!(held(&database, root), expected);
    assert!(held(&database, other).is_empty());
    assert!(database.pending_local_mutations(20).unwrap().is_empty());
}
