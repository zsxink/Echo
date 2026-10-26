use super::*;
use echo_core::domain::entities::LibraryRoot;
use echo_core::domain::ids::{Revision, SongId};
use echo_core::domain::library::durability::{ChangeOrigin, LibraryBatch};
use echo_core::domain::library::{
    HybridLogicalClock, LibraryId, LibraryManifest, LibraryRelativePath, PortableRecord,
    PortableRecordEnvelope, RecordKind, SongRecord,
};
use echo_core::infrastructure::filesystem::RootControlPlane;
use std::sync::atomic::{AtomicBool, Ordering};

struct Fixture {
    _directory: tempfile::TempDir,
    routed: crate::runtime::app::RoutedRuntime,
    root: LibraryRootId,
    library: LibraryId,
    song: SongId,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let routed = crate::runtime::app::assemble(
            &directory.path().join("local.db"),
            &directory.path().join("covers"),
        )
        .unwrap();
        let root = LibraryRootId::new();
        let library = LibraryId::from_uuid(uuid::Uuid::new_v4());
        let path = directory.path().join("library");
        std::fs::create_dir(&path).unwrap();
        routed.registry.register(root, &path);
        LibraryRepository::upsert(
            routed.database.as_ref(),
            &LibraryRoot::new(root, path, true, true),
        )
        .unwrap();
        routed
            .deps
            .control
            .write_manifest(root, &LibraryManifest::new(library, "recorder retry"))
            .unwrap();
        let song = SongId::new();
        let record = PortableRecord::Song(SongRecord {
            song_uuid: song.as_uuid(),
            media_path: LibraryRelativePath::new("media/playback.flac").unwrap(),
            content_hash: "a".repeat(64),
            added_at: 123,
            title: Some("Playback retry".into()),
            artist: None,
            album: None,
            revision: Revision::from_u64(1),
            hlc: HybridLogicalClock::new(1, 0),
            updated_by_device_id: routed.deps.device_id.current_device_id(),
        });
        let durability = routed.deps.control.durability().unwrap();
        durability
            .commit(
                root,
                &LibraryBatch::new(
                    uuid::Uuid::new_v4(),
                    library,
                    ChangeOrigin::Scan,
                    vec![PortableRecordEnvelope::new(record, None).unwrap()],
                ),
            )
            .unwrap();
        routed
            .database
            .project_library_commits(root, &durability.recover(root).unwrap())
            .unwrap();
        Self {
            _directory: directory,
            routed,
            root,
            library,
            song,
        }
    }

    fn recorder(&self, control: Arc<dyn ControlPlanePort>) -> CorePlaybackRecorder {
        CorePlaybackRecorder::new(
            self.routed.database.clone(),
            control,
            self.routed.deps.device_id.clone(),
            self.routed.deps.roots.clone(),
            self.routed.deps.activity.clone(),
        )
    }

    fn pending(&self, session: PlaybackSessionId) -> PendingPlayback {
        PendingPlayback {
            root: self.root,
            library: self.library,
            author: self.routed.deps.device_id.current_device_id(),
            session,
            song: self.song,
            qualified_at: 123_456,
            reason: PlaybackRetryReason::NotSaved,
            attempts: 0,
        }
    }

    fn count(&self) -> u64 {
        SongRepository::by_id(self.routed.database.as_ref(), self.song)
            .unwrap()
            .unwrap()
            .play_count()
            .as_u64()
    }
}

