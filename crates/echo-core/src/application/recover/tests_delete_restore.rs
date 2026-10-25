//! Delete/restore staging crash recovery, foreign occupancy holds and expired-delete trash handoff.

#[allow(clippy::wildcard_imports)]
use super::tests::*;
use crate::application::ports::ControlPlanePort;

fn assert_delete_intent_state(
    fixture: &ScanFixture,
    operation: OperationId,
    expected: crate::domain::sync::DeleteIntentState,
) {
    let replay = fixture
        .control
        .durability()
        .unwrap()
        .recover(fixture.root)
        .unwrap();
    let state = replay
        .states()
        .filter(|state| {
            state.kind == crate::domain::library::durability::StateKind::DeleteIntent
                && state.id == operation.as_uuid()
        })
        .last()
        .expect("delete intent remains in the directory");
    let intent: crate::domain::sync::LocalDeleteIntent =
        serde_json::from_value(state.payload.clone()).unwrap();
    assert_eq!(intent.state, expected);
}

/// Read a file from the staging path persisted for one journal item. The
/// basename is now the original media filename; using the journal path
/// keeps these tests compatible with older `audio`/`lyrics` entries too.
fn trash_path(fixture: &ScanFixture, operation: OperationId, key: &str) -> std::path::PathBuf {
    fixture
        .database
        .items(operation)
        .expect("delete items")
        .into_iter()
        .find(|item| item.item_key == key)
        .and_then(|item| item.staging_path)
        .map(|path| {
            fixture
                .fs
                .root_path(fixture.root)
                .expect("root")
                .join(path.normalized())
        })
        .expect("journal staging path")
}

fn read_trash(fixture: &ScanFixture, operation: OperationId, key: &str) -> Option<Vec<u8>> {
    std::fs::read(trash_path(fixture, operation, key)).ok()
}

/// The same-basename `.lrc` sidecar path beside an audio path.
fn lrc_sibling(audio: &RelativeMediaPath) -> RelativeMediaPath {
    let normalized = audio.normalized();
    let without_extension = match normalized.rsplit_once('.') {
        Some((stem, _)) => stem,
        None => normalized,
    };
    RelativeMediaPath::new(&format!("{without_extension}.lrc"))
        .expect("derived .lrc sibling stays valid")
}

/// Seed one library song (favorite + play stats, optional `.lrc` sidecar)
/// so undo-recovery can assert the preserved relationships (design §9).
fn seed_delete_song(fixture: &ScanFixture, path: &str, audio: &[u8], lrc: Option<&[u8]>) -> SongId {
    fixture.write_file(path, audio);
    fixture.set_audio(path, "晴天", 269_000);
    let audio_path = fixture.path(path);
    let mut song = crate::domain::entities::Song::new(
        SongId::new(),
        fixture.root,
        audio_path.clone(),
        crate::domain::ids::Revision::INITIAL,
    );
    song.apply_scan_facts(
        fixture.deps.hasher.hash_of_bytes(audio),
        audio.len() as u64,
        1,
        crate::domain::media::AudioFormat::Flac,
    );
    song.set_favorite(true);
    song.record_play();
    song.record_play();
    SongRepository::upsert(&fixture.database, &song).unwrap();
    if let Some(lrc_bytes) = lrc {
        let lrc_path = lrc_sibling(&audio_path);
        fixture.write_file(lrc_path.display(), lrc_bytes);
        let subject = song.id();
        let candidate = crate::domain::entities::LyricsCandidate::new(
            crate::domain::entities::LyricsSource::Sidecar,
            vec![],
            true,
        );
        fixture
            .deps
            .uow
            .with_tx(Box::new(move |tx: &mut dyn TxAccess| {
                tx.set_lyrics_candidate(subject, &candidate)
            }))
            .unwrap();
    }
    song.id()
}

