//! Continuation tests: the behaviours that must survive "open the same
//! directory again" (issue #1).

use std::collections::BTreeMap;

use crate::application::continuation::{ContinueFromRecords, ProjectionReport};
use crate::application::ports::{ControlPlanePort, PlaylistRepository, SongRepository};
use crate::application::testing::scan_fixture::ScanFixture;
use crate::domain::ids::{PlaylistId, PlaylistItemId, Revision, SongId};
use crate::domain::library::{
    DeviceId, FavoriteRecord, HybridLogicalClock, LibraryRelativePath, PlayStatsRecord,
    PlaylistItemRecord, PlaylistRecord, PortableRecord, RecordKind, SongRecord, TombstoneRecord,
};

const HLC: HybridLogicalClock = HybridLogicalClock::new(1_700_000_000, 0);

fn song_record(song_uuid: uuid::Uuid, path: &str) -> PortableRecord {
    PortableRecord::Song(SongRecord {
        song_uuid,
        revision: Revision::INITIAL,
        updated_by_device_id: DeviceId::new(),
        hlc: HLC,
        media_path: LibraryRelativePath::new(path).expect("media path"),
        content_hash: "hash".to_owned(),
        title: Some("晴天".to_owned()),
        artist: Some("歌手".to_owned()),
        album: None,
    })
}

fn favorite_record(song_uuid: uuid::Uuid, is_favorite: bool) -> PortableRecord {
    PortableRecord::Favorite(FavoriteRecord {
        song_uuid,
        revision: Revision::INITIAL,
        updated_by_device_id: DeviceId::new(),
        hlc: HLC,
        is_favorite,
    })
}

fn tombstone(object_uuid: uuid::Uuid, kind: RecordKind) -> PortableRecord {
    PortableRecord::Tombstone(TombstoneRecord {
        object_uuid,
        deleted_kind: kind,
        revision: Revision::from_u64(9),
        updated_by_device_id: DeviceId::new(),
        hlc: HybridLogicalClock::new(1_700_000_100, 0),
    })
}

fn continuation(fixture: &ScanFixture) -> ContinueFromRecords<'_> {
    ContinueFromRecords::new(
        &fixture.control,
        fixture.deps.songs.as_ref(),
        fixture.deps.playlists.as_ref(),
        fixture.deps.uow.as_ref(),
    )
}

