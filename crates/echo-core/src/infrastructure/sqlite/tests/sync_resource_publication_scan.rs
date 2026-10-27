//! Newer same-content scan after-images supersede old physical publication pointers.
use super::*;
use crate::application::scan::{ScanDeps, StartScan};
use crate::application::testkit::scan_fixture::ScanFixture;

#[test]
fn library_durability_resource_location_does_not_undo_a_later_same_hash_scan_move() {
    for managed in [false, true] {
        let f = DirectoryFixture::new();
        if managed {
            std::fs::create_dir_all(f.path().join("media")).unwrap();
            std::fs::write(f.path().join("media/track.flac"), b"foreign retained music").unwrap();
        }
        f.publish(b"initial audio");
        let ObjectKey::Song(song) = f.object else {
            unreachable!()
        };
        let old = SongRepository::by_id(f.database.as_ref(), song)
            .unwrap()
            .unwrap()
            .path()
            .clone();
        let moved = RelativeMediaPath::new("media/moved.flac").unwrap();
        std::fs::rename(
            f.path().join(old.normalized()),
            f.path().join(moved.normalized()),
        )
        .unwrap();
        let scan = ScanFixture::new();
        scan.fs.add_root_at(f.root, f.path());
        scan.set_audio("moved.flac", "Moved track", 1000);
        let deps = ScanDeps {
            history: Some(f.database.clone()),
            roots: f.database.clone(),
            songs: f.database.clone(),
            catalog: f.database.clone(),
            playlists: f.database.clone(),
            lyrics: f.database.clone(),
            covers: f.database.clone(),
            runs: f.database.clone(),
            journal: f.database.clone(),
            uow: f.database.clone(),
            device_id: f.database.clone(),
            sync: f.database.clone(),
            control: f.plane.clone(),
            ..(*scan.deps).clone()
        };
        StartScan::new(&deps, &scan.supervisor).run(f.root).unwrap();
        let after_scan = SongRepository::by_id(f.database.as_ref(), song)
            .unwrap()
            .unwrap();
        assert_eq!(
            after_scan.path(),
            &moved,
            "an older location pointer undid the committed scan move"
        );
        assert_eq!(
            after_scan.blake3_hash(),
            Some(f.initial.digest.to_hex().as_str())
        );
        assert_eq!(
            SongRepository::all_in_root(f.database.as_ref(), f.root)
                .unwrap()
                .len(),
            1
        );
        let replay = f.plane.durability().unwrap().recover(f.root).unwrap();
        let fresh_dir = tempfile::tempdir().unwrap();
        let fresh = SqliteDatabase::open(fresh_dir.path().join("fresh.sqlite")).unwrap();
        LibraryRepository::upsert(&fresh, &LibraryRoot::new(f.root, f.path(), true, true)).unwrap();
        for _ in 0..2 {
            fresh.project_library_commits(f.root, &replay).unwrap();
            assert_eq!(
                SongRepository::by_id(&fresh, song).unwrap().unwrap().path(),
                &moved
            );
            assert!(
                SongRepository::published_file_paths(&fresh, f.root)
                    .unwrap()
                    .is_empty()
            );
        }
        assert_eq!(
            std::fs::read(f.path().join(moved.normalized())).unwrap(),
            b"initial audio"
        );
    }
}
