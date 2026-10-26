mod artist_cover;
mod favorite;
mod import;
mod ordered_fake;
mod playback;
mod playlist_cover;
mod playlist_create;
mod playlist_delete;
mod playlist_members;
mod playlist_remove;
mod playlist_rename;
use super::*;
use crate::application::playlist::durable::DurableLibraryEdits;
use crate::application::ports::{ControlPlanePort, RuntimeStateStore};
use crate::application::root_switch::{ActivateLibrary, Blockers, ROOT_EPOCH_KEY};
use crate::domain::library::{LibraryId, LibraryManifest, PortableRecord};
use crate::infrastructure::filesystem::{RootControlPlane, RootRegistry};

fn configured_roots(stack: &ScanStack) -> (LibraryRootId, Arc<RootControlPlane>, ScanDeps) {
    let candidate = LibraryRootId::new();
    let candidate_dir = stack.library_dir.with_file_name("candidate");
    std::fs::create_dir(&candidate_dir).unwrap();
    LibraryRepository::upsert(
        stack.database.as_ref(),
        &LibraryRoot::new(candidate, candidate_dir.clone(), false, true),
    )
    .unwrap();
    stack
        .database
        .with_tx(Box::new(|tx| tx.set_runtime_state(ROOT_EPOCH_KEY, "7")))
        .unwrap();
    let registry = RootRegistry::new();
    registry.register(stack.root, &stack.library_dir);
    registry.register(candidate, &candidate_dir);
    let control = Arc::new(RootControlPlane::new(registry));
    for root in [stack.root, candidate] {
        control
            .write_manifest(
                root,
                &LibraryManifest::new(LibraryId::from_uuid(uuid::Uuid::new_v4()), "test"),
            )
            .unwrap();
    }
    stack.database.configure_library_control(control.clone());
    let deps = ScanDeps {
        control: control.clone(),
        ..stack.deps()
    };
    (candidate, control, deps)
}

/// A wiped projection: an empty database that only knows the library root and
/// must rebuild everything else from the directory replay alone.
fn fresh_database(
    stack: &ScanStack,
) -> (
    tempfile::TempDir,
    crate::infrastructure::sqlite::SqliteDatabase,
) {
    let directory = tempfile::tempdir().unwrap();
    let rebuilt =
        crate::infrastructure::sqlite::SqliteDatabase::open(directory.path().join("fresh.sqlite"))
            .unwrap();
    LibraryRepository::upsert(
        &rebuilt,
        &LibraryRoot::new(stack.root, stack.library_dir.clone(), true, true),
    )
    .unwrap();
    (directory, rebuilt)
}

#[test]
fn library_durability_later_edit_catches_up_failed_earlier_playlist() {
    let stack = ScanStack::new();
    let (_, control, deps) = configured_roots(&stack);
    let edits = DurableLibraryEdits {
        control: deps.control.as_ref(),
        songs: deps.songs.as_ref(),
        playlists: deps.playlists.as_ref(),
        unit: deps.uow.as_ref(),
        device: deps.device_id.current_device_id(),
    };
    stack
        .database
        .writer
        .run(|connection| {
            connection.execute_batch(
            "CREATE TRIGGER fail_first_playlist BEFORE INSERT ON library_projection_watermarks
             BEGIN SELECT RAISE(ABORT, 'injected playlist projection failure'); END;",
        ).map_err(storage)
        })
        .unwrap();
    let error = edits
        .create_playlist(stack.root, "first saved playlist")
        .unwrap_err();
    assert!(matches!(
        error,
        crate::error::Error::LibrarySavedPending { .. }
    ));
    let first = control
        .durability()
        .unwrap()
        .recover(stack.root)
        .unwrap()
        .commits[0]
        .batch
        .records[0]
        .record()
        .object_uuid();
    assert!(deps
        .playlists
        .name(crate::domain::ids::PlaylistId::from_uuid(first))
        .unwrap()
        .is_none());
    stack
        .database
        .writer
        .run(|connection| {
            connection
                .execute_batch("DROP TRIGGER fail_first_playlist")
                .map_err(storage)
        })
        .unwrap();
    let second = edits
        .create_playlist(stack.root, "second saved playlist")
        .unwrap();
    assert_eq!(
        deps.playlists.name(second).unwrap().as_deref(),
        Some("second saved playlist")
    );
    assert_eq!(
        deps.playlists
            .name(crate::domain::ids::PlaylistId::from_uuid(first))
            .unwrap()
            .as_deref(),
        Some("first saved playlist"),
        "a successful later edit must project the whole missing directory prefix",
    );
}