/// A library whose *local database was wiped*: song/favorite/playlist/member
/// records exist in `echo/`, the store is empty, and no manifest is present.
#[test]
fn continuation_restores_songs_favorites_playlists_and_member_order() {
    let fixture = ScanFixture::new();
    let song_a = uuid::Uuid::new_v4();
    let song_b = uuid::Uuid::new_v4();
    let playlist_uuid = uuid::Uuid::new_v4();
    let item_a = uuid::Uuid::new_v4();
    let item_b = uuid::Uuid::new_v4();

    fixture
        .control
        .write_record(
            fixture.root,
            &song_record(song_a, "media/歌手/歌手 - 晴天.flac"),
        )
        .expect("song a");
    fixture
        .control
        .write_record(
            fixture.root,
            &song_record(song_b, "media/歌手/歌手 - 稻香.flac"),
        )
        .expect("song b");
    fixture
        .control
        .write_record(fixture.root, &favorite_record(song_a, true))
        .expect("favorite a");
    fixture
        .control
        .write_record(fixture.root, &favorite_record(song_b, false))
        .expect("unfavorite b");
    fixture
        .control
        .write_record(
            fixture.root,
            &PortableRecord::Playlist(PlaylistRecord {
                playlist_uuid,
                revision: Revision::INITIAL,
                updated_by_device_id: DeviceId::new(),
                hlc: HLC,
                display_name: "通勤路上".to_owned(),
            }),
        )
        .expect("playlist");
    // Members are written out of order on purpose: the record order on disk
    // must not decide the playlist order.
    for (item, song, position) in [(item_b, song_b, 1), (item_a, song_a, 0)] {
        fixture
            .control
            .write_record(
                fixture.root,
                &PortableRecord::PlaylistItem(PlaylistItemRecord {
                    item_uuid: item,
                    revision: Revision::INITIAL,
                    updated_by_device_id: DeviceId::new(),
                    hlc: HLC,
                    playlist_uuid,
                    song_uuid: song,
                    position,
                }),
            )
            .expect("member");
    }

    let report = continuation(&fixture).run(fixture.root).expect("continue");
    // The manifest was missing while records existed → one self-heal.
    assert!(report.control_plane.healed());
    assert_eq!(report.projection.songs, 2);
    assert_eq!(report.projection.favorites, 2);
    assert_eq!(report.projection.playlists, 1);
    assert_eq!(report.projection.members, 2);
    assert_eq!(report.projection.invalid, 0);

    // Identity: the songs carry the record UUIDs, not fresh ones.
    let a = SongRepository::by_id(fixture.deps.songs.as_ref(), SongId::from_uuid(song_a))
        .expect("read a")
        .expect("present");
    assert_eq!(a.title(), Some("晴天"));
    assert!(
        a.favorite(),
        "「我的喜欢」 is restored from the favorite record"
    );
    let b = SongRepository::by_id(fixture.deps.songs.as_ref(), SongId::from_uuid(song_b))
        .expect("read b")
        .expect("present");
    assert!(
        !b.favorite(),
        "an explicit is_favorite:false record is a real state, not a missing one"
    );

    // Playlist + member order, restored by position.
    let playlist = PlaylistId::from_uuid(playlist_uuid);
    assert_eq!(
        PlaylistRepository::name(fixture.deps.playlists.as_ref(), playlist)
            .expect("name")
            .as_deref(),
        Some("通勤路上")
    );
    let members =
        PlaylistRepository::members(fixture.deps.playlists.as_ref(), playlist).expect("members");
    assert_eq!(members.len(), 2);
    assert_eq!(members[0].song(), SongId::from_uuid(song_a));
    assert_eq!(members[1].song(), SongId::from_uuid(song_b));
    assert_eq!(
        members[0].id(),
        PlaylistItemId::from_uuid(item_a),
        "the member keeps the record's UUID instead of a fresh one"
    );
}

#[test]
fn continuation_is_idempotent_across_repeated_opens() {
    let fixture = ScanFixture::new();
    let song = uuid::Uuid::new_v4();
    let playlist_uuid = uuid::Uuid::new_v4();
    let item = uuid::Uuid::new_v4();
    fixture
        .control
        .write_record(
            fixture.root,
            &song_record(song, "media/歌手/歌手 - 晴天.flac"),
        )
        .expect("song");
    fixture
        .control
        .write_record(fixture.root, &favorite_record(song, true))
        .expect("favorite");
    fixture
        .control
        .write_record(
            fixture.root,
            &PortableRecord::Playlist(PlaylistRecord {
                playlist_uuid,
                revision: Revision::INITIAL,
                updated_by_device_id: DeviceId::new(),
                hlc: HLC,
                display_name: "List".to_owned(),
            }),
        )
        .expect("playlist");
    fixture
        .control
        .write_record(
            fixture.root,
            &PortableRecord::PlaylistItem(PlaylistItemRecord {
                item_uuid: item,
                revision: Revision::INITIAL,
                updated_by_device_id: DeviceId::new(),
                hlc: HLC,
                playlist_uuid,
                song_uuid: song,
                position: 0,
            }),
        )
        .expect("member");
    fixture
        .control
        .write_record(
            fixture.root,
            &PortableRecord::PlayStats(PlayStatsRecord {
                song_uuid: song,
                revision: Revision::INITIAL,
                updated_by_device_id: DeviceId::new(),
                hlc: HLC,
                by_device: BTreeMap::from([(uuid::Uuid::new_v4(), 1), (uuid::Uuid::new_v4(), 2)]),
            }),
        )
        .expect("play stats");

    let first = continuation(&fixture).run(fixture.root).expect("first");
    let second = continuation(&fixture).run(fixture.root).expect("second");
    // The second pass heals nothing (the manifest now exists) and adopts the
    // same objects — never a duplicate song, playlist or member.
    assert!(!second.control_plane.healed());
    assert_eq!(first.projection.adopted(), second.projection.adopted());
    let songs = fixture.all_songs();
    assert_eq!(songs.len(), 1);
    assert_eq!(songs[0].id(), SongId::from_uuid(song));
    assert_eq!(
        songs[0].play_count().as_u64(),
        3,
        "Σ by_device, and a replay never doubles the count"
    );
    let playlist = PlaylistId::from_uuid(playlist_uuid);
    assert_eq!(
        PlaylistRepository::members(fixture.deps.playlists.as_ref(), playlist)
            .expect("members")
            .len(),
        1,
        "no duplicated member after a second open"
    );
    assert_eq!(
        PlaylistRepository::list(fixture.deps.playlists.as_ref(), fixture.root)
            .expect("list")
            .len(),
        1
    );
}