#[test]
fn crash_after_delete_stage_rename_recovers_to_unique_hidden() {
    // Crash after the stage rename happened but BEFORE the `StageApplied`
    // journal write: the per-resource matrix must normalize `StagePending →
    // StageApplied` (原缺失而暂存匹配) and then finish the hide.
    let fixture = ScanFixture::new();
    let ctrl = Arc::new(Controller::default());
    let song = seed_delete_song(
        &fixture,
        "media/歌手/周杰伦 - 晴天.flac",
        b"audio-bytes",
        None,
    );
    let deps = crash_deps(&fixture, &ctrl);
    ctrl.arm(Point {
        site: Site::State("stage_applied"),
        phase: Phase::Before,
    });
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        DeleteSongs::new(&deps).delete(fixture.root, song).unwrap()
    }));
    assert!(
        outcome.is_err(),
        "injection at stage_applied must crash the delete"
    );
    assert!(ctrl.fired());

    let operation = single_operation(&fixture);
    // The rename landed before the crash: original gone, trash has the
    // exact bytes.
    assert_eq!(read_file(&fixture, "media/歌手/周杰伦 - 晴天.flac"), None);
    assert_eq!(
        read_trash(&fixture, operation, "audio"),
        Some(b"audio-bytes".to_vec())
    );

    RecoverOperations::new(&fixture.deps)
        .run(fixture.root)
        .unwrap();

    let after = SongRepository::by_id(&fixture.database, song)
        .unwrap()
        .unwrap();
    assert_eq!(
        after.availability(),
        SongAvailability::PendingDelete,
        "recovery completes the hide"
    );
    let items = fixture.database.items(operation).unwrap();
    assert!(
        items
            .iter()
            .all(|item| item.state == OperationState::HiddenInDatabase),
        "unique hidden state: {items:?}"
    );
    assert_eq!(
        read_file(&fixture, "media/歌手/周杰伦 - 晴天.flac"),
        None,
        "audio stays staged"
    );
    assert_eq!(
        read_trash(&fixture, operation, "audio"),
        Some(b"audio-bytes".to_vec())
    );
    assert!(
        fixture.database.undo_deadline(operation).unwrap().is_some(),
        "a fresh undo deadline is durable"
    );
    // Second recovery is an idempotent no-op on the hidden state.
    RecoverOperations::new(&fixture.deps)
        .run(fixture.root)
        .unwrap();
    assert_eq!(
        SongRepository::by_id(&fixture.database, song)
            .unwrap()
            .unwrap()
            .availability(),
        SongAvailability::PendingDelete
    );
}

#[test]
fn crash_at_the_delete_stage_rename_re_stages_then_hides() {
    // Crash at the rename itself (before it runs): original intact, trash
    // empty. Recovery re-stages from the persisted source, then hides.
    let fixture = ScanFixture::new();
    let ctrl = Arc::new(Controller::default());
    let song = seed_delete_song(
        &fixture,
        "media/歌手/周杰伦 - 晴天.flac",
        b"audio-bytes",
        None,
    );
    let deps = crash_deps(&fixture, &ctrl);
    ctrl.arm(Point {
        site: Site::TrashMove,
        phase: Phase::Before,
    });
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        DeleteSongs::new(&deps).delete(fixture.root, song).unwrap()
    }));
    assert!(
        outcome.is_err(),
        "injection at the stage rename must crash the delete"
    );
    assert!(ctrl.fired());

    let operation = single_operation(&fixture);
    assert_eq!(
        read_file(&fixture, "media/歌手/周杰伦 - 晴天.flac"),
        Some(b"audio-bytes".to_vec()),
        "the rename never ran"
    );
    assert!(
        read_trash(&fixture, operation, "audio").is_none(),
        "trash empty"
    );

    RecoverOperations::new(&fixture.deps)
        .run(fixture.root)
        .unwrap();

    assert_eq!(
        SongRepository::by_id(&fixture.database, song)
            .unwrap()
            .unwrap()
            .availability(),
        SongAvailability::PendingDelete
    );
    assert_eq!(read_file(&fixture, "media/歌手/周杰伦 - 晴天.flac"), None);
    assert_eq!(
        read_trash(&fixture, operation, "audio"),
        Some(b"audio-bytes".to_vec())
    );
    let items = fixture.database.items(operation).unwrap();
    assert!(items
        .iter()
        .all(|item| item.state == OperationState::HiddenInDatabase));
}

