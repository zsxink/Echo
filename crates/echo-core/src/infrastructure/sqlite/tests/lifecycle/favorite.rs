//! Favorite values and their frozen events follow ordered directory commits.
use super::*;
use crate::application::ports::{LibraryProjectionPort, SongRepository};
use crate::domain::ids::SongId;
use crate::domain::library::durability::{ChangeOrigin, LibraryBatch};
use crate::domain::library::{
    HybridLogicalClock, LibraryRelativePath, PortableRecordEnvelope, SongRecord,
};

pub(super) fn seed_song(stack: &ScanStack, control: &RootControlPlane, deps: &ScanDeps) -> SongId {
    seed_song_at(stack, control, deps, "media/favorite.flac")
}

pub(super) fn seed_song_at(
    stack: &ScanStack,
    control: &RootControlPlane,
    deps: &ScanDeps,
    media_path: &str,
) -> SongId {
    let song = SongId::new();
    let library = control
        .read_manifest(stack.root)
        .unwrap()
        .unwrap()
        .library_id;
    let record = PortableRecord::Song(SongRecord {
        song_uuid: song.as_uuid(),
        media_path: LibraryRelativePath::new(media_path).unwrap(),
        content_hash: blake3::hash(media_path.as_bytes()).to_hex().to_string(),
        added_at: 123,
        title: Some("favorite song".into()),
        artist: None,
        album: None,
        revision: crate::domain::ids::Revision::from_u64(1),
        hlc: HybridLogicalClock::new(1, 0),
        updated_by_device_id: deps.device_id.current_device_id(),
    });
    control
        .durability()
        .unwrap()
        .commit(
            stack.root,
            &LibraryBatch::new(
                uuid::Uuid::new_v4(),
                library,
                ChangeOrigin::Scan,
                vec![PortableRecordEnvelope::new(record, None).unwrap()],
            ),
        )
        .unwrap();
    let replay = control.durability().unwrap().recover(stack.root).unwrap();
    stack
        .database
        .project_library_commits(stack.root, &replay)
        .unwrap();
    song
}

fn favorite_time(stack: &ScanStack, song: SongId) -> Option<u64> {
    stack
        .database
        .with_reader(|c| {
            c.query_row(
                "SELECT favorited_at FROM songs WHERE uuid=?1",
                [song.to_string()],
                |r| r.get(0),
            )
            .map_err(storage)
        })
        .unwrap()
}

#[test]
fn library_durability_favorite_failure_is_saved_and_later_false_catches_up() {
    let stack = ScanStack::new();
    let (_, control, deps) = configured_roots(&stack);
    let song = seed_song(&stack, &control, &deps);
    let edits = DurableLibraryEdits {
        control: deps.control.as_ref(),
        songs: deps.songs.as_ref(),
        playlists: deps.playlists.as_ref(),
        unit: deps.uow.as_ref(),
        device: deps.device_id.current_device_id(),
    };
    stack.database.writer.run(|c| {
        c.execute_batch("CREATE TRIGGER fail_favorite_projection BEFORE INSERT ON library_projection_watermarks BEGIN SELECT RAISE(ABORT,'injected favorite projection failure'); END;").map_err(storage)
    }).unwrap();
    assert!(matches!(
        edits.set_favorite(stack.root, song, true),
        Err(crate::error::Error::LibrarySavedPending { .. })
    ));
    assert!(!SongRepository::by_id(stack.database.as_ref(), song)
        .unwrap()
        .unwrap()
        .favorite());
    let saved = control.durability().unwrap().recover(stack.root).unwrap();
    let PortableRecord::Favorite(favorite) =
        saved.commits.last().unwrap().batch.records[0].record()
    else {
        panic!("favorite after-image expected")
    };
    assert!(favorite.is_favorite);
    assert_eq!(saved.commits.last().unwrap().batch.state.len(), 2);
    stack
        .database
        .writer
        .run(|c| {
            c.execute_batch("DROP TRIGGER fail_favorite_projection")
                .map_err(storage)
        })
        .unwrap();
    assert!(!edits
        .set_favorite(stack.root, song, false)
        .unwrap()
        .favorite());
    let replay = control.durability().unwrap().recover(stack.root).unwrap();
    for _ in 0..2 {
        stack
            .database
            .project_library_commits(stack.root, &replay)
            .unwrap();
        assert!(!SongRepository::by_id(stack.database.as_ref(), song)
            .unwrap()
            .unwrap()
            .favorite());
    }
}

#[test]
fn library_durability_favorite_time_survives_toggle_and_fresh_projection() {
    let stack = ScanStack::new();
    let (_, control, deps) = configured_roots(&stack);
    let song = seed_song(&stack, &control, &deps);
    let edits = DurableLibraryEdits {
        control: deps.control.as_ref(),
        songs: deps.songs.as_ref(),
        playlists: deps.playlists.as_ref(),
        unit: deps.uow.as_ref(),
        device: deps.device_id.current_device_id(),
    };
    edits.set_favorite(stack.root, song, true).unwrap();
    let first = favorite_time(&stack, song).expect("a saved favorite keeps its ordering time");
    edits.set_favorite(stack.root, song, true).unwrap();
    assert_eq!(favorite_time(&stack, song), Some(first));
    edits.set_favorite(stack.root, song, false).unwrap();
    assert_eq!(favorite_time(&stack, song), None);
    edits.set_favorite(stack.root, song, true).unwrap();
    let time = favorite_time(&stack, song).unwrap();
    let replay = control.durability().unwrap().recover(stack.root).unwrap();
    let record = replay.commits.last().unwrap().batch.records[0].record();
    let json = serde_json::to_value(record).unwrap();
    assert_eq!(json["favorited_at"].as_u64(), Some(time));
    let directory = tempfile::tempdir().unwrap();
    let rebuilt =
        crate::infrastructure::sqlite::SqliteDatabase::open(directory.path().join("fresh.sqlite"))
            .unwrap();
    LibraryRepository::upsert(
        &rebuilt,
        &LibraryRoot::new(stack.root, stack.library_dir.clone(), true, true),
    )
    .unwrap();
    for _ in 0..2 {
        rebuilt
            .project_library_commits(stack.root, &replay)
            .unwrap();
        let actual: Option<u64> = rebuilt
            .with_reader(|c| {
                c.query_row(
                    "SELECT favorited_at FROM songs WHERE uuid=?1",
                    [song.to_string()],
                    |r| r.get(0),
                )
                .map_err(storage)
            })
            .unwrap();
        assert_eq!(actual, Some(time));
        assert!(SongRepository::by_id(&rebuilt, song)
            .unwrap()
            .unwrap()
            .favorite());
    }
}