#[test]
fn tombstone_outranks_a_stale_record_and_is_never_resurrected() {
    let fixture = ScanFixture::new();
    let song = uuid::Uuid::new_v4();
    let playlist_uuid = uuid::Uuid::new_v4();
    let item = uuid::Uuid::new_v4();
    fixture
        .control
        .write_record(
            fixture.root,
            &song_record(song, "media/歌手/歌手 - 晴天.flac"),
        )
        .expect("song");
    fixture
        .control
        .write_record(fixture.root, &favorite_record(song, true))
        .expect("favorite");
    fixture
        .control
        .write_record(
            fixture.root,
            &PortableRecord::Playlist(PlaylistRecord {
                playlist_uuid,
                revision: Revision::INITIAL,
                updated_by_device_id: DeviceId::new(),
                hlc: HLC,
                display_name: "Deleted".to_owned(),
            }),
        )
        .expect("playlist");
    // First open: everything is adopted.
    let adopted = continuation(&fixture).run(fixture.root).expect("first");
    assert_eq!(adopted.projection.playlists, 1);
    assert_eq!(adopted.projection.tombstoned, 0);

    // Now the deletes land as tombstones (a newer version than every record).
    fixture
        .control
        .write_record(
            fixture.root,
            &tombstone(playlist_uuid, RecordKind::Playlist),
        )
        .expect("playlist tombstone");
    fixture
        .control
        .write_record(fixture.root, &tombstone(song, RecordKind::Song))
        .expect("song tombstone");

    let suppressed = continuation(&fixture).run(fixture.root).expect("second");
    assert!(
        suppressed.projection.tombstoned >= 2,
        "both tombstoned objects are suppressed: {suppressed:?}"
    );
    assert!(
        PlaylistRepository::by_id(
            fixture.deps.playlists.as_ref(),
            PlaylistId::from_uuid(playlist_uuid)
        )
        .expect("by id")
        .is_none(),
        "a tombstoned playlist does not come back"
    );
    let stored = SongRepository::by_id(fixture.deps.songs.as_ref(), SongId::from_uuid(song))
        .expect("read")
        .expect("present");
    assert!(
        !stored.favorite(),
        "a tombstoned favorite stays off — the delete propagates"
    );
    let _ = item;
}

#[test]
fn dangling_records_are_counted_and_never_invented() {
    let fixture = ScanFixture::new();
    let orphan_song = uuid::Uuid::new_v4();
    let playlist_uuid = uuid::Uuid::new_v4();
    // A favorite + play-stats for a song that has no song record at all, and a
    // member pointing at a playlist that does not exist.
    fixture
        .control
        .write_record(fixture.root, &favorite_record(orphan_song, true))
        .expect("orphan favorite");
    fixture
        .control
        .write_record(
            fixture.root,
            &PortableRecord::PlayStats(PlayStatsRecord {
                song_uuid: orphan_song,
                revision: Revision::INITIAL,
                updated_by_device_id: DeviceId::new(),
                hlc: HLC,
                by_device: BTreeMap::from([(uuid::Uuid::new_v4(), 4)]),
            }),
        )
        .expect("orphan stats");
    fixture
        .control
        .write_record(
            fixture.root,
            &PortableRecord::PlaylistItem(PlaylistItemRecord {
                item_uuid: uuid::Uuid::new_v4(),
                revision: Revision::INITIAL,
                updated_by_device_id: DeviceId::new(),
                hlc: HLC,
                playlist_uuid,
                song_uuid: orphan_song,
                position: 0,
            }),
        )
        .expect("orphan member");

    let report = continuation(&fixture).run(fixture.root).expect("continue");
    assert_eq!(report.projection.adopted(), 0, "nothing was invented");
    assert_eq!(
        report.projection.invalid, 3,
        "every dangling record is counted: {report:?}"
    );
    // The files themselves are untouched — this pass never deletes evidence.
    assert_eq!(fixture.control.records_of(fixture.root).len(), 3);
    assert!(fixture.all_songs().is_empty());
}

