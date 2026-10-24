//! Root-identity path-form invariants (issue #82).
//!
//! The persisted root record must carry the plain, user-visible path while
//! the root id stays derived from the raw canonical form (`std::fs::canonicalize`,
//! verbatim on Windows). Deriving the id from the plain path instead would
//! re-enroll every existing Windows library as a new identity and orphan its
//! songs, favorites and play history.
//!
//! `ScanFixture` only exists under the `testkit` feature; CI always runs with
//! `--all-features`, local runs should pass `-p echo-core --all-features`.

#![cfg(feature = "testkit")]

use echo_core::application::ports::LibraryRepository;
use echo_core::application::root_switch::{derive_root_id, PrepareLibraryCandidate};
use echo_core::application::testkit::ScanFixture;
use echo_core::domain::entities::LibraryRoot;
use echo_core::domain::paths::plain_absolute;

fn candidate_with_media() -> (ScanFixture, tempfile::TempDir) {
    let fixture = ScanFixture::new();
    let dir = tempfile::tempdir().expect("temp dir");
    let target = dir.path().join("media").join("歌手");
    std::fs::create_dir_all(&target).expect("media dir");
    std::fs::write(target.join("歌手 - 晴天.flac"), b"audio-bytes").expect("write file");
    fixture.set_audio("歌手/歌手 - 晴天.flac", "晴天", 200_000);
    (fixture, dir)
}

#[test]
fn prepare_reuses_the_raw_canonical_identity_and_stores_the_plain_path() {
    let (fixture, dir) = candidate_with_media();
    let canonical = dir.path().canonicalize().expect("canonical");
    let plain = plain_absolute(&canonical);
    // The identity a previous session derived for this directory.
    let root_id = derive_root_id(&canonical);
    fixture
        .database
        .upsert(&LibraryRoot::new(root_id, plain.clone(), false, true))
        .expect("seed existing root");
    fixture.fs.add_root_at(root_id, dir.path().to_path_buf());

    let prepared =
        PrepareLibraryCandidate::new(&fixture.deps, &fixture.database, &fixture.supervisor)
            .prepare(&plain)
            .expect("prepare");
    assert!(prepared.reused, "the pre-existing identity must be reused");
    let stored = fixture
        .database
        .by_id(root_id)
        .expect("read back")
        .expect("root present");
    assert_eq!(
        stored.absolute_path(),
        plain.as_path(),
        "the record keeps the plain, user-visible spelling"
    );
}

#[cfg(windows)]
#[test]
fn prepared_record_is_never_verbatim() {
    let (fixture, dir) = candidate_with_media();
    let canonical = dir.path().canonicalize().expect("canonical");
    let root_id = derive_root_id(&canonical);
    fixture.fs.add_root_at(root_id, dir.path().to_path_buf());

    PrepareLibraryCandidate::new(&fixture.deps, &fixture.database, &fixture.supervisor)
        .prepare(dir.path())
        .expect("prepare");
    let stored = fixture
        .database
        .by_id(root_id)
        .expect("read back")
        .expect("root present");
    assert!(
        !stored
            .absolute_path()
            .to_string_lossy()
            .starts_with(r"\\?\"),
        "persisted root path must be plain: {:?}",
        stored.absolute_path()
    );
}
