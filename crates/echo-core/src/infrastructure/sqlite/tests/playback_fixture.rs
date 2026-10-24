//! Give low-level repository fixtures an explicit, synthetic directory baseline.
use super::*;
use crate::application::ports::ControlPlanePort;
use crate::domain::library::durability::{ChangeOrigin, LibraryBatch};
use crate::domain::library::{LibraryId, LibraryManifest};
use crate::infrastructure::filesystem::{RootControlPlane, RootRegistry};

pub fn directory(database: &SqliteDatabase, root: LibraryRootId) -> Arc<RootControlPlane> {
    let path = LibraryRepository::by_id(database, root)
        .unwrap()
        .unwrap()
        .absolute_path()
        .to_path_buf();
    std::fs::create_dir_all(&path).unwrap();
    let registry = RootRegistry::new();
    registry.register(root, &path);
    let control = Arc::new(RootControlPlane::new(registry));
    let library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    control
        .write_manifest(
            root,
            &LibraryManifest::new(library, "synthetic playback fixture"),
        )
        .unwrap();
    let records = database
        .with_reader(|c| super::super::durability_backfill::export_library_records(c, root))
        .unwrap();
    control
        .durability()
        .unwrap()
        .commit(
            root,
            &LibraryBatch::new(uuid::Uuid::new_v4(), library, ChangeOrigin::Scan, records),
        )
        .unwrap();
    database.configure_library_control(control.clone());
    control
}

pub fn record(
    database: &SqliteDatabase,
    control: &RootControlPlane,
    session: PlaybackSessionId,
    song: SongId,
) -> bool {
    crate::application::playback_recording::record_qualified_playback(
        control,
        database,
        database,
        database.current_device_id(),
        session,
        song,
    )
    .unwrap()
}

pub fn mutations(
    control: &RootControlPlane,
    root: LibraryRootId,
) -> Vec<crate::domain::sync::MutationCandidate> {
    control
        .durability()
        .unwrap()
        .recover(root)
        .unwrap()
        .states()
        .filter(|state| state.kind == crate::domain::library::durability::StateKind::Pending)
        .map(|state| {
            serde_json::from_value::<crate::domain::library::durability::FrozenMutation>(
                state.payload["frozen"].clone(),
            )
            .unwrap()
            .candidate
        })
        .collect()
}