#[test]
fn a_directory_without_records_stays_a_pure_scan_target() {
    let fixture = ScanFixture::new();
    // No `echo/records/` at all: the legacy directory shape. Continuation must
    // not fabricate a manifest-owned identity or adopt anything.
    let report = continuation(&fixture).run(fixture.root).expect("continue");
    assert_eq!(report.projection, ProjectionReport::default());
    assert!(
        !report.control_plane.healed(),
        "an empty directory is initialized, not healed"
    );
}

#[test]
fn a_newer_manifest_refuses_continuation_instead_of_projecting() {
    let fixture = ScanFixture::new();
    let song = uuid::Uuid::new_v4();
    fixture
        .control
        .write_record(
            fixture.root,
            &song_record(song, "media/歌手/歌手 - 晴天.flac"),
        )
        .expect("song");
    fixture.control.set_manifest(
        fixture.root,
        crate::domain::library::LibraryManifest {
            format_version: crate::domain::library::CURRENT_FORMAT_VERSION + 5,
            library_id: crate::domain::library::LibraryId::from_uuid(uuid::Uuid::new_v4()),
            written_by_app_version: "9.9.9".to_owned(),
        },
    );

    let error = continuation(&fixture)
        .run(fixture.root)
        .expect_err("a newer manifest is a refusal");
    assert_eq!(error.code(), "unsupported_media");
    // Nothing was projected: the local store stays empty.
    assert!(fixture.all_songs().is_empty());
}

#[test]
fn an_unwritable_control_surface_continues_nothing_and_fails_nothing() {
    let fixture = ScanFixture::new();
    let song = uuid::Uuid::new_v4();
    fixture
        .control
        .write_record(
            fixture.root,
            &song_record(song, "media/歌手/歌手 - 晴天.flac"),
        )
        .expect("song");
    fixture.control.set_usable(false);

    let report = continuation(&fixture).run(fixture.root).expect("reported");
    assert_eq!(
        report.control_plane.library_id(),
        None,
        "no library identity without a writable control surface"
    );
    assert_eq!(report.projection, ProjectionReport::default());
    // The existing record was preserved (never overwritten with a half state).
    assert_eq!(fixture.control.records_of(fixture.root).len(), 1);
}