#[test]
fn crash_at_the_delete_hide_commit_then_recovery_hides_audio_and_lrc() {
    // Both resources fully staged (`StageApplied`), crash at the hide DB
    // commit: recovery completes the one-transaction hide for both items.
    let fixture = ScanFixture::new();
    let ctrl = Arc::new(Controller::default());
    let song = seed_delete_song(
        &fixture,
        "media/歌手/周杰伦 - 晴天.flac",
        b"audio-bytes",
        Some(b"lrc-bytes"),
    );
    let deps = crash_deps(&fixture, &ctrl);
    ctrl.arm(Point {
        site: Site::Commit,
        phase: Phase::Before,
    });
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        DeleteSongs::new(&deps).delete(fixture.root, song).unwrap()
    }));
    assert!(
        outcome.is_err(),
        "injection at the hide commit must crash the delete"
    );
    assert!(ctrl.fired());

    let operation = delete_operation(&fixture);
    let items = fixture.database.items(operation).unwrap();
    assert_eq!(items.len(), 2, "audio + lyrics both staged");
    assert!(items
        .iter()
        .all(|item| item.state == OperationState::StageApplied));
    assert_eq!(
        SongRepository::by_id(&fixture.database, song)
            .unwrap()
            .unwrap()
            .availability(),
        SongAvailability::Available,
        "the hide never committed"
    );

    RecoverOperations::new(&fixture.deps)
        .run(fixture.root)
        .unwrap();

    assert_eq!(
        SongRepository::by_id(&fixture.database, song)
            .unwrap()
            .unwrap()
            .availability(),
        SongAvailability::PendingDelete
    );
    let items = fixture.database.items(operation).unwrap();
    assert!(items
        .iter()
        .all(|item| item.state == OperationState::HiddenInDatabase));
    assert_eq!(
        read_trash(&fixture, operation, "audio"),
        Some(b"audio-bytes".to_vec())
    );
    assert_eq!(
        read_trash(&fixture, operation, "lyrics"),
        Some(b"lrc-bytes".to_vec())
    );
}

#[test]
fn crash_during_restore_after_rename_recovers_song_available() {
    // Mid-undo crash AFTER the file came home but BEFORE `RestoreApplied`:
    // the matrix normalizes `RestorePending → Restored` from the matching
    // original, and the song is brought back to Available (keeping UUID,
    // favorite, stats — and the playlist position is untouched by delete).
    let fixture = ScanFixture::new();
    let song = seed_delete_song(
        &fixture,
        "media/歌手/周杰伦 - 晴天.flac",
        b"audio-bytes",
        None,
    );
    let playlist = crate::domain::ids::PlaylistId::new();
    fixture
        .database
        .create(playlist, fixture.root, "最爱")
        .unwrap();
    fixture.database.add_member(playlist, song, 7).unwrap();
    let outcome = DeleteSongs::new(&fixture.deps)
        .delete(fixture.root, song)
        .unwrap();
    let operation = outcome.operation;

    let ctrl = Arc::new(Controller::default());
    let deps = crash_deps(&fixture, &ctrl);
    ctrl.arm(Point {
        site: Site::State("restore_applied"),
        phase: Phase::Before,
    });
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        RestoreDeletedOperation::new(&deps)
            .restore(fixture.root, operation)
            .unwrap()
    }));
    assert!(
        res.is_err(),
        "injection at restore_applied must crash the undo"
    );
    assert!(ctrl.fired());

    // The rename already landed: file home, item still `RestorePending`.
    assert_eq!(
        read_file(&fixture, "media/歌手/周杰伦 - 晴天.flac"),
        Some(b"audio-bytes".to_vec())
    );
    let items = fixture.database.items(operation).unwrap();
    assert_eq!(items[0].state, OperationState::RestorePending);
    assert_eq!(
        SongRepository::by_id(&fixture.database, song)
            .unwrap()
            .unwrap()
            .availability(),
        SongAvailability::PendingDelete,
        "the song is still hidden mid-undo"
    );

    RecoverOperations::new(&fixture.deps)
        .run(fixture.root)
        .unwrap();

    let after = SongRepository::by_id(&fixture.database, song)
        .unwrap()
        .unwrap();
    assert_eq!(
        after.availability(),
        SongAvailability::Available,
        "undo completes"
    );
    assert_eq!(after.id(), song, "UUID preserved");
    assert!(after.favorite(), "favorite preserved");
    assert_eq!(after.play_count().as_u64(), 2, "stats preserved");
    assert_eq!(
        fixture
            .database
            .members(playlist)
            .unwrap()
            .first()
            .map(crate::domain::entities::PlaylistMember::position),
        Some(7),
        "playlist position preserved"
    );
    let items = fixture.database.items(operation).unwrap();
    assert!(items
        .iter()
        .all(|item| item.state == OperationState::Restored));
    assert_eq!(fixture.database.released_claims(), vec![operation]);
    assert_delete_intent_state(
        &fixture,
        operation,
        crate::domain::sync::DeleteIntentState::Cancelled,
    );
    // Second recovery is an idempotent no-op (claims not re-released).
    RecoverOperations::new(&fixture.deps)
        .run(fixture.root)
        .unwrap();
    assert_eq!(fixture.database.released_claims(), vec![operation]);
}

