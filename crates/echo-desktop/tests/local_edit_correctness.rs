//! Fast production-path checks for local import, favorite and playlist edits.
//!
//! A disposable library and miniature audio fixture keep these checks suitable
//! for ordinary workspace tests. Release latency belongs to a separate suite.

use echo_core::application::ports::{ControlPlanePort, LibraryRepository, PlaylistRepository};
use echo_core::application::root_switch::Blockers;
use echo_core::application::scan::ScanSupervisor;
use echo_core::domain::catalog::{SongSort, SongSortField, SortDirection};
use echo_core::domain::entities::LibraryRoot;
use echo_core::domain::ids::{LibraryRootId, PlaylistId, SongId};
use echo_core::domain::library::{LibraryId, LibraryManifest, PortableRecord, RecordKind};
use echo_core::infrastructure::sqlite::SqliteDatabase;
use echo_desktop::ipc::dto::ImportResultDto;
use echo_desktop::platform::dialogs::TestDialogs;
use echo_desktop::platform::trash::DesktopTrash;
use echo_desktop::runtime::app::assemble;
use echo_desktop::runtime::services::AppServices;
use echo_desktop::runtime::StartupSupervisor;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

struct Library {
    app: AppServices,
    database: Arc<SqliteDatabase>,
    control: Arc<dyn ControlPlanePort>,
    root: LibraryRootId,
    directory: tempfile::TempDir,
}

impl Library {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let root_path = directory.path().join("library");
        std::fs::create_dir(&root_path).unwrap();
        let runtime = assemble(
            &directory.path().join("echo.sqlite"),
            &directory.path().join("covers"),
        )
        .unwrap();
        let root = LibraryRootId::new();
        runtime.registry.register(root, &root_path);
        LibraryRepository::upsert(
            runtime.database.as_ref(),
            &LibraryRoot::new(root, root_path, true, true),
        )
        .unwrap();
        runtime
            .deps
            .control
            .write_manifest(
                root,
                &LibraryManifest::new(LibraryId::from_uuid(uuid::Uuid::new_v4()), "fixture"),
            )
            .unwrap();
        runtime.deps.fs.establish_write_capability(root).unwrap();

        let startup = Arc::new(StartupSupervisor::new());
        let supervisor = ScanSupervisor::new();
        startup
            .run_recovery(&runtime.deps, &supervisor, &DesktopTrash::default())
            .unwrap();
        startup.on_ready();
        let app = AppServices::with_runtime(
            runtime.deps.clone(),
            supervisor,
            startup,
            Arc::new(TestDialogs::cancelling()),
            runtime.registry,
            runtime.database.clone(),
            Blockers::new(),
        );
        Self {
            app,
            database: runtime.database,
            control: runtime.deps.control.clone(),
            root,
            directory,
        }
    }

    fn import(&self) -> SongId {
        let source =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/audio/tone-short.flac");
        let input = self.directory.path().join("import.flac");
        std::fs::copy(source, &input).unwrap();
        let outcome = self.app.import_single_path(&input, "import.flac").unwrap();
        let ImportResultDto::Imported { song_id, .. } = outcome else {
            panic!("a valid miniature FLAC should import: {outcome:?}");
        };
        song_id.parse().unwrap()
    }

    fn flush(&self) {
        self.database
            .flush_library_projection(self.root, Duration::from_secs(5))
            .unwrap();
    }
}

#[test]
fn import_is_visible_without_rescan() {
    let library = Library::new();
    let load_catalog = || {
        library.app.all_songs(
            SongSort {
                field: SongSortField::AddedAt,
                direction: SortDirection::Desc,
            },
            None,
            10,
        )
    };
    assert_eq!(load_catalog().unwrap().total_count, 0);
    let song = library.import();
    // Unlike the playlist checks, do not wait for projection or run a scan:
    // successful import must already be visible through the desktop read path.
    let catalog = load_catalog().unwrap();
    assert_eq!(catalog.total_count, 1);
    assert_eq!(catalog.items[0].id, song.to_string());
}

#[test]
fn favorite_success_has_a_durable_after_image_and_visible_new_value() {
    let library = Library::new();
    let song = library.import();
    assert!(!library.app.song_detail(song).unwrap().favorite);
    assert!(library.app.set_favorite(song, true).unwrap().favorite);
    let record = library
        .control
        .read_record(library.root, RecordKind::Favorite, &song.to_string())
        .unwrap();
    assert!(matches!(record, Some(PortableRecord::Favorite(record))
        if record.song_uuid == song.as_uuid() && record.is_favorite));
    assert!(library.app.song_detail(song).unwrap().favorite);
}

#[test]
fn playlist_membership_is_durable_and_projected() {
    let library = Library::new();
    let song = library.import();
    let playlist = library
        .app
        .create_playlist(library.root, "Small correctness fixture")
        .unwrap()
        .parse::<PlaylistId>()
        .unwrap();
    let record = library
        .control
        .read_record(library.root, RecordKind::Playlist, &playlist.to_string())
        .unwrap();
    assert!(matches!(record, Some(PortableRecord::Playlist(record))
        if record.playlist_uuid == playlist.as_uuid() && record.display_name == "Small correctness fixture"));
    library.flush();
    assert!(
        PlaylistRepository::by_id(library.database.as_ref(), playlist)
            .unwrap()
            .is_some()
    );

    library.app.add_to_playlists(song, &[playlist]).unwrap();
    let durable = library.control.durability().unwrap();
    let replay = durable.recover(library.root).unwrap();
    let addition = replay
        .commits
        .last()
        .expect("the addition was durably committed");
    let member = addition
        .batch
        .records
        .iter()
        .find_map(|envelope| match envelope.record() {
            PortableRecord::PlaylistItem(record)
                if record.playlist_uuid == playlist.as_uuid()
                    && record.song_uuid == song.as_uuid() =>
            {
                Some(record.item_uuid)
            }
            _ => None,
        })
        .expect("the saved addition has a membership identity");
    library.flush();
    let members = PlaylistRepository::members(library.database.as_ref(), playlist).unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].song(), song);
    assert_eq!(members[0].id().as_uuid(), member);

    library.app.remove_playlist_song(playlist, song).unwrap();
    let replay = durable.recover(library.root).unwrap();
    let removal = replay
        .commits
        .last()
        .expect("the removal was durably committed");
    assert!(removal
        .batch
        .records
        .iter()
        .any(|envelope| matches!(envelope.record(),
        PortableRecord::Tombstone(record) if record.deleted_kind == RecordKind::PlaylistItem
            && record.object_uuid == member)));
    library.flush();
    assert!(
        PlaylistRepository::members(library.database.as_ref(), playlist)
            .unwrap()
            .is_empty()
    );
}
