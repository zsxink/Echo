use super::*;
use crate::application::playlist::durable::DurableLibraryEdits;
use crate::application::ports::{ControlPlanePort, RuntimeStateStore};
use crate::application::root_switch::{ActivateLibrary, Blockers, ROOT_EPOCH_KEY};
use crate::domain::library::{LibraryId, LibraryManifest};
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
