//! Controlled directory-commit ordering across file-operation entrypoints.

use crate::application::delete::{DeleteSongs, RestoreDeletedOperation};
use crate::application::playlist::durable::DurableLibraryEdits;
use crate::application::ports::{
    ControlPlanePort, LibraryRepository, OperationJournalRepository, SongRepository,
    SystemTrashPort,
};
use crate::application::recover::RecoverOperations;
use crate::application::scan::{ScanDeps, StartScan};
use crate::application::testing::scan_fixture::ScanFixture;
use crate::application::testing::small_fakes::FakeTrash;
use crate::application::testkit::durability_ports::RefusingAuthority;
use crate::application::trash::FinalizeExpiredDeletes;
use crate::domain::entities::{LibraryRoot, SongAvailability};
use crate::domain::ids::{OperationId, SongId};
use crate::domain::library::durability::LibraryReplay;
use crate::domain::state::OperationState;
use crate::error::Error;
use crate::infrastructure::filesystem::{RootControlPlane, RootRegistry};
use crate::infrastructure::sqlite::SqliteDatabase;
use std::fmt::Write as _;
use std::sync::Arc;

struct Fixture {
    scan: ScanFixture,
    _database_dir: tempfile::TempDir,
    database: Arc<SqliteDatabase>,
    control: Arc<RootControlPlane>,
    authority: Arc<RefusingAuthority>,
    deps: ScanDeps,
}

impl Fixture {
    fn new() -> Self {
        let scan = ScanFixture::new();
        let database_dir = tempfile::tempdir().unwrap();
        let database =
            Arc::new(SqliteDatabase::open(database_dir.path().join("order.sqlite")).unwrap());
        let path = scan.fs.root_path(scan.root).unwrap();
        LibraryRepository::upsert(
            &*database,
            &LibraryRoot::new(scan.root, path.clone(), true, true),
        )
        .unwrap();
        let registry = RootRegistry::new();
        registry.register(scan.root, path);
        let control = Arc::new(RootControlPlane::new(registry));
        let authority = Arc::new(RefusingAuthority::new(control.clone()));
        // Both routes must share the same authority: file use cases commit via
        // ScanDeps; repository edit planning commits via the database binding.
        database.configure_library_control(authority.clone());
        let deps = ScanDeps {
            history: Some(database.clone()),
            roots: database.clone(),
            songs: database.clone(),
            catalog: database.clone(),
            playlists: database.clone(),
            lyrics: database.clone(),
            covers: database.clone(),
            runs: database.clone(),
            journal: database.clone(),
            uow: database.clone(),
            device_id: database.clone(),
            sync: database.clone(),
            control: authority.clone(),
            ..(*scan.deps).clone()
        };
        Self {
            scan,
            _database_dir: database_dir,
            database,
            control,
            authority,
            deps,
        }
    }

    fn seed_song(&self) -> SongId {
        self.scan
            .write_file("order.flac", b"controlled order audio");
        self.scan.set_audio("order.flac", "controlled order", 1000);
        StartScan::new(&self.deps, &self.scan.supervisor)
            .run(self.scan.root)
            .unwrap();
        let id = self.database.all_in_root(self.scan.root).unwrap()[0].id();
        self.database
            .set_availability(id, SongAvailability::Available)
            .unwrap();
        id
    }

    fn replay(&self) -> LibraryReplay {
        self.control
            .durability()
            .unwrap()
            .recover(self.scan.root)
            .unwrap()
    }

    fn fingerprint(&self) -> String {
        self.database
            .with_reader(|c| {
                let mut fingerprint = String::new();
                for table in [
                    "songs",
                    "playlists",
                    "playlist_songs",
                    "recorded_play_sessions",
                    "sync_device_play_counts",
                    "song_overrides",
                    "song_lyrics",
                    "sync_mutations",
                    "sync_outbox",
                    "sync_local_highwater",
                    "sync_local_origin_cursors",
                    "sync_object_versions",
                    "library_recovery_records",
                    "library_applied_operations",
                    "library_projection_watermarks",
                ] {
                    let mut statement = c
                        .prepare(&format!("SELECT * FROM {table}"))
                        .map_err(storage)?;
                    let columns = statement.column_count();
                    let mut rows = statement
                        .query_map([], |row| {
                            (0..columns)
                                .map(|column| row.get_ref(column).map(|value| format!("{value:?}")))
                                .collect::<Result<Vec<_>, _>>()
                        })
                        .map_err(storage)?
                        .collect::<Result<Vec<_>, _>>()
                        .map_err(storage)?;
                    rows.sort();
                    let _ = write!(fingerprint, "{table}:{rows:?};");
                }
                Ok(fingerprint)
            })
            .unwrap()
    }
}

