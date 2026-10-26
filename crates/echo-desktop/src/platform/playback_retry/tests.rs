use super::*;

fn entry() -> PendingPlayback {
    PendingPlayback {
        root: LibraryRootId::new(),
        library: LibraryId::from_uuid(uuid::Uuid::new_v4()),
        author: DeviceId::new(),
        session: PlaybackSessionId::new(),
        song: SongId::new(),
        qualified_at: 123,
        reason: PlaybackRetryReason::NotSaved,
        attempts: 0,
    }
}

#[test]
fn playback_retry_queue_survives_reopen_and_never_changes_event_identity() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("app.sqlite");
    let store = PlaybackRetryStore::beside_database(&path);
    let intent = entry();
    store.enqueue(intent.clone()).unwrap();
    store.enqueue(intent.clone()).unwrap();
    store
        .failed(&intent, PlaybackRetryReason::SavedPendingProjection)
        .unwrap();
    drop(store);
    let reopened = PlaybackRetryStore::beside_database(&path);
    let pending = reopened.pending().unwrap();
    assert_eq!(pending.len(), 1);
    assert!(pending[0].same_intent(&intent));
    assert_eq!(pending[0].qualified_at, 123);
    assert_eq!(
        pending[0].reason,
        PlaybackRetryReason::SavedPendingProjection
    );
    assert_eq!(pending[0].attempts, 1);
    let mut conflicting = intent.clone();
    conflicting.author = DeviceId::new();
    assert!(reopened.enqueue(conflicting).is_err());
    assert_eq!(reopened.pending().unwrap(), pending);
    reopened.complete(&intent).unwrap();
    assert!(reopened.pending().unwrap().is_empty());
}

#[test]
fn playback_retry_queue_preserves_damaged_and_future_originals() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("app.sqlite");
    let store = PlaybackRetryStore::beside_database(&path);
    for bytes in [
        b"damaged".as_slice(),
        br#"{"format_version":99,"pending":[]}"#,
    ] {
        std::fs::write(&store.0.path, bytes).unwrap();
        assert!(store.enqueue(entry()).is_err());
        assert_eq!(std::fs::read(&store.0.path).unwrap(), bytes);
    }
}

#[test]
fn playback_retry_queue_status_is_root_scoped_and_path_free() {
    let directory = tempfile::tempdir().unwrap();
    let store = PlaybackRetryStore::beside_database(&directory.path().join("app.sqlite"));
    let first = entry();
    let second = entry();
    store.enqueue(first.clone()).unwrap();
    store.enqueue(second).unwrap();
    let status = store.status(first.root).unwrap();
    assert_eq!(status.len(), 1);
    assert_eq!(status[0].operation_id, first.session);
    assert_eq!(status[0].reason, PlaybackRetryReason::NotSaved);
}