#[test]
fn library_durability_local_playlist_preserves_allocations_without_send_authority() {
    use crate::domain::library::durability::{FrozenMutation, StateNamespace, StateTarget};
    let stack = ScanStack::new();
    let (_, control, deps) = configured_roots(&stack);
    let edits = DurableLibraryEdits {
        control: deps.control.as_ref(),
        songs: deps.songs.as_ref(),
        playlists: deps.playlists.as_ref(),
        unit: deps.uow.as_ref(),
        device: deps.device_id.current_device_id(),
    };
    let first = edits
        .create_playlist(stack.root, "first allocation")
        .unwrap();
    let second = edits
        .create_playlist(stack.root, "second allocation")
        .unwrap();
    let replay = control.durability().unwrap().recover(stack.root).unwrap();
    let frozen: Vec<FrozenMutation> = replay
        .commits
        .iter()
        .flat_map(|commit| &commit.batch.state)
        .filter(|state| {
            matches!(
                state.key.as_ref().map(|key| &key.target),
                Some(StateTarget::Mutation { .. })
            )
        })
        .map(|state| {
            assert!(matches!(
                state.key.as_ref().unwrap().namespace,
                StateNamespace::LocalUnbound { .. }
            ));
            serde_json::from_value(state.payload["frozen"].clone()).unwrap()
        })
        .collect();
    assert_eq!(frozen.len(), 2);
    assert_eq!(frozen[0].candidate.origin_seq, 1);
    assert_eq!(frozen[1].candidate.origin_seq, 2);
    assert_eq!(
        frozen[1].candidate.previous_mutation_id,
        Some(frozen[0].candidate.mutation_id)
    );
    assert_eq!(
        frozen[0].candidate.object,
        crate::domain::sync::ObjectKey::Playlist(first)
    );
    assert_eq!(
        frozen[1].candidate.object,
        crate::domain::sync::ObjectKey::Playlist(second)
    );
    for commit in &replay.commits {
        let record = commit.batch.records[0].record();
        let PortableRecord::Playlist(record) = record else {
            panic!("playlist expected")
        };
        let mutation = &frozen[usize::try_from(commit.library_commit_seq - 1).unwrap()];
        assert_eq!(
            record.created_at,
            mutation.candidate.payload["created_at"].as_u64()
        );
        let projected_created_at: u64 = stack
            .database
            .with_reader(|connection| {
                connection
                    .query_row(
                        "SELECT created_at FROM playlists WHERE uuid=?1",
                        [record.playlist_uuid.to_string()],
                        |row| row.get(0),
                    )
                    .map_err(storage)
            })
            .unwrap();
        assert_eq!(record.created_at, Some(projected_created_at));
    }
    stack
        .database
        .writer
        .run(move |connection| {
            let allocated: u64 = connection
                .query_row(
                    "SELECT origin_seq FROM sync_local_origin_cursors WHERE local_root_uuid=?1",
                    [stack.root.to_string()],
                    |row| row.get(0),
                )
                .map_err(storage)?;
            assert_eq!(allocated, 2);
            let queued: u64 = connection
                .query_row("SELECT COUNT(*) FROM sync_mutations", [], |row| row.get(0))
                .map_err(storage)?;
            assert_eq!(queued, 0, "unbound recovery must not authorize sending");
            Ok(())
        })
        .unwrap();
}

#[test]
fn library_durability_complete_local_allocation_is_not_an_unresolved_send_request() {
    use crate::application::ports::LibraryProjectionPort;
    let stack = ScanStack::new();
    let (_, control, deps) = configured_roots(&stack);
    DurableLibraryEdits {
        control: deps.control.as_ref(),
        songs: deps.songs.as_ref(),
        playlists: deps.playlists.as_ref(),
        unit: deps.uow.as_ref(),
        device: deps.device_id.current_device_id(),
    }
    .create_playlist(stack.root, "local evidence retained")
    .unwrap();
    let evidence = stack.database.read_recovery_records(stack.root).unwrap();
    assert_eq!(evidence.state.len(), 2);
    for state in &evidence.state {
        state.validate().unwrap();
    }
    let status = stack.database.projection_status(stack.root).unwrap();
    assert!(status.unresolved.is_empty(), "{status:?}");
    let replay = control.durability().unwrap().recover(stack.root).unwrap();
    stack
        .database
        .project_library_commits(stack.root, &replay)
        .unwrap();
    assert_eq!(
        stack
            .database
            .read_recovery_records(stack.root)
            .unwrap()
            .state,
        evidence.state,
        "resolved local evidence must remain available for recovery"
    );
    assert!(stack
        .database
        .projection_status(stack.root)
        .unwrap()
        .unresolved
        .is_empty());
    let queued: u64 = stack
        .database
        .with_reader(|connection| {
            connection
                .query_row(
                    "SELECT (SELECT COUNT(*) FROM sync_mutations) + (SELECT COUNT(*) FROM sync_outbox)",
                    [],
                    |row| row.get(0),
                )
                .map_err(storage)
        })
        .unwrap();
    assert_eq!(queued, 0, "local evidence must not grant send authority");
}