#[test]
fn crash_at_the_restore_rename_recovers_song_available() {
    // Mid-undo crash BEFORE the file comes home: the staged copy still sits
    // in the trash slot. Recovery retries the restore into the original
    // path, verifies the hash, and brings the song back to Available.
    let fixture = ScanFixture::new();
    let song = seed_delete_song(
        &fixture,
        "media/歌手/周杰伦 - 晴天.flac",
        b"audio-bytes",
        None,
    );
    let outcome = DeleteSongs::new(&fixture.deps)
        .delete(fixture.root, song)
        .unwrap();
    let operation = outcome.operation;

    let ctrl = Arc::new(Controller::default());
    let deps = crash_deps(&fixture, &ctrl);
    ctrl.arm(Point {
        site: Site::TrashMove,
        phase: Phase::Before,
    });
    let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        RestoreDeletedOperation::new(&deps)
            .restore(fixture.root, operation)
            .unwrap()
    }));
    assert!(
        res.is_err(),
        "injection at the restore rename must crash the undo"
    );
    assert!(ctrl.fired());

    assert_eq!(read_file(&fixture, "media/歌手/周杰伦 - 晴天.flac"), None);
    assert_eq!(
        read_trash(&fixture, operation, "audio"),
        Some(b"audio-bytes".to_vec())
    );

    RecoverOperations::new(&fixture.deps)
        .run(fixture.root)
        .unwrap();

    assert_eq!(
        SongRepository::by_id(&fixture.database, song)
            .unwrap()
            .unwrap()
            .availability(),
        SongAvailability::Available
    );
    assert_eq!(
        read_file(&fixture, "media/歌手/周杰伦 - 晴天.flac"),
        Some(b"audio-bytes".to_vec())
    );
    let items = fixture.database.items(operation).unwrap();
    assert!(items
        .iter()
        .all(|item| item.state == OperationState::Restored));
    assert_eq!(fixture.database.released_claims(), vec![operation]);
}

