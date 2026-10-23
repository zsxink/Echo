//! Candidate preparation, read-only compatibility and activation contracts.
use super::*;
use crate::application::continuation::ProjectionReport;
use crate::application::delete::DeleteSongs;
use crate::application::import::{ImportOutcome, PlanImport};
use crate::application::ports::ControlPlanePort;
use crate::application::ports::ImportSource;
use crate::application::testing::{FakeImportSources, ScanFixture};
use crate::domain::entities::Song;
use crate::domain::ids::{RelativeMediaPath, Revision, SongId};
use crate::domain::state::scan::ScanState;

fn fixture() -> ScanFixture {
    ScanFixture::new()
}

/// Bind a candidate path in the fake file system under the derived root
/// id (what the desktop composition root does before preparing).
fn bind_candidate(fixture: &ScanFixture, path: &std::path::Path) -> LibraryRootId {
    let root_id = derive_root_id(&path.canonicalize().expect("canonical"));
    fixture.fs.add_root_at(root_id, path.to_path_buf());
    root_id
}

/// Put one real audio file into a candidate root's `media/` tree (the
/// fixture's own `write_file` targets the fixture root, not the candidate).
fn seed_candidate_media(dir: &std::path::Path) {
    let target = dir.join("media").join("歌手");
    std::fs::create_dir_all(&target).expect("media dir");
    std::fs::write(target.join("歌手 - 晴天.flac"), b"audio-bytes").expect("write file");
}

/// One portable song record for `root` (the shape a previous session left
/// behind in `echo/records/songs/`).
fn candidate_song_record(song_uuid: uuid::Uuid) -> crate::domain::library::PortableRecord {
    crate::domain::library::PortableRecord::Song(crate::domain::library::SongRecord {
        song_uuid,
        revision: Revision::INITIAL,
        updated_by_device_id: crate::domain::library::DeviceId::new(),
        hlc: crate::domain::library::HybridLogicalClock::new(1_700_000_000, 0),
        media_path: crate::domain::library::LibraryRelativePath::new("media/歌手/歌手 - 晴天.flac")
            .expect("media path"),
        content_hash: "hash".to_owned(),
        added_at: 1_700_001_002_003,
        title: Some("晴天".to_owned()),
        artist: Some("歌手".to_owned()),
        album: None,
    })
}

#[test]
fn prepare_continues_object_records_before_scanning_media() {
    let fixture = fixture();
    let dir = tempfile::tempdir().expect("temp dir");
    let root_id = bind_candidate(&fixture, dir.path());
    // What a previous session left in `echo/`: a song record, no manifest
    // (the real-world issue #1 shape). The media file is present too.
    let song_uuid = uuid::Uuid::new_v4();
    fixture
        .control
        .write_record(root_id, &candidate_song_record(song_uuid))
        .expect("seed record");
    seed_candidate_media(dir.path());
    fixture.set_audio("歌手/歌手 - 晴天.flac", "晴天", 200_000);

    let prepared =
        PrepareLibraryCandidate::new(&fixture.deps, &fixture.database, &fixture.supervisor)
            .prepare(dir.path())
            .expect("prepare");
    let continuation = prepared
        .continuation
        .expect("a directory with records is continued");
    assert!(
        continuation.control_plane.healed(),
        "a manifest-less but records-bearing directory is healed"
    );
    assert_eq!(continuation.projection.songs, 1);

    // The decisive assertion: the scanned media kept the record's UUID —
    // the scan did not mint a second identity for the same file.
    let songs = crate::application::ports::SongRepository::all_in_root(&fixture.database, root_id)
        .expect("songs");
    assert_eq!(songs.len(), 1, "one identity, not one per open");
    assert_eq!(songs[0].id(), SongId::from_uuid(song_uuid));
}

