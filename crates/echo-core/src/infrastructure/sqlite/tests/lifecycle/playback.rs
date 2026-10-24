//! Qualified listens retain the private session through projection failures.
use super::*;
use crate::application::playback_recording::record_qualified_playback;
use crate::application::ports::{LibraryProjectionPort, SongRepository};
use crate::domain::ids::{PlaybackSessionId, SongId};
use crate::domain::library::durability::{FrozenMutation, PlaybackEvent, StateKind};

fn record(
    stack: &ScanStack,
    control: &RootControlPlane,
    deps: &ScanDeps,
    session: PlaybackSessionId,
    song: SongId,
) -> Result<bool, crate::error::Error> {
    record_qualified_playback(
        control,
        stack.database.as_ref(),
        stack.database.as_ref(),
        deps.device_id.current_device_id(),
        session,
        song,
    )
}

fn count(database: &SqliteDatabase, song: SongId) -> u64 {
    SongRepository::by_id(database, song)
        .unwrap()
        .unwrap()
        .play_count()
        .as_u64()
}

#[test]
fn library_durability_playback_retry_preserves_saved_pending_session() {
    let stack = ScanStack::new();
    let (_, control, deps) = configured_roots(&stack);
    let song = favorite::seed_song(&stack, &control, &deps);
    let session = PlaybackSessionId::new();
    stack.database.writer.run(|c| {
        c.execute_batch("CREATE TRIGGER fail_playback_projection BEFORE INSERT ON library_projection_watermarks BEGIN SELECT RAISE(ABORT,'injected playback projection failure'); END;").map_err(storage)
    }).unwrap();
    for _ in 0..2 {
        let error = record(&stack, &control, &deps, session, song).unwrap_err();
        assert!(
            matches!(error, crate::error::Error::LibrarySavedPending { operation, .. } if operation.as_uuid() == session.as_uuid()),
            "{error:?}"
        );
        assert_eq!(count(stack.database.as_ref(), song), 0);
        let replay = control.durability().unwrap().recover(stack.root).unwrap();
        assert_eq!(replay.commits.len(), 2);
        assert_eq!(replay.commits[1].batch.operation_id, session.as_uuid());
    }
    stack
        .database
        .writer
        .run(|c| {
            c.execute_batch("DROP TRIGGER fail_playback_projection")
                .map_err(storage)
        })
        .unwrap();
    assert!(!record(&stack, &control, &deps, session, song).unwrap());
    assert!(!record(&stack, &control, &deps, session, song).unwrap());
    assert_eq!(count(stack.database.as_ref(), song), 1);
    assert_eq!(
        control
            .durability()
            .unwrap()
            .recover(stack.root)
            .unwrap()
            .commits
            .len(),
        2
    );
}