#[test]
fn library_durability_recorder_reopens_unsaved_intent_and_commits_the_same_session_once() {
    let fixture = Fixture::new();
    let failed = Arc::new(AtomicBool::new(true));
    let fail_hook = failed.clone();
    let control = Arc::new(
        RootControlPlane::new(fixture.routed.registry.clone()).with_commit_hook(move |at, _| {
            if at == "prepared" && fail_hook.load(Ordering::SeqCst) {
                return Err(echo_core::error::Error::unavailable(
                    "playback directory",
                    "injected unavailable resource",
                ));
            }
            Ok(())
        }),
    );
    // The commit authority is the database's configured plane; the recorder's
    // control argument only cross-checks manifests (plan_library_change).
    fixture
        .routed
        .database
        .configure_library_control(control.clone());
    let session = PlaybackSessionId::new();
    let recorder = fixture.recorder(control.clone());
    assert!(recorder.record(session, fixture.song).is_err());
    assert_eq!(fixture.count(), 0);
    let status = recorder.retry_status(fixture.root).unwrap();
    assert_eq!(status.len(), 1);
    assert_eq!(status[0].operation_id, session);
    assert_eq!(status[0].reason, PlaybackRetryReason::NotSaved);
    drop(recorder);

    failed.store(false, Ordering::SeqCst);
    let reopened = fixture.recorder(control.clone());
    reopened.retry_pending().unwrap();
    assert_eq!(fixture.count(), 1);
    assert!(reopened.retry_status(fixture.root).unwrap().is_empty());
    assert!(!reopened.record(session, fixture.song).unwrap());
    assert_eq!(fixture.count(), 1);
    let shared_stats = control
        .read_record(
            fixture.root,
            RecordKind::PlayStats,
            &fixture.song.to_string(),
        )
        .unwrap()
        .unwrap();
    let shared = serde_json::to_string(&shared_stats).unwrap();
    assert!(!shared.contains(&session.to_string()));
    assert!(!shared.contains("session"));
    assert!(!shared.contains("event"));
}

#[test]
fn library_durability_recorder_lost_response_uses_saved_event_without_reattributing_author() {
    let fixture = Fixture::new();
    let session = PlaybackSessionId::new();
    let recorder = fixture.recorder(fixture.routed.deps.control.clone());
    assert!(recorder.record(session, fixture.song).unwrap());
    let mut entry = fixture.pending(session);
    // The local database/device may have been replaced after the event was
    // committed. The directory proof must repair projection, never make a play
    // on behalf of this different author.
    entry.author = echo_core::domain::library::DeviceId::new();
    recorder.retries.enqueue(entry).unwrap();
    drop(recorder);
    let reopened = fixture.recorder(fixture.routed.deps.control.clone());
    reopened.retry_pending().unwrap();
    assert_eq!(fixture.count(), 1);
    assert!(reopened.retry_status(fixture.root).unwrap().is_empty());
    let counts = fixture
        .routed
        .database
        .device_play_counts(fixture.song)
        .unwrap();
    assert_eq!(counts.len(), 1);
    assert_eq!(counts.values().copied().sum::<u64>(), 1);
}

#[test]
fn library_durability_recorder_keeps_unproven_author_library_and_song_requests_pending() {
    let fixture = Fixture::new();
    let recorder = fixture.recorder(fixture.routed.deps.control.clone());
    let mut old_author = fixture.pending(PlaybackSessionId::new());
    old_author.author = echo_core::domain::library::DeviceId::new();
    let mut wrong_library = fixture.pending(PlaybackSessionId::new());
    wrong_library.library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let mut missing_song = fixture.pending(PlaybackSessionId::new());
    missing_song.song = SongId::new();
    for entry in [old_author, wrong_library, missing_song] {
        recorder.retries.enqueue(entry).unwrap();
    }
    recorder.retry_pending().unwrap();
    let status = recorder.retry_status(fixture.root).unwrap();
    assert_eq!(status.len(), 3);
    let reasons: Vec<_> = status.iter().map(|entry| entry.reason).collect();
    assert!(reasons.contains(&PlaybackRetryReason::AuthorChanged));
    assert!(reasons.contains(&PlaybackRetryReason::LibraryChanged));
    assert!(reasons.contains(&PlaybackRetryReason::MissingSong));
    assert_eq!(fixture.count(), 0);
    assert!(fixture
        .routed
        .database
        .device_play_counts(fixture.song)
        .unwrap()
        .is_empty());
}

#[test]
fn library_durability_recorder_does_not_retry_into_an_inactive_root() {
    let fixture = Fixture::new();
    let recorder = fixture.recorder(fixture.routed.deps.control.clone());
    let entry = fixture.pending(PlaybackSessionId::new());
    recorder.retries.enqueue(entry).unwrap();
    fixture.routed.deps.roots.deactivate(fixture.root).unwrap();
    recorder.retry_pending().unwrap();
    let status = recorder.retry_status(fixture.root).unwrap();
    assert_eq!(status.len(), 1);
    assert_eq!(status[0].reason, PlaybackRetryReason::InactiveLibrary);
    assert_eq!(fixture.count(), 0);
}