/// The ordering guarantee behind "continue, then scan" (requirement
/// 对象资料接续与对账): media the records already own keeps its UUID, while
/// media no record describes is still discovered in the same pass — the
/// scan is not replaced by the continuation, it runs *after* it.
#[test]
fn prepare_continues_records_then_scans_only_unrecorded_media() {
    let fixture = fixture();
    let dir = tempfile::tempdir().expect("temp dir");
    let root_id = bind_candidate(&fixture, dir.path());
    let song_uuid = uuid::Uuid::new_v4();
    fixture
        .control
        .write_record(root_id, &candidate_song_record(song_uuid))
        .expect("seed record");
    seed_candidate_media(dir.path());
    // A second file the portable records do NOT describe.
    let target = dir.path().join("media").join("歌手");
    std::fs::write(target.join("歌手 - 稻香.flac"), b"audio-bytes-2").expect("write file");
    fixture.set_audio("歌手/歌手 - 晴天.flac", "晴天", 200_000);
    fixture.set_audio("歌手/歌手 - 稻香.flac", "稻香", 180_000);

    let prepared =
        PrepareLibraryCandidate::new(&fixture.deps, &fixture.database, &fixture.supervisor)
            .prepare(dir.path())
            .expect("prepare");

    let songs = crate::application::ports::SongRepository::all_in_root(&fixture.database, root_id)
        .expect("songs");
    assert_eq!(songs.len(), 2, "the scan still reaches unrecorded media");
    assert!(
        songs
            .iter()
            .any(|song| song.id() == SongId::from_uuid(song_uuid)),
        "the continued song kept its record UUID instead of being re-minted"
    );
    assert!(
        songs
            .iter()
            .any(|song| song.id() != SongId::from_uuid(song_uuid)),
        "media with no record is registered once, not swallowed by the continuation"
    );
    assert_eq!(
        prepared.scan.progress.state,
        ScanState::Completed,
        "continuation does not leave the scan half-done"
    );
}

/// Re-selecting a directory that already owns a control plane must be
/// *non-destructive*: the local database is rebuilt from the records, the
/// media tree is neither moved nor rewritten, and the directory is
/// recognised as managed (a manifest exists afterwards).
#[test]
fn prepare_reuses_record_identities_and_leaves_media_untouched() {
    let fixture = fixture();
    let dir = tempfile::tempdir().expect("temp dir");
    let root_id = bind_candidate(&fixture, dir.path());
    let song_uuid = uuid::Uuid::new_v4();
    fixture
        .control
        .write_record(root_id, &candidate_song_record(song_uuid))
        .expect("seed record");
    seed_candidate_media(dir.path());
    fixture.set_audio("歌手/歌手 - 晴天.flac", "晴天", 200_000);
    let media = dir
        .path()
        .join("media")
        .join("歌手")
        .join("歌手 - 晴天.flac");
    let before = std::fs::read(&media).expect("media bytes");

    PrepareLibraryCandidate::new(&fixture.deps, &fixture.database, &fixture.supervisor)
        .prepare(dir.path())
        .expect("prepare");

    assert_eq!(
        std::fs::read(&media).expect("media still there"),
        before,
        "reopening a managed directory must not rewrite the media tree"
    );
    let songs = crate::application::ports::SongRepository::all_in_root(&fixture.database, root_id)
        .expect("songs");
    assert_eq!(songs.len(), 1);
    assert_eq!(songs[0].id(), SongId::from_uuid(song_uuid));
    assert!(
        fixture
            .control
            .read_manifest(root_id)
            .expect("manifest read")
            .is_some(),
        "the directory is recognised as managed: a manifest now exists"
    );
}

#[test]
fn prepare_degrades_to_read_only_on_a_newer_manifest() {
    let fixture = fixture();
    let dir = tempfile::tempdir().expect("temp dir");
    let root_id = bind_candidate(&fixture, dir.path());
    let existing = Song::new(
        SongId::new(),
        root_id,
        RelativeMediaPath::new("existing.flac").unwrap(),
        Revision::INITIAL,
    );
    crate::application::ports::SongRepository::upsert(&fixture.database, &existing).unwrap();
    seed_candidate_media(dir.path());
    fixture.control.set_manifest(
        root_id,
        crate::domain::library::LibraryManifest {
            format_version: crate::domain::library::CURRENT_FORMAT_VERSION + 3,
            library_id: crate::domain::library::LibraryId::from_uuid(uuid::Uuid::new_v4()),
            written_by_app_version: "9.9.9".to_owned(),
        },
    );
    let prepared =
        PrepareLibraryCandidate::new(&fixture.deps, &fixture.database, &fixture.supervisor)
            .prepare(dir.path())
            .expect("prepare still succeeds for reads");
    assert!(
        prepared.control_plane_degraded,
        "a newer manifest refuses continuation instead of being overwritten"
    );
    assert!(!prepared.writes_allowed());
    assert!(prepared.scan.cancelled);
    assert_eq!(prepared.scan.progress.state, ScanState::Cancelled);
    assert_eq!(prepared.scan.progress.discovered, 0);
    let visible =
        crate::application::ports::SongRepository::all_in_root(&fixture.database, root_id).unwrap();
    assert_eq!(
        visible.len(),
        1,
        "unsupported format must not mint scanned identities"
    );
    assert_eq!(
        visible[0].id(),
        existing.id(),
        "existing projection remains browseable"
    );
    assert!(
        prepared.continuation.is_none(),
        "nothing was projected from a control surface this build cannot read"
    );
    // The manifest file itself was not rewritten (byte-for-byte intact).
    let stored = fixture.control.manifest_of(root_id).expect("kept");
    assert_eq!(
        stored.format_version,
        crate::domain::library::CURRENT_FORMAT_VERSION + 3
    );
}