#[test]
fn foreign_occupancy_at_recover_restore_pending_holds_the_item_not_the_whole_run() {
    // A mid-undo crash leaves an item `RestorePending` with its staged copy
    // intact, and the original target later becomes re-occupied by foreign
    // content with NO safe numbered path available (every ` (n)` candidate
    // foreign too): recovery must HOLD that one item (`FailedRecoverable`),
    // never abort the whole root recovery, and still recover every unrelated
    // operation in the same pass (design §9: 两处证据矛盾时逐 item held,
    // 不删除任一文件; 一项冲突不得阻塞根目录内所有无关操作的恢复). The foreign
    // content is never replaced and the held claim stays reserved.
    let fixture = ScanFixture::new();
    let song_a = seed_delete_song(&fixture, "media/歌手/周杰伦 - 晴天.flac", b"audio-a", None);
    let song_b = seed_delete_song(
        &fixture,
        "media/歌手/林俊杰 - 不为谁而作的歌.flac",
        b"audio-b",
        None,
    );
    let op_a = DeleteSongs::new(&fixture.deps)
        .delete(fixture.root, song_a)
        .unwrap()
        .operation;
    let op_b = DeleteSongs::new(&fixture.deps)
        .delete(fixture.root, song_b)
        .unwrap()
        .operation;

    let ctrl = Arc::new(Controller::default());
    let deps = crash_deps(&fixture, &ctrl);

    // Crash A's undo before the restore rename: staged copy intact, item
    // `RestorePending`, original absent.
    ctrl.arm(Point {
        site: Site::TrashMove,
        phase: Phase::Before,
    });
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            RestoreDeletedOperation::new(&deps)
                .restore(fixture.root, op_a)
                .unwrap()
        }))
        .is_err(),
        "injection at A's restore rename must crash the undo"
    );
    assert!(ctrl.fired());

    // Crash B's undo after the restore rename lands: original matches, item
    // `RestorePending` — it only needs normalization to `Restored`.
    ctrl.arm(Point {
        site: Site::State("restore_applied"),
        phase: Phase::Before,
    });
    assert!(
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            RestoreDeletedOperation::new(&deps)
                .restore(fixture.root, op_b)
                .unwrap()
        }))
        .is_err(),
        "injection at B's restore_applied must crash the undo"
    );
    assert!(ctrl.fired());

    // Re-occupy A's original with foreign content AND exhaust every safe
    // numbered candidate so `safe_restore_target` conflicts for A.
    fixture.write_file("media/歌手/周杰伦 - 晴天.flac", b"foreign-audio");
    for suffix in 1..=100_u32 {
        fixture.write_file(
            &format!("media/歌手/周杰伦 - 晴天 ({suffix}).flac"),
            b"foreign-numbered",
        );
    }

    // Recovery must NOT abort even though A is an unsupported conflict; B
    // (an unrelated operation) still completes in the same root pass.
    RecoverOperations::new(&fixture.deps)
        .run(fixture.root)
        .unwrap();

    // A is held per-item: staged copy and foreign occupant untouched, claim
    // never released.
    let a_items = fixture.database.items(op_a).unwrap();
    assert!(
        a_items
            .iter()
            .all(|item| item.state == OperationState::FailedRecoverable),
        "A is held, not aborted: {a_items:?}"
    );
    assert_eq!(
        read_file(&fixture, "media/歌手/周杰伦 - 晴天.flac"),
        Some(b"foreign-audio".to_vec()),
        "the foreign occupant at A's original is never replaced"
    );
    assert_eq!(
        read_trash(&fixture, op_a, "audio"),
        Some(b"audio-a".to_vec()),
        "A's staged copy is preserved for the held item"
    );
    assert!(
        !fixture.database.released_claims().contains(&op_a),
        "a held conflict never releases its claim"
    );

    // B (unrelated operation) still recovers to Restored and releases.
    let b_after = SongRepository::by_id(&fixture.database, song_b)
        .unwrap()
        .unwrap();
    assert_eq!(b_after.availability(), SongAvailability::Available);
    let b_items = fixture.database.items(op_b).unwrap();
    assert!(
        b_items
            .iter()
            .all(|item| item.state == OperationState::Restored),
        "B recovers to Restored: {b_items:?}"
    );
    assert_eq!(
        read_file(&fixture, "media/歌手/林俊杰 - 不为谁而作的歌.flac"),
        Some(b"audio-b".to_vec())
    );
    assert!(
        fixture.database.released_claims().contains(&op_b),
        "the unrelated restored operation releases its claim"
    );
}