#[test]
fn library_durability_incomplete_or_foreign_local_allocation_stays_unresolved() {
    use crate::application::ports::LibraryProjectionPort;
    for missing in ["pending", "pending_disposition", "local_author"] {
        let stack = ScanStack::new();
        let (_, control, deps) = configured_roots(&stack);
        DurableLibraryEdits {
            control: deps.control.as_ref(),
            songs: deps.songs.as_ref(),
            playlists: deps.playlists.as_ref(),
            unit: deps.uow.as_ref(),
            device: deps.device_id.current_device_id(),
        }
        .create_playlist(stack.root, "incomplete local evidence")
        .unwrap();
        stack.database.writer.run(move |connection| {
            if missing == "local_author" {
                connection.execute("UPDATE device_state SET value=?1 WHERE key='device_id'", [uuid::Uuid::new_v4().to_string()]).map_err(storage)?;
            } else {
                connection.execute("DELETE FROM library_recovery_records WHERE row_kind='state' AND json_extract(envelope_json,'$.kind')=?1", [missing]).map_err(storage)?;
            }
            Ok(())
        }).unwrap();
        let before = stack.database.read_recovery_records(stack.root).unwrap();
        let replay = control.durability().unwrap().recover(stack.root).unwrap();
        stack
            .database
            .project_library_commits(stack.root, &replay)
            .unwrap();
        let status = stack.database.projection_status(stack.root).unwrap();
        assert!(!status.unresolved.is_empty(), "{missing}");
        assert_eq!(
            stack.database.read_recovery_records(stack.root).unwrap(),
            before
        );
        let queued: u64 = stack
            .database
            .with_reader(|connection| {
                connection
                    .query_row("SELECT COUNT(*) FROM sync_mutations", [], |row| row.get(0))
                    .map_err(storage)
            })
            .unwrap();
        assert_eq!(queued, 0, "{missing}");
    }
}

#[test]
fn library_durability_playlist_accepts_a_cloned_directory_authority() {
    let stack = ScanStack::new();
    let (_, control, deps) = configured_roots(&stack);
    let cloned = control.as_ref().clone();
    let playlist = DurableLibraryEdits {
        control: &cloned,
        songs: deps.songs.as_ref(),
        playlists: deps.playlists.as_ref(),
        unit: deps.uow.as_ref(),
        device: deps.device_id.current_device_id(),
    }
    .create_playlist(stack.root, "cloned authority")
    .unwrap();
    assert_eq!(
        deps.playlists.name(playlist).unwrap().as_deref(),
        Some("cloned authority")
    );
    let replay = control.durability().unwrap().recover(stack.root).unwrap();
    assert_eq!(replay.commits.len(), 1);
    assert_eq!(
        replay.commits[0].batch.records[0].record().object_uuid(),
        playlist.as_uuid()
    );
}

#[test]
fn library_durability_bound_edits_reserve_distinct_sequences_before_sync() {
    use crate::domain::library::durability::{FrozenMutation, StateTarget};
    use crate::domain::sync::{AuthorityId, Epoch, LibraryId as SyncId, SyncBinding};
    let stack = ScanStack::new();
    let (_, control, deps) = configured_roots(&stack);
    stack
        .database
        .bind_sync_library(
            SyncBinding {
                library_id: SyncId::new(),
                authority_id: AuthorityId::new(),
                epoch: Epoch::INITIAL,
            },
            true,
            stack.root,
        )
        .unwrap();
    let edits = DurableLibraryEdits {
        control: deps.control.as_ref(),
        songs: deps.songs.as_ref(),
        playlists: deps.playlists.as_ref(),
        unit: deps.uow.as_ref(),
        device: deps.device_id.current_device_id(),
    };
    edits
        .create_playlist(stack.root, "first bound allocation")
        .unwrap();
    edits
        .create_playlist(stack.root, "second bound allocation")
        .unwrap();
    let replay = control.durability().unwrap().recover(stack.root).unwrap();
    let mutations: Vec<FrozenMutation> = replay
        .states()
        .filter(|state| {
            matches!(
                state.key.as_ref().map(|key| &key.target),
                Some(StateTarget::Mutation { .. })
            )
        })
        .map(|state| serde_json::from_value(state.payload["frozen"].clone()).unwrap())
        .collect();
    assert_eq!(mutations.len(), 2);
    assert_eq!(mutations[0].candidate.origin_seq, 1);
    assert_eq!(mutations[1].candidate.origin_seq, 2);
    assert_eq!(
        mutations[1].candidate.previous_mutation_id,
        Some(mutations[0].candidate.mutation_id)
    );
    assert!(
        crate::application::ports::LocalSyncQueuePort::pending_local_mutations(
            stack.database.as_ref(),
            10
        )
        .unwrap()
        .is_empty()
    );
}

