use super::*;
use crate::application::ports::{OperationJournalRepository, PlaylistRepository, SongRepository};
use crate::application::testing::ScanFixture;
use crate::domain::entities::{PlaylistMember, SongAvailability};

/// Seed one library song at `path` with the given bytes and (optionally) a
/// same-basename `.lrc` beside it, plus favorite/play-count/playlist state.
fn seed_song(fixture: &ScanFixture, path: &str, audio: &[u8], lrc: Option<&[u8]>) -> SongId {
    fixture.write_file(path, audio);
    fixture.set_audio(path, "晴天", 269_000);
    let audio_path = fixture.path(path);
    let mut song = crate::domain::entities::Song::new(
        crate::domain::ids::SongId::new(),
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
        fixture.write_file(derive_lrc_path(&audio_path).display(), lrc_bytes);
        // A persisted sidecar lyrics candidate, like an import would leave.
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

fn playlist_position(fixture: &ScanFixture, playlist: crate::domain::ids::PlaylistId) -> u64 {
    fixture
        .database
        .members(playlist)
        .unwrap()
        .first()
        .map_or(u64::MAX, PlaylistMember::position)
}

#[test]
fn held_remote_delete_blocks_undo_before_moving_staged_resources() {
    let fixture = ScanFixture::new();
    let song = seed_song(
        &fixture,
        "media/held.flac",
        b"retained-audio",
        Some(b"retained-lyrics"),
    );
    let outcome = DeleteSongs::new(&fixture.deps)
        .delete(fixture.root, song)
        .unwrap();
    let items = fixture.deps.journal.items(outcome.operation).unwrap();
    for item in &items {
        fixture
            .deps
            .journal
            .upsert_item(
                outcome.operation,
                OperationItem {
                    state: OperationState::FailedRecoverable,
                    ..item.clone()
                },
            )
            .unwrap();
    }
    assert!(RestoreDeletedOperation::new(&fixture.deps)
        .restore(fixture.root, outcome.operation)
        .is_err());
    for item in &items {
        assert!(fixture
            .deps
            .fs
            .path_exists(fixture.root, item.staging_path.as_ref().unwrap())
            .unwrap());
        assert!(!fixture
            .deps
            .fs
            .path_exists(fixture.root, &item.target_path)
            .unwrap());
    }
    assert_eq!(
        SongRepository::by_id(&fixture.database, song)
            .unwrap()
            .unwrap()
            .availability(),
        SongAvailability::PendingDelete
    );
}

#[test]
fn delete_stages_audio_and_lrc_into_trash_and_hides_the_song() {
    let fixture = ScanFixture::new();
    let song = seed_song(
        &fixture,
        "media/歌手/周杰伦 - 晴天.flac",
        b"audio-bytes",
        Some(b"lrc-bytes"),
    );
    let audio_path = fixture.path("歌手/周杰伦 - 晴天.flac");
    let lrc_path = derive_lrc_path(&audio_path);

    let outcome = DeleteSongs::new(&fixture.deps)
        .delete(fixture.root, song)
        .unwrap();
    assert!(outcome.staged_lyrics);

    // Both files moved into the owned trash slot under the operation.
    let base = fixture.fs.root_path(fixture.root).expect("root");
    assert!(
        !base.join("歌手/周杰伦 - 晴天.flac").exists(),
        "audio left the library"
    );
    assert!(
        !base.join("歌手/周杰伦 - 晴天.lrc").exists(),
        "lyrics left the library"
    );
    let trash_dir = base
        .join(crate::domain::library::STAGING_ROOT)
        .join("trash")
        .join(outcome.operation.as_uuid().simple().to_string());
    assert_eq!(
        std::fs::read(trash_dir.join("周杰伦 - 晴天.flac")).unwrap(),
        b"audio-bytes"
    );
    assert_eq!(
        std::fs::read(trash_dir.join("周杰伦 - 晴天.lrc")).unwrap(),
        b"lrc-bytes"
    );

    // The song is hidden (pending-delete) but every relationship survives.
    let after = SongRepository::by_id(&fixture.database, song)
        .unwrap()
        .unwrap();
    assert_eq!(after.availability(), SongAvailability::PendingDelete);
    assert_eq!(after.id(), song, "UUID preserved");
    assert!(after.favorite(), "favorite preserved");
    assert_eq!(after.play_count().as_u64(), 2, "play stats preserved");
    let lrc = fixture.database.lyrics_of(song);
    assert_eq!(lrc.len(), 1, "lyrics candidate preserved");

    // The undo deadline was persisted atomically with the hide.
    let deadline = fixture
        .database
        .undo_deadline(outcome.operation)
        .unwrap()
        .unwrap();
    assert_eq!(deadline, wall_ms(&fixture.clock).unwrap() + UNDO_WINDOW_MS);
    let items = fixture.database.items(outcome.operation).unwrap();
    assert_eq!(items.len(), 2);
    assert!(items
        .iter()
        .all(|item| item.state == OperationState::HiddenInDatabase));
    let _ = lrc_path;
}

#[test]
fn restore_undo_preserves_uuid_favorite_stats_and_playlist_position() {
    let fixture = ScanFixture::new();
    let song = seed_song(
        &fixture,
        "歌手/周杰伦 - 晴天.flac",
        b"audio-bytes",
        Some(b"lrc-bytes"),
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
    assert_eq!(
        playlist_position(&fixture, playlist),
        7,
        "member kept during pending-delete"
    );

    let restored = RestoreDeletedOperation::new(&fixture.deps)
        .restore(fixture.root, outcome.operation)
        .unwrap();
    assert_eq!(restored, song, "undo restores the same UUID");

    // Files back at their original paths with the exact bytes.
    let base = fixture.fs.root_path(fixture.root).expect("root");
    assert_eq!(
        std::fs::read(base.join("media/歌手/周杰伦 - 晴天.flac")).unwrap(),
        b"audio-bytes"
    );
    assert_eq!(
        std::fs::read(base.join("media/歌手/周杰伦 - 晴天.lrc")).unwrap(),
        b"lrc-bytes"
    );

    // The song is available again; favorite/stats/playlist position kept.
    let after = SongRepository::by_id(&fixture.database, song)
        .unwrap()
        .unwrap();
    assert_eq!(after.availability(), SongAvailability::Available);
    assert!(after.favorite());
    assert_eq!(after.play_count().as_u64(), 2);
    assert_eq!(playlist_position(&fixture, playlist), 7);
    let items = fixture.database.items(outcome.operation).unwrap();
    assert!(items
        .iter()
        .all(|item| item.state == OperationState::Restored));
    assert!(fixture
        .database
        .released_claims()
        .contains(&outcome.operation));
}

#[test]
fn restore_to_safe_numbered_path_when_original_is_occupied() {
    let fixture = ScanFixture::new();
    let song = seed_song(
        &fixture,
        "media/歌手/周杰伦 - 晴天.flac",
        b"audio-bytes",
        Some(b"lrc-bytes"),
    );
    let outcome = DeleteSongs::new(&fixture.deps)
        .delete(fixture.root, song)
        .unwrap();

    // Foreign content occupies the original paths before the undo.
    fixture.write_file("media/歌手/周杰伦 - 晴天.flac", b"foreign-audio");
    fixture.write_file("media/歌手/周杰伦 - 晴天.lrc", b"foreign-lrc");

    RestoreDeletedOperation::new(&fixture.deps)
        .restore(fixture.root, outcome.operation)
        .unwrap();

    let base = fixture.fs.root_path(fixture.root).expect("root");
    // The foreign files are untouched; our files land at numbered paths.
    assert_eq!(
        std::fs::read(base.join("media/歌手/周杰伦 - 晴天.flac")).unwrap(),
        b"foreign-audio"
    );
    assert_eq!(
        std::fs::read(base.join("media/歌手/周杰伦 - 晴天.lrc")).unwrap(),
        b"foreign-lrc"
    );
    assert_eq!(
        std::fs::read(base.join("media/歌手/周杰伦 - 晴天 (1).flac")).unwrap(),
        b"audio-bytes",
        "audio restored to the safe numbered path"
    );
    assert_eq!(
        std::fs::read(base.join("media/歌手/周杰伦 - 晴天 (1).lrc")).unwrap(),
        b"lrc-bytes",
        "lyrics restored to the safe numbered path"
    );
    // The song keeps its identity (and the record path stays the original:
    // relinking to the numbered path happens on a later scan).
    let after = SongRepository::by_id(&fixture.database, song)
        .unwrap()
        .unwrap();
    assert_eq!(after.availability(), SongAvailability::Available);
    assert_eq!(after.id(), song);
}

#[test]
fn restore_is_refused_after_the_undo_window_expires() {
    let fixture = ScanFixture::new();
    let song = seed_song(&fixture, "歌手/周杰伦 - 晴天.flac", b"audio-bytes", None);
    let outcome = DeleteSongs::new(&fixture.deps)
        .delete(fixture.root, song)
        .unwrap();

    fixture.clock.advance_ms(20_000);
    let error = RestoreDeletedOperation::new(&fixture.deps)
        .restore(fixture.root, outcome.operation)
        .unwrap_err();
    assert_eq!(error.code(), "conflict", "expired undo is refused");
    // Nothing moved: the audio remains staged in the trash slot.
    let base = fixture.fs.root_path(fixture.root).expect("root");
    assert!(!base.join("歌手/周杰伦 - 晴天.flac").exists());
    let staged_audio = fixture
        .database
        .items(outcome.operation)
        .unwrap()
        .into_iter()
        .find(|item| item.item_key == "audio")
        .and_then(|item| item.staging_path)
        .expect("audio staging path");
    assert!(base.join(staged_audio.normalized()).exists());
}

#[test]
fn read_only_root_refuses_the_delete_entry() {
    let fixture = ScanFixture::new();
    let song = seed_song(&fixture, "歌手/周杰伦 - 晴天.flac", b"audio-bytes", None);
    fixture.fs.set_write_capable(false);
    let error = DeleteSongs::new(&fixture.deps)
        .delete(fixture.root, song)
        .unwrap_err();
    assert_eq!(
        error.code(),
        "unavailable",
        "read-only root has no delete entry"
    );
    // Nothing changed.
    let after = SongRepository::by_id(&fixture.database, song)
        .unwrap()
        .unwrap();
    assert_eq!(after.availability(), SongAvailability::Available);
}

#[test]
fn delete_without_a_sidecar_stages_only_the_audio() {
    let fixture = ScanFixture::new();
    let song = seed_song(&fixture, "歌手/周杰伦 - 晴天.flac", b"audio-bytes", None);
    let outcome = DeleteSongs::new(&fixture.deps)
        .delete(fixture.root, song)
        .unwrap();
    assert!(!outcome.staged_lyrics);
    let items = fixture.database.items(outcome.operation).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].kind, OperationResourceKind::Audio);
}

/// `PendingDelete` is a local intent only. The portable song fact remains
/// available until finalization commits the tombstone.
#[test]
fn pending_delete_does_not_publish_a_tombstone() {
    use crate::application::portable_materialize::song_record;
    use crate::application::ports::ControlPlanePort;
    use crate::domain::ids::Revision;
    use crate::domain::library::{HybridLogicalClock, PortableRecord, RecordKind};

    let fixture = ScanFixture::new();
    let song = seed_song(
        &fixture,
        "media/歌手/周杰伦 - 晴天.flac",
        b"audio-bytes",
        None,
    );
    // A committed import would have left this song record behind.
    let entity = SongRepository::by_id(&fixture.database, song)
        .unwrap()
        .unwrap();
    let device = fixture.deps.device_id.current_device_id();
    fixture
        .control
        .write_record(
            fixture.root,
            &PortableRecord::Song(
                song_record(
                    device,
                    HybridLogicalClock::default(),
                    Revision::INITIAL,
                    &entity,
                )
                .unwrap(),
            ),
        )
        .unwrap();

    DeleteSongs::new(&fixture.deps)
        .delete(fixture.root, song)
        .unwrap();

    let tombstone = fixture
        .control
        .read_record(fixture.root, RecordKind::Tombstone, &song.to_string())
        .unwrap();
    assert!(
        tombstone.is_none(),
        "undo-window intent is not a shared tombstone"
    );
    let retained_song = fixture
        .control
        .read_record(fixture.root, RecordKind::Song, &song.to_string())
        .unwrap();
    assert!(
        matches!(retained_song, Some(PortableRecord::Song(record)) if record.revision == Revision::INITIAL)
    );
    assert_eq!(
        SongRepository::by_id(&fixture.database, song)
            .unwrap()
            .unwrap()
            .availability(),
        SongAvailability::PendingDelete
    );
}

/// Undo cancels the local intent and leaves shared portable state intact.
#[test]
fn undoing_a_delete_removes_a_legacy_temporary_tombstone() {
    use crate::application::portable_materialize::{song_record, tombstone_record};
    use crate::application::ports::ControlPlanePort;
    use crate::domain::ids::Revision;
    use crate::domain::library::{HybridLogicalClock, PortableRecord, RecordKind};

    let fixture = ScanFixture::new();
    let song = seed_song(
        &fixture,
        "media/歌手/周杰伦 - 晴天.flac",
        b"audio-bytes",
        None,
    );
    let entity = SongRepository::by_id(&fixture.database, song)
        .unwrap()
        .unwrap();
    let device = fixture.deps.device_id.current_device_id();
    fixture
        .control
        .write_record(
            fixture.root,
            &PortableRecord::Song(
                song_record(
                    device,
                    HybridLogicalClock::default(),
                    Revision::INITIAL,
                    &entity,
                )
                .unwrap(),
            ),
        )
        .unwrap();
    fixture
        .control
        .write_record(
            fixture.root,
            &PortableRecord::Tombstone(tombstone_record(
                device,
                HybridLogicalClock::default(),
                Revision::from_u64(1),
                song.as_uuid(),
                RecordKind::Song,
            )),
        )
        .unwrap();
    let outcome = DeleteSongs::new(&fixture.deps)
        .delete(fixture.root, song)
        .unwrap();
    assert!(fixture
        .control
        .read_record(fixture.root, RecordKind::Tombstone, &song.to_string())
        .unwrap()
        .is_some());

    RestoreDeletedOperation::new(&fixture.deps)
        .restore(fixture.root, outcome.operation)
        .unwrap();

    assert!(
        fixture
            .control
            .read_record(fixture.root, RecordKind::Tombstone, &song.to_string())
            .unwrap()
            .is_none(),
        "undo must revoke the tombstone together with the delete"
    );
    let after = SongRepository::by_id(&fixture.database, song)
        .unwrap()
        .unwrap();
    assert_eq!(after.availability(), SongAvailability::Available);
}
