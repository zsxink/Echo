//! Cancellation arriving while the final real directory transaction publishes.
use super::*;
use echo_core::application::ports::{ProbeOutcome, SongRepository};
use echo_core::application::testing::small_fakes::{FakeMediaProbe, FakeMetadataReader};
use echo_core::domain::entities::SongAvailability;
use echo_core::infrastructure::filesystem::RootControlPlane;
use std::sync::{mpsc, Arc};

#[test]
fn cancellation_during_final_directory_commit_never_marks_other_songs_missing() {
    let (reached, _paused) = mpsc::channel();
    let (_release, resume) = mpsc::channel();
    let f = lifecycle::real_fixture("never", reached, resume);
    let root_path = f
        .app
        .deps
        .roots
        .by_id(f.root)
        .unwrap()
        .unwrap()
        .absolute_path()
        .to_path_buf();
    std::fs::create_dir_all(root_path.join("media")).unwrap();
    let probe = FakeMediaProbe::new();
    let metadata = FakeMetadataReader::new();
    for (name, bytes) in [
        ("gone.flac", b"first".as_slice()),
        ("new.flac", b"second".as_slice()),
    ] {
        probe.set(
            name,
            ProbeOutcome::Audio {
                format: AudioFormat::Flac,
                duration: Some(std::time::Duration::from_secs(5)),
            },
        );
        metadata.set(
            name,
            ParsedMetadata {
                title: Some(name.to_owned()),
                format: AudioFormat::Flac,
                ..ParsedMetadata::default()
            },
        );
        if name == "gone.flac" {
            std::fs::write(root_path.join("media").join(name), bytes).unwrap();
        }
    }
    let deps = ScanDeps {
        probe: Arc::new(probe),
        metadata: Arc::new(metadata),
        ..(*f.app.deps).clone()
    };
    StartScan::new(&deps, &f.app.supervisor)
        .run(f.root)
        .unwrap();
    let old = deps.songs.all_in_root(f.root).unwrap().pop().unwrap();
    std::fs::remove_file(root_path.join("media/gone.flac")).unwrap();
    std::fs::write(root_path.join("media/new.flac"), b"second").unwrap();
    let supervisor = f.app.supervisor.clone();
    let control = Arc::new(
        RootControlPlane::new(f.app.registry.clone()).with_commit_hook(move |phase, _| {
            if phase == "committed" {
                let _ = supervisor.cancel_all();
            }
            Ok(())
        }),
    );
    let deps = ScanDeps { control, ..deps };
    let summary = StartScan::new(&deps, &f.app.supervisor)
        .run(f.root)
        .unwrap();
    assert!(
        summary.cancelled,
        "cancellation during final commit was lost"
    );
    assert_eq!(
        SongRepository::by_id(deps.songs.as_ref(), old.id())
            .unwrap()
            .unwrap()
            .availability(),
        SongAvailability::Available
    );
}