#[test]
fn library_durability_playback_later_session_catches_up_and_rebuild_has_no_echo() {
    let stack = ScanStack::new();
    let (_, control, deps) = configured_roots(&stack);
    let song = favorite::seed_song(&stack, &control, &deps);
    let first = PlaybackSessionId::new();
    stack.database.writer.run(|c| {
        c.execute_batch("CREATE TRIGGER fail_playback_projection BEFORE INSERT ON library_projection_watermarks BEGIN SELECT RAISE(ABORT,'injected playback projection failure'); END;").map_err(storage)
    }).unwrap();
    assert!(matches!(
        record(&stack, &control, &deps, first, song),
        Err(crate::error::Error::LibrarySavedPending { .. })
    ));
    stack
        .database
        .writer
        .run(|c| {
            c.execute_batch("DROP TRIGGER fail_playback_projection")
                .map_err(storage)
        })
        .unwrap();
    let second = PlaybackSessionId::new();
    assert!(record(&stack, &control, &deps, second, song).unwrap());
    assert_eq!(count(stack.database.as_ref(), song), 2);
    assert!(!record(&stack, &control, &deps, first, song).unwrap());
    let replay = control.durability().unwrap().recover(stack.root).unwrap();
    assert_eq!(replay.commits.len(), 3);
    let mut frozen = Vec::new();
    for (commit, session) in replay.commits[1..].iter().zip([first, second]) {
        assert_eq!(commit.batch.operation_id, session.as_uuid());
        let event = commit
            .batch
            .state
            .iter()
            .find(|state| state.kind == StateKind::PlaybackEvent)
            .unwrap();
        assert_eq!(event.id, session.as_uuid());
        let event: PlaybackEvent = serde_json::from_value(event.payload.clone()).unwrap();
        assert_eq!(event.song_uuid, song.as_uuid());
        assert!(event.recorded_at > 0);
        let mutation = commit
            .batch
            .state
            .iter()
            .find(|state| state.kind == StateKind::Pending)
            .unwrap();
        let mutation: FrozenMutation =
            serde_json::from_value(mutation.payload["frozen"].clone()).unwrap();
        for private_session in [first, second] {
            assert!(!serde_json::to_string(&mutation.candidate.payload)
                .unwrap()
                .contains(&private_session.to_string()));
            assert!(!serde_json::to_string(&commit.batch.records)
                .unwrap()
                .contains(&private_session.to_string()));
        }
        frozen.push(mutation);
    }
    assert_eq!(frozen[0].candidate.origin_seq, 1);
    assert_eq!(frozen[1].candidate.origin_seq, 2);
    assert_eq!(
        frozen[1].candidate.previous_mutation_id,
        Some(frozen[0].candidate.mutation_id)
    );
    assert_eq!(
        frozen[1].candidate.payload["by_device"][deps.device_id.current_device_id().to_string()]
            .as_u64(),
        Some(2)
    );
    let directory = tempfile::tempdir().unwrap();
    let rebuilt = SqliteDatabase::open(directory.path().join("rebuilt.sqlite")).unwrap();
    LibraryRepository::upsert(
        &rebuilt,
        &LibraryRoot::new(stack.root, stack.library_dir.clone(), true, true),
    )
    .unwrap();
    for _ in 0..2 {
        rebuilt
            .project_library_commits(stack.root, &replay)
            .unwrap();
        assert_eq!(count(&rebuilt, song), 2);
        let (sessions, queued, mutations): (u64, u64, u64) = rebuilt.with_reader(|c| {
            c.query_row("SELECT (SELECT COUNT(*) FROM recorded_play_sessions),(SELECT COUNT(*) FROM sync_outbox),(SELECT COUNT(*) FROM sync_mutations)", [], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).map_err(storage)
        }).unwrap();
        assert_eq!((sessions, queued, mutations), (2, 0, 0));
    }
}

#[test]
fn library_durability_playback_session_cannot_be_reused_for_another_song() {
    let stack = ScanStack::new();
    let (_, control, deps) = configured_roots(&stack);
    let first_song = favorite::seed_song(&stack, &control, &deps);
    let other_song = SongId::new();
    let Some(PortableRecord::Song(mut other)) = control
        .read_record(
            stack.root,
            crate::domain::library::RecordKind::Song,
            &first_song.to_string(),
        )
        .unwrap()
    else {
        panic!("saved song expected");
    };
    other.song_uuid = other_song.as_uuid();
    other.media_path =
        crate::domain::library::LibraryRelativePath::new("media/other.flac").unwrap();
    control
        .durability()
        .unwrap()
        .commit(
            stack.root,
            &crate::domain::library::durability::LibraryBatch::new(
                uuid::Uuid::new_v4(),
                control
                    .read_manifest(stack.root)
                    .unwrap()
                    .unwrap()
                    .library_id,
                crate::domain::library::durability::ChangeOrigin::Scan,
                vec![crate::domain::library::PortableRecordEnvelope::new(
                    PortableRecord::Song(other),
                    None,
                )
                .unwrap()],
            ),
        )
        .unwrap();
    let replay = control.durability().unwrap().recover(stack.root).unwrap();
    stack
        .database
        .project_library_commits(stack.root, &replay)
        .unwrap();
    let session = PlaybackSessionId::new();
    assert!(record(&stack, &control, &deps, session, first_song).unwrap());
    let before = control.durability().unwrap().recover(stack.root).unwrap();
    let result = record(&stack, &control, &deps, session, other_song);
    assert!(
        matches!(result, Err(crate::error::Error::Conflict { .. })),
        "{result:?}"
    );
    assert_eq!(count(stack.database.as_ref(), first_song), 1);
    assert_eq!(count(stack.database.as_ref(), other_song), 0);
    assert_eq!(
        control.durability().unwrap().recover(stack.root).unwrap(),
        before
    );
}
