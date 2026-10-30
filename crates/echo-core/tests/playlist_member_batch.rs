//! Regression: adding several songs to one playlist must keep every membership.
//!
//! The desktop "加入歌单" batch runs one `add_to_playlists(song, targets)` per
//! selected song. `add_to_playlists` is an interactive edit, so in a
//! non-`cfg(test)` build it commits to the directory and lets the projection
//! worker catch SQLite up asynchronously. The next song's edit therefore plans
//! against a SQLite read model that is still behind the directory.
//!
//! This drives the production composition root (real directory durability +
//! real SQLite + the real projection worker) exactly like the desktop does.

use echo_core::application::playlist::durable::DurableLibraryEdits;
use echo_core::application::ports::{
    ControlPlanePort, DeviceIdProvider, LibraryProjectionPort, LibraryRepository,
    PlaylistRepository, UnitOfWork,
};
use echo_core::application::root_switch::ROOT_EPOCH_KEY;
use echo_core::domain::entities::{LibraryRoot, SongAvailability};
use echo_core::domain::ids::Revision;
use echo_core::domain::ids::{LibraryRootId, SongId};
use echo_core::domain::library::durability::{ChangeOrigin, LibraryBatch};
use echo_core::domain::library::{
    HybridLogicalClock, LibraryId, LibraryManifest, LibraryRelativePath, PortableRecord,
    PortableRecordEnvelope, SongRecord,
};
use echo_core::infrastructure::filesystem::{RootControlPlane, RootRegistry};
use echo_core::infrastructure::sqlite::SqliteDatabase;
use std::sync::Arc;

struct Stack {
    _directory: tempfile::TempDir,
    database: Arc<SqliteDatabase>,
    control: Arc<RootControlPlane>,
    root: LibraryRootId,
    songs: Vec<SongId>,
}

fn stack(song_count: usize) -> Stack {
    let directory = tempfile::tempdir().expect("private fixture");
    let library_dir = directory.path().join("library");
    std::fs::create_dir(&library_dir).expect("library dir");
    let database =
        Arc::new(SqliteDatabase::open(directory.path().join("echo.sqlite")).expect("real sqlite"));
    let root = LibraryRootId::new();
    let registry = RootRegistry::new();
    registry.register(root, &library_dir);
    let control = Arc::new(RootControlPlane::new(registry));
    let library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    control
        .write_manifest(root, &LibraryManifest::new(library, "member batch"))
        .expect("library manifest");
    database.configure_library_control(control.clone());
    LibraryRepository::upsert(
        database.as_ref(),
        &LibraryRoot::new(root, library_dir, true, true),
    )
    .expect("active root");
    database
        .with_tx(Box::new(|tx| tx.set_runtime_state(ROOT_EPOCH_KEY, "7")))
        .expect("runtime epoch");

    // Seed available songs through the directory, exactly as a completed scan
    // would, then let the read model catch up once before measuring edits.
    let device = database.current_device_id();
    let songs: Vec<_> = (0..song_count).map(|_| SongId::new()).collect();
    let records = songs
        .iter()
        .enumerate()
        .map(|(index, song)| {
            PortableRecordEnvelope::new(
                PortableRecord::Song(SongRecord {
                    song_uuid: song.as_uuid(),
                    revision: Revision::INITIAL,
                    updated_by_device_id: device,
                    hlc: HybridLogicalClock::new(1_700_000_000, 0),
                    media_path: LibraryRelativePath::new(&format!(
                        "media/artist/song-{index:03}.flac"
                    ))
                    .expect("relative media path"),
                    content_hash: format!("{index:064x}"),
                    added_at: 1_700_000_000_000 + index as u64,
                    title: Some(format!("Song {index}")),
                    artist: Some("Artist".to_owned()),
                    album: Some("Album".to_owned()),
                }),
                None,
            )
            .expect("portable song")
        })
        .collect();
    let durable = control.durability().expect("real durability");
    durable
        .commit(
            root,
            &LibraryBatch::new(uuid::Uuid::new_v4(), library, ChangeOrigin::Scan, records),
        )
        .expect("durable song seed");
    let replay = durable.recover(root).expect("verify fixture directory");
    database
        .project_library_commits(root, &replay)
        .expect("seed projection");
    let available = songs.clone();
    database
        .with_tx(Box::new(move |tx| {
            for song in available {
                tx.set_song_availability(song, SongAvailability::Available)?;
            }
            Ok(())
        }))
        .expect("seed availability");

    Stack {
        _directory: directory,
        database,
        control,
        root,
        songs,
    }
}