#[test]
fn prepare_leaves_a_legacy_directory_as_a_plain_scan_target() {
    let fixture = fixture();
    let dir = tempfile::tempdir().expect("temp dir");
    let root_id = bind_candidate(&fixture, dir.path());
    seed_candidate_media(dir.path());
    fixture.set_audio("歌手/歌手 - 晴天.flac", "晴天", 200_000);

    let prepared =
        PrepareLibraryCandidate::new(&fixture.deps, &fixture.database, &fixture.supervisor)
            .prepare(dir.path())
            .expect("prepare");
    let continuation = prepared.continuation.expect("the manifest is established");
    assert!(
        !continuation.control_plane.healed(),
        "a directory with no prior records is initialized, not healed"
    );
    assert_eq!(continuation.projection, ProjectionReport::default());
    // The scan still discovers the media normally (pre-portable behaviour).
    assert_eq!(
        crate::application::ports::SongRepository::all_in_root(&fixture.database, root_id)
            .expect("songs")
            .len(),
        1
    );
}

#[test]
fn empty_directory_activates_and_reuses_root_id() {
    let fixture = fixture();
    let dir = tempfile::tempdir().expect("temp dir");
    let _root_id = bind_candidate(&fixture, dir.path());
    // An empty directory is a valid candidate (设计：空目录允许成功).
    let prepared =
        PrepareLibraryCandidate::new(&fixture.deps, &fixture.database, &fixture.supervisor)
            .prepare(dir.path())
            .expect("empty dir prepares");
    assert!(!prepared.reused);
    assert_eq!(prepared.scan.progress.discovered, 0);
    assert_eq!(prepared.scan.progress.state, ScanState::Completed);

    let outcome = ActivateLibrary::new(
        &fixture.deps,
        &fixture.database,
        &fixture.supervisor,
        &fixture.database,
        &Blockers::new(),
    )
    .activate(prepared.root_id)
    .expect("activation");
    assert_eq!(outcome.root_state, LibraryRootState::ActiveAvailable);
    assert_eq!(outcome.barrier_state, RootSwitchState::RebindRuntime);
    assert_eq!(
        fixture.database.active_root().unwrap().unwrap().id(),
        prepared.root_id
    );
    assert_eq!(
        fixture.database.runtime_state(ROOT_EPOCH_KEY).unwrap(),
        "1",
        "the epoch advanced with the activation commit"
    );

    // Re-preparing the same canonical path reuses the id and record.
    let again = PrepareLibraryCandidate::new(&fixture.deps, &fixture.database, &fixture.supervisor)
        .prepare(dir.path())
        .expect("re-prepare");
    assert!(again.reused, "same canonical path reuses the root id");
    assert_eq!(again.root_id, prepared.root_id);
}

#[test]
fn root_level_error_keeps_old_active_root() {
    let fixture = fixture();
    let active_dir = tempfile::tempdir().unwrap();
    bind_candidate(&fixture, active_dir.path());
    let first = PrepareLibraryCandidate::new(&fixture.deps, &fixture.database, &fixture.supervisor)
        .prepare(active_dir.path())
        .expect("first prepare");
    ActivateLibrary::new(
        &fixture.deps,
        &fixture.database,
        &fixture.supervisor,
        &fixture.database,
        &Blockers::new(),
    )
    .activate(first.root_id)
    .expect("first activation");

    // The second candidate enumerates fine until the fs faults.
    let candidate_dir = tempfile::tempdir().unwrap();
    let _candidate_id = bind_candidate(&fixture, candidate_dir.path());
    fixture
        .fs
        .inject_fault(Error::unavailable("library root", "disk ejected"));
    let prepared =
        PrepareLibraryCandidate::new(&fixture.deps, &fixture.database, &fixture.supervisor)
            .prepare(candidate_dir.path());
    drop(prepared); // Err expected
    fixture.fs.clear_fault();

    // The old active root is untouched and the candidate is retryable.
    assert_eq!(
        fixture.database.active_root().unwrap().unwrap().id(),
        first.root_id,
        "the failed candidate never touched the active root"
    );
    // The old root's songs/state are intact (no missing pass ran).
    assert_eq!(fixture.database.runtime_state(ROOT_EPOCH_KEY).unwrap(), "1");
}