fn storage(error: rusqlite::Error) -> Error {
    Error::Storage {
        what: "controlled-order fingerprint".to_owned(),
        source: Box::new(error),
    }
}

fn assert_refused(error: &Error) {
    assert!(
        matches!(error, Error::Unavailable { resource, .. } if resource == "test directory"),
        "{error:?}"
    );
}

#[test]
fn delete_directory_refusal_does_not_change_user_projection_or_directory_replay() {
    let fixture = Fixture::new();
    let song = fixture.seed_song();
    let before = fixture.fingerprint();
    let replay = fixture.replay();
    fixture.authority.set_refused(true);

    let error = DeleteSongs::new(&fixture.deps)
        .delete(fixture.scan.root, song)
        .expect_err("directory intent rejection must stop the delete");

    assert_refused(&error);
    assert_eq!(
        fixture.fingerprint(),
        before,
        "business tables stay unchanged"
    );
    assert_eq!(
        fixture.replay(),
        replay,
        "rejected intent is absent from replay"
    );
    assert_eq!(
        SongRepository::by_id(&*fixture.database, song)
            .unwrap()
            .unwrap()
            .availability(),
        SongAvailability::Available
    );
}

#[test]
fn undo_directory_refusal_keeps_hidden_song_and_staged_media_for_retry() {
    let fixture = Fixture::new();
    let song = fixture.seed_song();
    let operation = DeleteSongs::new(&fixture.deps)
        .delete(fixture.scan.root, song)
        .unwrap()
        .operation;
    let before = fixture.fingerprint();
    let replay = fixture.replay();
    let items = fixture.database.items(operation).unwrap();
    let staged = items[0].staging_path.clone().unwrap();
    let target = items[0].target_path.clone();
    fixture.authority.set_refused(true);

    let error = RestoreDeletedOperation::new(&fixture.deps)
        .restore(fixture.scan.root, operation)
        .expect_err("undo must stop before moving files when cancel cannot commit");

    assert_refused(&error);
    assert_eq!(
        fixture.fingerprint(),
        before,
        "business tables stay unchanged"
    );
    assert_eq!(
        fixture.replay(),
        replay,
        "the open intent remains authoritative"
    );
    assert!(
        fixture
            .deps
            .fs
            .path_exists(fixture.scan.root, &staged)
            .unwrap()
    );
    assert!(
        !fixture
            .deps
            .fs
            .path_exists(fixture.scan.root, &target)
            .unwrap()
    );
    assert!(
        fixture
            .database
            .items(operation)
            .unwrap()
            .iter()
            .all(|item| item.state == OperationState::HiddenInDatabase)
    );
    assert_eq!(
        SongRepository::by_id(&*fixture.database, song)
            .unwrap()
            .unwrap()
            .availability(),
        SongAvailability::PendingDelete
    );

    fixture.authority.set_refused(false);
    assert_eq!(
        RestoreDeletedOperation::new(&fixture.deps)
            .restore(fixture.scan.root, operation)
            .unwrap(),
        song,
        "retry uses the same persisted operation"
    );
    assert_eq!(
        SongRepository::by_id(&*fixture.database, song)
            .unwrap()
            .unwrap()
            .availability(),
        SongAvailability::Available
    );
}

#[test]
fn recovery_directory_refusal_keeps_pending_delete_until_cancel_can_commit() {
    let fixture = Fixture::new();
    let song = fixture.seed_song();
    let operation = DeleteSongs::new(&fixture.deps)
        .delete(fixture.scan.root, song)
        .unwrap()
        .operation;
    let items = fixture.database.items(operation).unwrap();
    for item in items {
        let staged = item.staging_path.clone().unwrap();
        fixture
            .deps
            .fs
            .restore_from_trash(fixture.scan.root, &staged, &item.target_path)
            .unwrap();
        fixture
            .database
            .upsert_item(
                operation,
                crate::application::ports::OperationItem {
                    state: OperationState::RestorePending,
                    ..item
                },
            )
            .unwrap();
    }
    assert!(
        !fixture
            .database
            .replica_enrollment_pending(fixture.scan.root)
            .unwrap()
    );
    assert_eq!(
        fixture
            .database
            .incomplete_items(fixture.scan.root)
            .unwrap()
            .len(),
        1
    );
    let before = fixture.fingerprint();
    let replay = fixture.replay();
    fixture.authority.set_refused(true);

    let error = RecoverOperations::new(&fixture.deps)
        .run(fixture.scan.root)
        .expect_err("recovery cannot publish the undo terminal state");

    assert_refused(&error);
    assert_eq!(fixture.fingerprint(), before);
    assert_eq!(fixture.replay(), replay);
    assert_eq!(
        SongRepository::by_id(&*fixture.database, song)
            .unwrap()
            .unwrap()
            .availability(),
        SongAvailability::PendingDelete
    );

    fixture.authority.set_refused(false);
    RecoverOperations::new(&fixture.deps)
        .run(fixture.scan.root)
        .unwrap();
    assert_eq!(
        SongRepository::by_id(&*fixture.database, song)
            .unwrap()
            .unwrap()
            .availability(),
        SongAvailability::Available
    );
}

