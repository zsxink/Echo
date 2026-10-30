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
        1_700_000_000_000,
    )
}

fn count(database: &SqliteDatabase, song: SongId) -> u64 {
    SongRepository::by_id(database, song)
        .unwrap()
        .unwrap()
        .play_count()
        .as_u64()
}

#[allow(clippy::too_many_lines)] // One end-to-end pending-bucket scenario; splitting would hide its protocol assertions.
#[test]
fn library_durability_new_author_playback_preserves_old_pending_bucket_without_claiming_it() {
    use crate::domain::library::durability::{ChangeOrigin, LibraryBatch};
    use crate::domain::library::{HybridLogicalClock, PlayStatsRecord, PortableRecordEnvelope};
    use crate::domain::sync::{
        AuthorityId, CommitSeq, ConfirmedObject, Epoch, LibraryId as SyncId, ObjectKey,
        ObjectVersion, OriginCursor, SyncBinding,
    };
    let stack = ScanStack::new();
    let (_, control, deps) = configured_roots(&stack);
    // The pending play bucket is a LAN protocol fact: bind the root so the
    // listen freezes its candidate exactly as it does in a synced library.
    stack
        .database
        .bind_sync_library(
            SyncBinding {
                library_id: SyncId::new(),
                authority_id: AuthorityId::new(),
                epoch: Epoch::INITIAL,
            },
            true,
            stack.root,
        )
        .unwrap();
    let song = favorite::seed_song(&stack, &control, &deps);
    let old_author = uuid::Uuid::new_v4();
    let new_author = deps.device_id.current_device_id();
    let manifest = control.read_manifest(stack.root).unwrap().unwrap();
    let before = PlayStatsRecord {
        song_uuid: song.as_uuid(),
        revision: Revision::INITIAL,
        updated_by_device_id: crate::domain::library::DeviceId::from_uuid(old_author),
        hlc: HybridLogicalClock::default(),
        by_device: std::iter::once((old_author, 7)).collect(),
    };
    control
        .durability()
        .unwrap()
        .commit(
            stack.root,
            &LibraryBatch::new(
                uuid::Uuid::new_v4(),
                manifest.library_id,
                ChangeOrigin::Recovery,
                vec![PortableRecordEnvelope::new(PortableRecord::PlayStats(before), None).unwrap()],
            ),
        )
        .unwrap();
    stack
        .database
        .project_library_commits(
            stack.root,
            &control.durability().unwrap().recover(stack.root).unwrap(),
        )
        .unwrap();
    let session = PlaybackSessionId::new();
    assert!(record(&stack, &control, &deps, session, song).unwrap());
    assert!(!record(&stack, &control, &deps, session, song).unwrap());
    let replay = control.durability().unwrap().recover(stack.root).unwrap();
    let state = replay
        .commits
        .last()
        .unwrap()
        .batch
        .state
        .iter()
        .find(|state| state.kind == StateKind::Pending)
        .unwrap();
    let frozen: FrozenMutation = serde_json::from_value(state.payload["frozen"].clone()).unwrap();
    assert_eq!(
        frozen.candidate.payload["by_device"][old_author.to_string()],
        serde_json::Value::Null
    );
    assert_eq!(
        frozen.candidate.payload["by_device"][new_author.to_string()],
        1
    );
    let local = stack.database.device_play_counts(song).unwrap();
    assert_eq!(local[&old_author], 7);
    assert_eq!(local[&new_author.as_uuid()], 1);
    let current = ConfirmedObject {
        object: ObjectKey::PlayStats(song),
        version: ObjectVersion::from_commit_seq(CommitSeq::from_u64(5)),
        origin_device_id: crate::domain::sync::DeviceId::from_uuid(old_author),
        payload: serde_json::to_value(PlayStatsRecord {
            song_uuid: song.as_uuid(),
            revision: Revision::INITIAL,
            updated_by_device_id: crate::domain::library::DeviceId::from_uuid(old_author),
            hlc: HybridLogicalClock::default(),
            by_device: std::iter::once((old_author, 5)).collect(),
        })
        .unwrap(),
    };
    let cursor = frozen
        .candidate
        .previous_mutation_id
        .map(|last_mutation_id| OriginCursor {
            origin_device_id: frozen.candidate.origin_device_id,
            origin_seq: frozen.candidate.origin_seq - 1,
            last_mutation_id,
        });
    let confirmed = crate::domain::sync::confirm_candidate_at(
        Some(&current),
        cursor,
        &frozen.candidate,
        CommitSeq::from_u64(6),
        frozen.frozen_at_ms,
        0,
    )
    .unwrap();
    assert_eq!(
        confirmed.object.payload["by_device"][old_author.to_string()],
        5
    );
    assert_eq!(
        confirmed.object.payload["by_device"][new_author.to_string()],
        1
    );
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
    use crate::domain::sync::{AuthorityId, Epoch, LibraryId as SyncId, SyncBinding};
    let stack = ScanStack::new();
    let (_, control, deps) = configured_roots(&stack);
    // A synced library records each qualified listen as a pending mutation; the
    // session itself must still never leak into it (SYN-R08-S01).
    stack
        .database
        .bind_sync_library(
            SyncBinding {
                library_id: SyncId::new(),
                authority_id: AuthorityId::new(),
                epoch: Epoch::INITIAL,
            },
            true,
            stack.root,
        )
        .unwrap();
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
fn library_durability_playback_unwritable_library_leaves_no_database_only_increment() {
    let stack = ScanStack::new();
    let (_, control, deps) = configured_roots(&stack);
    let song = favorite::seed_song(&stack, &control, &deps);
    let rejecting = Arc::new(control.as_ref().clone().with_commit_hook(|phase, _| {
        if phase == "prepared" {
            return Err(crate::error::Error::unavailable(
                "playback directory",
                "injected unwritable library resource",
            ));
        }
        Ok(())
    }));
    stack.database.configure_library_control(rejecting.clone());
    let session = PlaybackSessionId::new();
    for _ in 0..2 {
        assert!(record(&stack, rejecting.as_ref(), &deps, session, song).is_err());
        assert_eq!(
            count(stack.database.as_ref(), song),
            0,
            "an unwritable library must never leave a projected play"
        );
        assert!(stack.database.device_play_counts(song).unwrap().is_empty());
        let (sessions, queued, mutations): (u64, u64, u64) = stack
            .database
            .with_reader(|c| {
                c.query_row(
                    "SELECT (SELECT COUNT(*) FROM recorded_play_sessions),\
                     (SELECT COUNT(*) FROM sync_outbox),\
                     (SELECT COUNT(*) FROM sync_mutations)",
                    [],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .map_err(storage)
            })
            .unwrap();
        assert_eq!(
            (sessions, queued, mutations),
            (0, 0, 0),
            "no private session, outbox snapshot or frozen mutation may exist"
        );
        assert!(control
            .durability()
            .unwrap()
            .recover(stack.root)
            .unwrap()
            .states()
            .all(|state| state.kind != StateKind::PlaybackEvent));
    }
    // The same intent still commits once the resource becomes writable, and
    // repeated attempts keep counting exactly one qualified play.
    stack.database.configure_library_control(control.clone());
    assert!(record(&stack, &control, &deps, session, song).unwrap());
    assert!(!record(&stack, &control, &deps, session, song).unwrap());
    assert_eq!(count(stack.database.as_ref(), song), 1);
    assert_eq!(
        stack.database.device_play_counts(song).unwrap(),
        std::collections::BTreeMap::from([(deps.device_id.current_device_id().as_uuid(), 1)])
    );
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