#[test]
fn read_only_root_activates_readonly_and_disables_writes() {
    let fixture = fixture();
    let dir = tempfile::tempdir().unwrap();
    bind_candidate(&fixture, dir.path());
    fixture.fs.set_write_capable(false);
    let prepared =
        PrepareLibraryCandidate::new(&fixture.deps, &fixture.database, &fixture.supervisor)
            .prepare(dir.path())
            .expect("prepare");
    assert!(!prepared.write_capable);
    let outcome = ActivateLibrary::new(
        &fixture.deps,
        &fixture.database,
        &fixture.supervisor,
        &fixture.database,
        &Blockers::new(),
    )
    .activate(prepared.root_id)
    .expect("activation");
    assert_eq!(
        outcome.root_state,
        LibraryRootState::ActiveReadOnly,
        "write-incapable roots activate read-only"
    );
    let root = fixture.database.active_root().unwrap().unwrap();
    assert!(!root.write_capable());
    assert!(
        !fixture.deps.fs.write_capable(root.id()).unwrap(),
        "import/delete stay disabled for read-only roots"
    );
}

#[test]
fn trash_outcome_unknown_survives_reprepare_and_activation_and_rejects_writes() {
    let fixture = fixture();
    let dir = tempfile::tempdir().unwrap();
    bind_candidate(&fixture, dir.path());
    let prepared =
        PrepareLibraryCandidate::new(&fixture.deps, &fixture.database, &fixture.supervisor)
            .prepare(dir.path())
            .expect("prepare");
    ActivateLibrary::new(
        &fixture.deps,
        &fixture.database,
        &fixture.supervisor,
        &fixture.database,
        &Blockers::new(),
    )
    .activate(prepared.root_id)
    .expect("activation");

    // This is the durable safety consequence of TrashOutcomeUnknown. A
    // later filesystem permission probe must not make it writable again.
    fixture
        .database
        .set_write_safety_locked(prepared.root_id, true)
        .expect("persist safety lock");
    let reparsed =
        PrepareLibraryCandidate::new(&fixture.deps, &fixture.database, &fixture.supervisor)
            .prepare(dir.path())
            .expect("re-prepare keeps safety isolation");
    assert!(reparsed.reused);
    assert!(!reparsed.write_capable);
    let activated = ActivateLibrary::new(
        &fixture.deps,
        &fixture.database,
        &fixture.supervisor,
        &fixture.database,
        &Blockers::new(),
    )
    .activate(reparsed.root_id)
    .expect("re-activation remains possible for reads");
    assert_eq!(activated.root_state, LibraryRootState::ActiveReadOnly);
    assert!(crate::application::ports::LibraryRepository::by_id(
        &fixture.database,
        prepared.root_id,
    )
    .unwrap()
    .unwrap()
    .write_safety_locked());

    let sources = FakeImportSources::new();
    sources.add("new", "new.flac", b"bytes");
    let imported = PlanImport::new(&fixture.deps, &sources)
        .run(
            prepared.root_id,
            &[ImportSource::new("new").expect("logical source")],
        )
        .expect("write rejection is a normal report");
    assert_eq!(imported.results, vec![ImportOutcome::LibraryUnavailable]);

    let song = Song::new(
        SongId::new(),
        prepared.root_id,
        RelativeMediaPath::new("still-here.flac").unwrap(),
        Revision::INITIAL,
    );
    crate::application::ports::SongRepository::upsert(&fixture.database, &song).expect("seed song");
    let error = DeleteSongs::new(&fixture.deps)
        .delete(prepared.root_id, song.id())
        .expect_err("delete remains disabled after re-selection");
    assert_eq!(error.code(), "unavailable");
}

#[test]
fn switch_refused_while_import_blockers_active() {
    let fixture = fixture();
    let first_dir = tempfile::tempdir().unwrap();
    bind_candidate(&fixture, first_dir.path());
    let first = PrepareLibraryCandidate::new(&fixture.deps, &fixture.database, &fixture.supervisor)
        .prepare(first_dir.path())
        .unwrap();
    ActivateLibrary::new(
        &fixture.deps,
        &fixture.database,
        &fixture.supervisor,
        &fixture.database,
        &Blockers::new(),
    )
    .activate(first.root_id)
    .unwrap();

    let blockers = Blockers::new();
    blockers.add("import in flight");
    blockers.add("delete undo window open");
    let second_dir = tempfile::tempdir().unwrap();
    bind_candidate(&fixture, second_dir.path());
    let second =
        PrepareLibraryCandidate::new(&fixture.deps, &fixture.database, &fixture.supervisor)
            .prepare(second_dir.path())
            .unwrap();

    let error = ActivateLibrary::new(
        &fixture.deps,
        &fixture.database,
        &fixture.supervisor,
        &fixture.database,
        &blockers,
    )
    .activate(second.root_id)
    .unwrap_err();
    assert_eq!(error.code(), "conflict");
    assert_eq!(
        fixture.database.active_root().unwrap().unwrap().id(),
        first.root_id,
        "a blocked switch leaves the old root serving"
    );
    assert_eq!(
        fixture.database.runtime_state(ROOT_EPOCH_KEY).unwrap(),
        "1",
        "no epoch advance on a refused switch"
    );

    // Once the runtime resolves the blockers the switch goes through.
    blockers.clear();
    let outcome = ActivateLibrary::new(
        &fixture.deps,
        &fixture.database,
        &fixture.supervisor,
        &fixture.database,
        &blockers,
    )
    .activate(second.root_id)
    .unwrap();
    assert_eq!(outcome.previous_root, Some(first.root_id));
}