/// The measured shape of the reporter's own library (design evidence
/// E4/E6/E7): `echo/records/` describes the same `media/` files as the local
/// database, under an *older*, disjoint identity, with no manifest.
///
/// Continuation must not fold two identities onto one file, must not rewrite
/// the local rows, and must not fail. The local rows are the newer fact.
#[test]
fn detached_legacy_records_are_superseded_by_the_matching_local_rows() {
    use crate::domain::entities::SongAvailability;

    let fixture = ScanFixture::new();
    let seeded = fixture.seed_detached_legacy_records(6);
    assert_eq!(seeded.recorded.len(), 6);
    assert_eq!(seeded.favorites.len(), 2, "a subset, as E6 measured");
    let before = fixture.all_songs();
    assert_eq!(before.len(), 6);

    let report = continuation(&fixture).run(fixture.root).expect("reported");

    // Nothing was adopted: every record names a path a newer local row owns.
    assert_eq!(report.projection.songs, 0);
    assert_eq!(report.projection.superseded, 6);
    // The favorite records hang off the *older* identity, which never enters
    // the effective view, so they are counted (kept on disk) rather than
    // smeared onto the newer row (design D5 rule 3).
    assert_eq!(report.projection.invalid, seeded.favorites.len());
    assert_eq!(report.projection.favorites, 0);

    // The local rows are untouched — one file never gets two rows, and its
    // UUID is never rewritten to the older recorded one.
    let after = fixture.all_songs();
    assert_eq!(after.len(), 6, "never two rows for one file");
    let mut before_ids: Vec<String> = before.iter().map(|song| song.id().to_string()).collect();
    let mut after_ids: Vec<String> = after.iter().map(|song| song.id().to_string()).collect();
    before_ids.sort();
    after_ids.sort();
    assert_eq!(before_ids, after_ids, "local UUIDs are never rewritten");
    assert!(after
        .iter()
        .all(|song| song.availability() == SongAvailability::Available));
    // And the older generation's favorites are not smeared onto the newer
    // identity: an older record never overwrites the newer local state.
    assert!(after.iter().all(|song| !song.favorite()));

    // The manifest was healed (design D3 case 2) and every record was kept.
    assert!(report.control_plane.healed());
    assert!(fixture.control.records_present(fixture.root).unwrap());

    // Repeating the open changes nothing (spec 重复接续结果稳定).
    let again = continuation(&fixture).run(fixture.root).expect("reported");
    assert_eq!(again.projection, report.projection);
    assert_eq!(fixture.all_songs().len(), 6);
}

/// Real-scale replay of the acceptance case behind issue #1: a **wiped**
/// app-data directory rebuilt entirely from `echo/`, at the size and shape the
/// reporter's own library had (design E4/E6/E7 — 87 songs, 11 favorite records
/// of which 3 `true`, and no playlist ever written).
#[test]
fn a_wiped_database_is_rebuilt_from_the_records_at_real_scale() {
    use crate::domain::entities::SongAvailability;

    let fixture = ScanFixture::new();
    let mut recorded = Vec::new();
    for index in 0..87 {
        let uuid = uuid::Uuid::new_v4();
        fixture
            .control
            .write_record(
                fixture.root,
                &song_record(uuid, &format!("media/歌手/曲目{index:03}.flac")),
            )
            .expect("song record");
        recorded.push(uuid);
    }
    // 11 favorite records, the first 3 of them `true` — E6's measured split.
    let mut favorites = 0;
    for (index, uuid) in recorded.iter().enumerate() {
        if index % 8 == 0 {
            fixture
                .control
                .write_record(fixture.root, &favorite_record(*uuid, favorites < 3))
                .expect("favorite record");
            favorites += 1;
        }
    }
    assert_eq!(favorites, 11);

    let report = continuation(&fixture).run(fixture.root).expect("reported");
    assert_eq!(report.projection.songs, 87);
    assert_eq!(report.projection.favorites, 11);
    assert_eq!(
        report.projection.playlists, 0,
        "no playlist was ever written"
    );
    assert!(report.control_plane.healed(), "no manifest → healed");

    let after = fixture.all_songs();
    assert_eq!(after.len(), 87);
    assert_eq!(after.iter().filter(|song| song.favorite()).count(), 3);
    // Continuation establishes identity, not availability: every projected
    // song waits for the scan to confirm its file (spec 对象资料与媒体不一致
    // — a record whose file is gone must show as unavailable, never vanish).
    assert!(after
        .iter()
        .all(|song| song.availability() == SongAvailability::Missing));
    // Every identity came from a record: nothing was re-minted.
    let mut after_ids: Vec<String> = after.iter().map(|song| song.id().to_string()).collect();
    let mut expected: Vec<String> = recorded.iter().map(ToString::to_string).collect();
    after_ids.sort();
    expected.sort();
    assert_eq!(after_ids, expected);

    // Repeating the open changes nothing (spec 重复接续结果稳定).
    let again = continuation(&fixture).run(fixture.root).expect("reported");
    assert_eq!(again.projection, report.projection);
    assert_eq!(fixture.all_songs().len(), 87);
}