#[test]
fn library_durability_playlist_projection_preserves_planned_clock() {
    let stack = ScanStack::new();
    let (_, control, deps) = configured_roots(&stack);
    let playlist = DurableLibraryEdits {
        control: deps.control.as_ref(),
        songs: deps.songs.as_ref(),
        playlists: deps.playlists.as_ref(),
        unit: deps.uow.as_ref(),
        device: deps.device_id.current_device_id(),
    }
    .create_playlist(stack.root, "clock preserved")
    .unwrap();
    let replay = control.durability().unwrap().recover(stack.root).unwrap();
    let PortableRecord::Playlist(record) = replay.commits[0].batch.records[0].record() else {
        panic!("playlist expected")
    };
    assert!(record.hlc.wall_secs > 0);
    let projected: (u64, u32) = stack
        .database
        .with_reader(|connection| {
            connection
                .query_row(
                    "SELECT hlc_wall_secs,hlc_counter FROM playlists WHERE uuid=?1",
                    [playlist.to_string()],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .map_err(storage)
        })
        .unwrap();
    assert_eq!(projected, (record.hlc.wall_secs, record.hlc.counter));
}

#[test]
fn activation_sql_failure_rolls_back_root_and_epoch_then_reopens_real_writes() {
    let stack = ScanStack::new();
    let (candidate, control, deps) = configured_roots(&stack);
    // The root flip runs first. Fail the subsequent epoch SQL statement so
    // the real SQLite transaction must roll both changes back together.
    stack
        .database
        .writer
        .run(|connection| {
            connection
                .execute_batch(
                    "CREATE TRIGGER fail_activation_epoch BEFORE INSERT ON runtime_state
                     BEGIN SELECT RAISE(ABORT, 'injected activation epoch failure'); END;",
                )
                .map_err(storage)
        })
        .unwrap();
    let supervisor = ScanSupervisor::new();
    let blockers = Blockers::new();
    let activate = ActivateLibrary::new(
        &deps,
        stack.database.as_ref(),
        &supervisor,
        stack.database.as_ref(),
        &blockers,
    );
    let error = activate.activate(candidate).unwrap_err();
    assert!(error
        .to_string()
        .contains("injected activation epoch failure"));
    assert_eq!(
        stack.database.active_root().unwrap().unwrap().id(),
        stack.root
    );
    assert!(
        !LibraryRepository::by_id(stack.database.as_ref(), candidate)
            .unwrap()
            .unwrap()
            .is_active()
    );
    assert_eq!(
        stack.database.load(ROOT_EPOCH_KEY).unwrap().as_deref(),
        Some("7")
    );
    assert!(!deps.activity.is_closed());
    stack
        .database
        .writer
        .run(|connection| {
            connection
                .execute_batch("DROP TRIGGER fail_activation_epoch")
                .map_err(storage)
        })
        .unwrap();
    let playlist = {
        let _lease = deps.activity.enter().expect("failure reopened admission");
        DurableLibraryEdits {
            control: deps.control.as_ref(),
            songs: deps.songs.as_ref(),
            playlists: deps.playlists.as_ref(),
            unit: deps.uow.as_ref(),
            device: deps.device_id.current_device_id(),
        }
        .create_playlist(stack.root, "saved after failed activation")
        .unwrap()
    };
    let replay = control.durability().unwrap().recover(stack.root).unwrap();
    assert!(replay
        .commits
        .iter()
        .flat_map(|commit| &commit.batch.records)
        .any(|record| record.record().object_uuid() == playlist.as_uuid()));
    assert!(control
        .durability()
        .unwrap()
        .recover(candidate)
        .unwrap()
        .commits
        .is_empty());
    let outcome = activate
        .activate(candidate)
        .expect("activation remains retryable");
    assert_eq!(outcome.epoch.as_u64(), 8);
    assert_eq!(
        stack.database.active_root().unwrap().unwrap().id(),
        candidate
    );
    assert_eq!(
        stack.database.load(ROOT_EPOCH_KEY).unwrap().as_deref(),
        Some("8")
    );
    assert!(deps.activity.enter().is_ok());
}