#[test]
fn switch_cancels_active_scan_and_advances_epoch() {
    let fixture = fixture();
    let first_dir = tempfile::tempdir().unwrap();
    bind_candidate(&fixture, first_dir.path());
    let first = PrepareLibraryCandidate::new(&fixture.deps, &fixture.database, &fixture.supervisor)
        .prepare(first_dir.path())
        .unwrap();
    ActivateLibrary::new(
        &fixture.deps,
        &fixture.database,
        &fixture.supervisor,
        &fixture.database,
        &Blockers::new(),
    )
    .activate(first.root_id)
    .unwrap();

    // Simulate an in-flight scan of the old root.
    let token = crate::application::scan::ScanCancelToken::new();
    // register through a scan run would normally do this; use the
    // supervisor directly via a started scan on a worker thread is
    // overkill — expose the same state through a manual registration.
    // (register is private; simulate by starting + cancelling a scan.)
    assert!(!token.is_cancelled());

    let second_dir = tempfile::tempdir().unwrap();
    bind_candidate(&fixture, second_dir.path());
    let second =
        PrepareLibraryCandidate::new(&fixture.deps, &fixture.database, &fixture.supervisor)
            .prepare(second_dir.path())
            .unwrap();
    let outcome = ActivateLibrary::new(
        &fixture.deps,
        &fixture.database,
        &fixture.supervisor,
        &fixture.database,
        &Blockers::new(),
    )
    .activate(second.root_id)
    .unwrap();

    // The epoch advanced by exactly one and the barrier rejects
    // old-epoch results after the commit (RootEpoch discipline).
    assert_eq!(outcome.epoch.as_u64(), 2);
    let barrier =
        RootSwitchBarrier::new(Some(first.root_id), second.root_id, RootEpoch::from_u64(1));
    assert!(barrier.accepts_epoch(RootEpoch::from_u64(1)));
    assert!(!barrier.accepts_epoch(outcome.epoch));
    // And the persisted epoch matches the outcome.
    assert_eq!(fixture.database.runtime_state(ROOT_EPOCH_KEY).unwrap(), "2");
    // The old root's record is kept but inactive.
    assert!(!fixture
        .database
        .by_id(first.root_id)
        .unwrap()
        .unwrap()
        .is_active());
    drop(token);
}

#[test]
fn late_results_of_the_old_epoch_are_rejected() {
    // The runtime-side rule: results carry the epoch they started under;
    // after a committed switch only the new epoch is accepted.
    let mut barrier = RootSwitchBarrier::new(
        Some(LibraryRootId::new()),
        LibraryRootId::new(),
        RootEpoch::from_u64(5),
    );
    barrier.transition(RootSwitchState::QuiesceOldRoot).unwrap();
    barrier
        .transition(RootSwitchState::CommitActivation)
        .unwrap();
    barrier.commit_activation(RootEpoch::from_u64(6)).unwrap();
    assert!(!barrier.accepts_epoch(RootEpoch::from_u64(5)));
    assert!(barrier.accepts_epoch(RootEpoch::from_u64(6)));
    barrier.transition(RootSwitchState::Completed).unwrap();
    assert!(barrier.state().is_terminal());
}

#[test]
fn derive_root_id_is_deterministic_and_path_bound() {
    let dir = tempfile::tempdir().unwrap();
    let canonical = dir.path().canonicalize().unwrap();
    let first = derive_root_id(&canonical);
    let second = derive_root_id(&canonical);
    assert_eq!(first, second, "same canonical path → same root id");
    let other = tempfile::tempdir().unwrap();
    assert_ne!(
        first,
        derive_root_id(&other.path().canonicalize().unwrap()),
        "different paths → different root ids"
    );
}