/// The desktop picker sends one add per selected song, back to back, without
/// waiting for the projection worker in between.
#[test]
fn consecutive_multi_song_add_keeps_every_membership() {
    let stack = stack(3);
    let edits = DurableLibraryEdits {
        control: stack.control.as_ref(),
        songs: stack.database.as_ref(),
        playlists: stack.database.as_ref(),
        unit: stack.database.as_ref(),
        device: stack.database.current_device_id(),
    };
    let playlist = edits
        .create_playlist(stack.root, "批量加入")
        .expect("create playlist");
    stack
        .database
        .flush_library_projection(stack.root, std::time::Duration::from_secs(30))
        .expect("playlist projection settles before the batch");
    for song in &stack.songs {
        edits
            .add_to_playlists(stack.root, *song, &[playlist])
            .expect("add selected song");
    }

    // Give the background projection worker all the time a real session would.
    stack
        .database
        .flush_library_projection(stack.root, std::time::Duration::from_secs(30))
        .expect("projection catches up");

    // The directory is the authority: a wiped projection must rebuild all of
    // them too, so this is not just a read-model lag.
    let members = PlaylistRepository::members(stack.database.as_ref(), playlist).expect("members");
    let replay = stack
        .control
        .durability()
        .expect("durability")
        .recover(stack.root)
        .expect("recover");
    let mut item_records: Vec<_> = replay
        .commits
        .iter()
        .flat_map(|commit| &commit.batch.records)
        .filter_map(|envelope| match envelope.record() {
            PortableRecord::PlaylistItem(item) => Some(item.clone()),
            _ => None,
        })
        .collect();
    item_records.sort_by_key(|item| item.position);
    assert_eq!(
        item_records.len(),
        stack.songs.len(),
        "the directory keeps one member record per selected song"
    );
    assert_eq!(
        item_records
            .iter()
            .map(|item| item.position)
            .collect::<Vec<_>>(),
        (0..stack.songs.len() as u64).collect::<Vec<_>>(),
        "members keep the append positions the user acted in"
    );
    let member_ids: std::collections::BTreeSet<_> =
        item_records.iter().map(|item| item.item_uuid).collect();
    assert_eq!(
        member_ids.len(),
        stack.songs.len(),
        "each selected song gets its own member identity"
    );
    assert!(item_records
        .iter()
        .all(|item| item.playlist_uuid == playlist.as_uuid() && item.added_at.is_some()));
    assert_eq!(
        item_records
            .iter()
            .map(|item| item.song_uuid)
            .collect::<Vec<_>>(),
        stack
            .songs
            .iter()
            .map(|song| song.as_uuid())
            .collect::<Vec<_>>(),
        "members are appended in the order the user selected them"
    );
    assert_eq!(
        members.len(),
        stack.songs.len(),
        "every selected song keeps its membership"
    );
    assert_eq!(
        members
            .iter()
            .map(echo_core::domain::entities::PlaylistMember::position)
            .collect::<Vec<_>>(),
        (0..stack.songs.len() as u64).collect::<Vec<_>>(),
        "the read model already shows the append order"
    );
}

/// The playlist view reads membership through the repository; a stale read
/// model is what the user sees as "only one song was added".
#[test]
fn playlist_member_count_matches_the_selected_song_count() {
    let stack = stack(2);
    let edits = DurableLibraryEdits {
        control: stack.control.as_ref(),
        songs: stack.database.as_ref(),
        playlists: stack.database.as_ref(),
        unit: stack.database.as_ref(),
        device: stack.database.current_device_id(),
    };
    let playlist = edits
        .create_playlist(stack.root, "计数")
        .expect("create playlist");
    for song in &stack.songs {
        edits
            .add_to_playlists(stack.root, *song, &[playlist])
            .expect("add");
    }
    stack
        .database
        .flush_library_projection(stack.root, std::time::Duration::from_secs(30))
        .expect("projection catches up");
    let count = PlaylistRepository::members(stack.database.as_ref(), playlist)
        .expect("members")
        .len();
    assert_eq!(count, 2, "the view must not under-count a batch add");
}

/// Human-paced batches are what the picker issues; the catch-up must stay
/// cheap enough that a burst of adds does not become visibly slow.
#[test]
fn burst_add_latency_is_bounded() {
    let stack = stack(60);
    let edits = DurableLibraryEdits {
        control: stack.control.as_ref(),
        songs: stack.database.as_ref(),
        playlists: stack.database.as_ref(),
        unit: stack.database.as_ref(),
        device: stack.database.current_device_id(),
    };
    let playlist = edits
        .create_playlist(stack.root, "突发")
        .expect("create playlist");
    stack
        .database
        .flush_library_projection(stack.root, std::time::Duration::from_secs(30))
        .expect("settle");
    let mut samples = Vec::new();
    for song in &stack.songs {
        let started = std::time::Instant::now();
        edits
            .add_to_playlists(stack.root, *song, &[playlist])
            .expect("add");
        samples.push(started.elapsed());
    }
    stack
        .database
        .flush_library_projection(stack.root, std::time::Duration::from_secs(60))
        .expect("catch up");
    assert_eq!(
        PlaylistRepository::members(stack.database.as_ref(), playlist)
            .expect("members")
            .len(),
        stack.songs.len()
    );
    samples.sort();
    let p95 = samples[(samples.len() * 95).div_ceil(100) - 1];
    eprintln!("burst add p95 = {p95:?}");
    assert!(
        p95 < std::time::Duration::from_millis(200),
        "a burst add must stay interactive, got {p95:?}"
    );
}