#[test]
fn contradictory_stage_evidence_holds_and_deletes_nothing() {
    // Both the original and the trash slot now hold content that
    // contradicts the journal's expected hash: the matrix must HOLD
    // (`FailedRecoverable`), never delete either file, leave the song
    // available and keep the claim (design §9: 两处证据矛盾时不删除任一文件).
    let fixture = ScanFixture::new();
    let song = seed_delete_song(
        &fixture,
        "media/歌手/周杰伦 - 晴天.flac",
        b"audio-bytes",
        None,
    );
    let ctrl = Arc::new(Controller::default());
    let deps = crash_deps(&fixture, &ctrl);
    ctrl.arm(Point {
        site: Site::State("stage_applied"),
        phase: Phase::Before,
    });
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        DeleteSongs::new(&deps).delete(fixture.root, song).unwrap()
    }));
    assert!(outcome.is_err());
    assert!(ctrl.fired());
    let operation = single_operation(&fixture);

    fixture.write_file("media/歌手/周杰伦 - 晴天.flac", b"foreign-original");
    std::fs::write(trash_path(&fixture, operation, "audio"), b"foreign-trash")
        .expect("write contradictory trash");

    RecoverOperations::new(&fixture.deps)
        .run(fixture.root)
        .unwrap();

    assert_eq!(
        read_file(&fixture, "media/歌手/周杰伦 - 晴天.flac"),
        Some(b"foreign-original".to_vec()),
        "the original file is never deleted"
    );
    assert!(
        read_trash(&fixture, operation, "audio").is_some(),
        "the trash file is never deleted"
    );
    let items = fixture.database.items(operation).unwrap();
    assert!(
        items
            .iter()
            .all(|item| item.state == OperationState::FailedRecoverable),
        "contradictory evidence is held: {items:?}"
    );
    assert_eq!(
        SongRepository::by_id(&fixture.database, song)
            .unwrap()
            .unwrap()
            .availability(),
        SongAvailability::Available,
        "the song is not hidden"
    );
    assert!(
        fixture.database.released_claims().is_empty(),
        "a held conflict never releases the claim"
    );
}

#[test]
fn expired_hidden_delete_is_handed_to_trash_and_untouched_by_restore() {
    // An operation whose undo window expired is handed to task 5.8's trash
    // flow (`HiddenInDatabase → TrashPending`); recovery must NOT restore
    // the song or the file, and `RestoreDeletedOperation` refuses it.
    let fixture = ScanFixture::new();
    let song = seed_delete_song(
        &fixture,
        "media/歌手/周杰伦 - 晴天.flac",
        b"audio-bytes",
        None,
    );
    let outcome = DeleteSongs::new(&fixture.deps)
        .delete(fixture.root, song)
        .unwrap();
    let operation = outcome.operation;
    fixture.clock.advance_ms(20_000);

    let report = RecoverOperations::new(&fixture.deps)
        .run(fixture.root)
        .unwrap();
    let recovered = report
        .touched
        .iter()
        .find(|op| op.operation == operation)
        .expect("the expired delete is touched");
    assert!(recovered.handed_off, "handed to 5.8's trash flow");
    let items = fixture.database.items(operation).unwrap();
    assert!(
        items
            .iter()
            .all(|item| item.state == OperationState::TrashPending),
        "items advanced to TrashPending: {items:?}"
    );
    assert_eq!(
        SongRepository::by_id(&fixture.database, song)
            .unwrap()
            .unwrap()
            .availability(),
        SongAvailability::PendingDelete,
        "the song stays hidden (trash finalize is 5.8's work)"
    );
    // The undo is now refused.
    let err = RestoreDeletedOperation::new(&fixture.deps)
        .restore(fixture.root, operation)
        .unwrap_err();
    assert_eq!(err.code(), "conflict", "expired undo refused");
}

#[test]
fn expired_multi_resource_handoff_is_atomic_when_persisting_trash_pending() {
    let fixture = ScanFixture::new();
    let song = seed_delete_song(
        &fixture,
        "media/artist/song.flac",
        b"audio-bytes",
        Some(b"lyrics-bytes"),
    );
    let operation = DeleteSongs::new(&fixture.deps)
        .delete(fixture.root, song)
        .expect("delete")
        .operation;
    fixture.clock.advance_ms(20_000);
    fixture.database.set_fail_commit(true);

    assert!(RecoverOperations::new(&fixture.deps)
        .run(fixture.root)
        .is_err());
    assert!(fixture
        .database
        .items(operation)
        .unwrap()
        .iter()
        .all(|item| item.state == OperationState::HiddenInDatabase));

    fixture.database.set_fail_commit(false);
    let report = RecoverOperations::new(&fixture.deps)
        .run(fixture.root)
        .expect("retry handoff");
    assert!(report.touched.iter().any(|entry| entry.handed_off));
    assert!(fixture
        .database
        .items(operation)
        .unwrap()
        .iter()
        .all(|item| item.state == OperationState::TrashPending));
}