struct RefuseAfterTrash {
    inner: FakeTrash,
    authority: Arc<RefusingAuthority>,
}

impl SystemTrashPort for RefuseAfterTrash {
    fn send_to_trash(
        &self,
        root: crate::domain::ids::LibraryRootId,
        operation: OperationId,
    ) -> Result<(), Error> {
        self.inner.send_to_trash(root, operation)?;
        self.authority.set_refused(true);
        Ok(())
    }
}

#[test]
fn trash_directory_refusal_keeps_applied_receipt_and_retries_same_tombstone() {
    let fixture = Fixture::new();
    let song = fixture.seed_song();
    let operation = DeleteSongs::new(&fixture.deps)
        .delete(fixture.scan.root, song)
        .unwrap()
        .operation;
    fixture.scan.clock.advance_ms(20_000);
    let trash = FakeTrash::new();
    let hooked = RefuseAfterTrash {
        inner: trash.clone(),
        authority: fixture.authority.clone(),
    };

    let error = FinalizeExpiredDeletes::new(&fixture.deps, &hooked)
        .run(fixture.scan.root)
        .expect_err("directory finalization rejection must preserve the catalog row");
    assert_refused(&error);
    assert_eq!(trash.calls(), vec![operation]);
    assert_eq!(
        SongRepository::by_id(&*fixture.database, song)
            .unwrap()
            .unwrap()
            .availability(),
        SongAvailability::PendingDelete
    );
    assert!(
        fixture
            .database
            .items(operation)
            .unwrap()
            .iter()
            .all(|item| item.state == OperationState::TrashApplied)
    );

    fixture.authority.set_refused(false);
    let report = FinalizeExpiredDeletes::new(&fixture.deps, &trash)
        .run(fixture.scan.root)
        .unwrap();
    assert_eq!(report.finalized, vec![operation]);
    assert_eq!(
        trash.calls(),
        vec![operation],
        "retry does not repeat OS trash"
    );
    assert!(
        SongRepository::by_id(&*fixture.database, song)
            .unwrap()
            .is_none()
    );
    assert!(fixture.replay().commits.iter().any(|commit| {
        commit
            .batch
            .records
            .iter()
            .any(|record| record.record().is_tombstone())
    }));
}

/// LD-01a: a favorite whose directory commit is refused is **not saved**. The
/// error must stay a plain refusal (never `LibrarySavedPending`, which the IPC
/// layer reports as "saved, waiting for the view"), no database-only favorite
/// fact may survive, and nothing may enter the directory replay — so the user's
/// optimistic toggle is rolled back to a retryable pre-commit state.
#[test]
fn favorite_directory_refusal_is_not_saved_and_leaves_no_projection_fact() {
    let fixture = Fixture::new();
    let song = fixture.seed_song();
    let edits = DurableLibraryEdits {
        control: fixture.deps.control.as_ref(),
        songs: fixture.deps.songs.as_ref(),
        playlists: fixture.deps.playlists.as_ref(),
        unit: fixture.deps.uow.as_ref(),
        device: fixture.deps.device_id.current_device_id(),
    };
    let before = fixture.fingerprint();
    let replay = fixture.replay();
    fixture.authority.set_refused(true);

    let error = edits
        .set_favorite(fixture.scan.root, song, true)
        .expect_err("a refused directory intent must stop the favorite");

    assert_refused(&error);
    assert!(
        !matches!(error, Error::LibrarySavedPending { .. }),
        "an uncommitted intent is NotSaved, never saved-pending: {error:?}"
    );
    assert_eq!(
        fixture.fingerprint(),
        before,
        "no database-only favorite fact may be projected"
    );
    assert_eq!(
        fixture.replay(),
        replay,
        "a rejected intent is absent from the directory replay"
    );
    assert!(
        !SongRepository::by_id(&*fixture.database, song)
            .unwrap()
            .unwrap()
            .favorite()
    );

    // The retry after the directory accepts the intent starts from the same
    // pre-commit state and commits exactly one favorite fact.
    fixture.authority.set_refused(false);
    assert!(
        edits
            .set_favorite(fixture.scan.root, song, true)
            .expect("retry commits")
            .favorite()
    );
    let saved = fixture.replay();
    assert_eq!(
        saved.commits.len(),
        replay.commits.len() + 1,
        "the retry adds exactly one commit"
    );
}
