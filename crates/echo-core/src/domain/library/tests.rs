use std::collections::BTreeMap;

use super::super::ids::Revision;
use super::*;

// ── ControlPath ─────────────────────────────────────────────────────

#[test]
fn control_path_accepts_manifest_and_records() {
    assert_eq!(
        ControlPath::new("echo/manifest.json").unwrap().as_str(),
        "echo/manifest.json"
    );
    assert!(ControlPath::new("echo/records/songs/ab/uuid.json").is_ok());
    assert!(ControlPath::new("echo/records/playlists/cd/uuid.json").is_ok());
}

#[test]
fn control_path_rejects_staging() {
    assert!(ControlPath::new("echo/tmp/op-123/file.json").is_err());
    assert!(ControlPath::new("echo/tmp").is_err());
}

#[test]
fn control_path_rejects_media() {
    assert!(ControlPath::new("media/artist/song.mp3").is_err());
}

#[test]
fn control_path_rejects_absolute_and_escape() {
    assert!(ControlPath::new("/echo/manifest.json").is_err());
    assert!(ControlPath::new("echo/../secret.json").is_err());
    assert!(ControlPath::new("echo/./manifest.json").is_err());
}

#[test]
fn control_path_rejects_bare_echo() {
    assert!(ControlPath::new("echo").is_err());
}

#[test]
fn control_path_rejects_empty_and_nul() {
    assert!(ControlPath::new("").is_err());
    assert!(ControlPath::new("echo/\0secret").is_err());
}

#[test]
fn control_path_display_round_trips() {
    let p = ControlPath::new("echo/records/songs/ab/uuid.json").unwrap();
    assert_eq!(p.to_string(), "echo/records/songs/ab/uuid.json");
    assert_eq!(ControlPath::new(&p.to_string()).unwrap(), p);
}

// ── LibraryRelativePath ─────────────────────────────────────────────

#[test]
fn media_path_accepts_valid_paths() {
    let p = LibraryRelativePath::new("media/周杰伦/周杰伦 - 晴天.flac").unwrap();
    assert_eq!(p.as_str(), "media/周杰伦/周杰伦 - 晴天.flac");
    assert!(LibraryRelativePath::new("media/a/b.mp3").is_ok());
}

#[test]
fn media_path_accepts_legacy_root_relative_paths() {
    assert!(LibraryRelativePath::new("artwork/cover.jpg").is_ok());
    assert!(LibraryRelativePath::new("tmp/file").is_ok());
}

#[test]
fn media_path_rejects_control_surface() {
    assert!(LibraryRelativePath::new("echo/manifest.json").is_err());
    assert!(LibraryRelativePath::new("ECHO/records/song.json").is_err());
}

#[test]
fn media_path_rejects_absolute_escape_and_controls() {
    assert!(LibraryRelativePath::new("/media/song.mp3").is_err());
    assert!(LibraryRelativePath::new("media/../etc/passwd").is_err());
    assert!(LibraryRelativePath::new("media/\0song").is_err());
}

#[test]
fn media_path_rejects_windows_reserved_and_unportable_components() {
    for path in [
        "media/CON/song.flac",
        "media/aux.txt/song.flac",
        "media/Lpt9/song.flac",
        "media/COM¹/song.flac",
        "media/artist./song.flac",
        "media/artist /song.flac",
        "media/a:b/song.flac",
    ] {
        assert!(LibraryRelativePath::new(path).is_err(), "accepted {path}");
    }
    assert!(LibraryRelativePath::new("media/CONCERT/song.flac").is_ok());
    assert!(LibraryRelativePath::new("media/周杰伦/song.flac").is_ok());
}

#[test]
fn media_path_display_round_trips() {
    let p = LibraryRelativePath::new("media/artist/artist - song.flac").unwrap();
    assert_eq!(p.as_str(), "media/artist/artist - song.flac");
    assert_eq!(LibraryRelativePath::new(p.as_str()).unwrap(), p);
}

// ── Layout validation ───────────────────────────────────────────────

#[test]
fn layout_validator_accepts_media_prefix() {
    assert!(DefaultLayoutValidator::validate_layout_rules("media/周杰伦").is_ok());
}

#[test]
fn layout_validator_rejects_echo_under_media() {
    assert!(DefaultLayoutValidator::validate_layout_rules("echo/").is_err());
    assert!(DefaultLayoutValidator::validate_layout_rules("echo/tmp/op").is_err());
}

#[test]
fn layout_validator_accepts_empty_root() {
    assert!(DefaultLayoutValidator::validate_layout_rules("").is_ok());
}

// ── LibraryId ───────────────────────────────────────────────────────

#[test]
fn library_id_round_trips_through_string() {
    let id = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let text = id.to_string();
    let parsed: LibraryId = text.parse().unwrap();
    assert_eq!(id, parsed);
}

#[test]
fn library_id_rejects_invalid_uuid() {
    assert!("not-a-uuid".parse::<LibraryId>().is_err());
}

// ── DeviceId ────────────────────────────────────────────────────────

#[test]
fn device_id_new_produces_distinct_values() {
    let a = DeviceId::new();
    let b = DeviceId::new();
    assert_ne!(a, b);
}

#[test]
fn device_id_round_trips_through_string() {
    let id = DeviceId::new();
    let text = id.to_string();
    let parsed: DeviceId = text.parse().unwrap();
    assert_eq!(id, parsed);
}

// ── HybridLogicalClock ──────────────────────────────────────────────

#[test]
fn hlc_ordering_is_lexicographic() {
    let earlier = HybridLogicalClock::new(1000, 0);
    let later = HybridLogicalClock::new(1001, 0);
    assert!(earlier < later);

    let same_wall_higher_counter = HybridLogicalClock::new(1000, 1);
    assert!(earlier < same_wall_higher_counter);
}

#[test]
fn hlc_round_trips_through_string() {
    let hlc = HybridLogicalClock::new(1_700_000_000, 42);
    let text = hlc.to_string();
    assert_eq!(text, "1700000000:42");
    let parsed: HybridLogicalClock = text.parse().unwrap();
    assert_eq!(hlc, parsed);
}

#[test]
fn hlc_rejects_malformed() {
    assert!("not-a-number:0".parse::<HybridLogicalClock>().is_err());
    assert!("1000:".parse::<HybridLogicalClock>().is_err());
    assert!("".parse::<HybridLogicalClock>().is_err());
}

#[test]
fn hlc_default_is_zero() {
    assert_eq!(HybridLogicalClock::default(), HybridLogicalClock::new(0, 0));
}

#[test]
fn next_hlc_advances_wall_then_counter_and_never_regresses() {
    let mut hlc = HybridLogicalClock::default();
    // Advancing wall clock resets the counter.
    hlc = next_hlc(hlc, 1_700_000_000);
    assert_eq!(hlc, HybridLogicalClock::new(1_700_000_000, 0));
    // Same wall second → counter bumps.
    let bumped = next_hlc(hlc, 1_700_000_000);
    assert_eq!(bumped, HybridLogicalClock::new(1_700_000_000, 1));
    // A later wall second again resets the counter.
    let later = next_hlc(bumped, 1_700_000_001);
    assert_eq!(later, HybridLogicalClock::new(1_700_000_001, 0));
    // A wall clock that stepped *backward* never regresses the HLC.
    let stepped_back = next_hlc(later, 1_000);
    assert!(
        stepped_back > later,
        "a backward wall step must not regress the HLC"
    );
}

// ── RecordKind ──────────────────────────────────────────────────────

#[test]
fn record_kind_dir_names_are_stable() {
    assert_eq!(RecordKind::Song.dir_name(), "songs");
    assert_eq!(RecordKind::Favorite.dir_name(), "favorites");
    assert_eq!(RecordKind::Playlist.dir_name(), "playlists");
    assert_eq!(RecordKind::PlaylistItem.dir_name(), "playlist-items");
    assert_eq!(RecordKind::Override.dir_name(), "overrides");
    assert_eq!(RecordKind::Tombstone.dir_name(), "tombstones");
}

#[test]
fn record_kind_operations_match_spec() {
    assert_eq!(
        RecordKind::Song.valid_operations(),
        &[TraitOperation::Upsert]
    );
    assert_eq!(
        RecordKind::Playlist.valid_operations(),
        &[TraitOperation::Upsert, TraitOperation::Delete]
    );
    assert_eq!(
        RecordKind::Tombstone.valid_operations(),
        &[TraitOperation::Tombstone]
    );
}

#[test]
fn record_kind_display_matches_dir_name() {
    assert_eq!(RecordKind::Song.to_string(), "songs");
    assert_eq!(RecordKind::Tombstone.to_string(), "tombstones");
    assert_eq!(RecordKind::PlayStats.to_string(), "play-stats");
}

#[test]
fn record_kind_all_covers_every_variant_once() {
    // The projection pass and the layout gate both enumerate `ALL`; a
    // variant missing here is a kind whose records are silently dropped.
    assert_eq!(RecordKind::ALL.len(), 7);
    for kind in RecordKind::ALL {
        assert!(
            RecordKind::ALL
                .iter()
                .filter(|other| *other == kind)
                .count()
                == 1,
            "{kind} must appear exactly once"
        );
        assert!(!kind.dir_name().is_empty());
    }
}

#[test]
fn play_stats_counts_merge_additively_and_idempotently() {
    let device_a = DeviceId::new();
    let device_b = DeviceId::new();
    let base = PlayStatsRecord {
        song_uuid: uuid::Uuid::new_v4(),
        revision: Revision::INITIAL,
        updated_by_device_id: device_a,
        hlc: HybridLogicalClock::new(1_700_000_000, 0),
        by_device: BTreeMap::new(),
    };
    assert_eq!(base.total(), 0);

    // Two devices each play once → the merged count is 2 (never 1: an
    // LWW counter would have dropped one device's play).
    let merged = base
        .with_device_count(device_a, 1)
        .with_device_count(device_b, 1);
    assert_eq!(merged.total(), 2);

    // Re-writing a device's bucket is idempotent, never a doubling.
    let again = merged.with_device_count(device_a, 1);
    assert_eq!(again.total(), 2, "same device re-write does not double");
    // A monotone counter is never lowered by a stale merge.
    let stale = again.clone().with_device_count(device_b, 0);
    assert_eq!(stale.total(), 2, "a lower count never shrinks the total");
    // A real new play on one device still adds.
    assert_eq!(again.with_device_count(device_b, 2).total(), 3);
}

#[test]
fn play_stats_record_round_trips_through_json() {
    let device = DeviceId::new();
    let song_uuid = uuid::Uuid::new_v4();
    let record = PortableRecord::PlayStats(PlayStatsRecord {
        song_uuid,
        revision: Revision::from_u64(4),
        updated_by_device_id: device,
        hlc: HybridLogicalClock::new(1_700_000_000, 3),
        by_device: BTreeMap::from([(device.as_uuid(), 7)]),
    });
    assert_eq!(record.kind(), RecordKind::PlayStats);
    assert_eq!(record.object_uuid(), song_uuid);
    let json = record.to_canonical_json().unwrap();
    let back: PortableRecord = serde_json::from_str(&json).unwrap();
    assert_eq!(record, back);
    if let PortableRecord::PlayStats(stats) = &back {
        assert_eq!(stats.total(), 7);
    } else {
        panic!("expected PlayStats variant");
    }
}

// ── PortableRecord ──────────────────────────────────────────────────

#[test]
fn portable_record_kind_and_uuid_for_songs() {
    let uuid = uuid::Uuid::new_v4();
    let rec = PortableRecord::Song(SongRecord {
        song_uuid: uuid,
        revision: Revision::INITIAL,
        updated_by_device_id: DeviceId::new(),
        hlc: HybridLogicalClock::default(),
        media_path: LibraryRelativePath::new("media/周杰伦/周杰伦 - 晴天.flac").unwrap(),
        content_hash: "abc123".to_owned(),
        added_at: 1_700_001_002_003,
        title: Some("晴天".to_owned()),
        artist: Some("周杰伦".to_owned()),
        album: None,
    });
    assert_eq!(rec.kind(), RecordKind::Song);
    assert_eq!(rec.object_uuid(), uuid);
    assert!(!rec.is_tombstone());
    assert_eq!(rec.owner_name(), None);
}

#[test]
fn portable_record_kind_and_uuid_for_playlist() {
    let pl_uuid = uuid::Uuid::new_v4();
    let rec = PortableRecord::Playlist(PlaylistRecord {
        playlist_uuid: pl_uuid,
        revision: Revision::INITIAL,
        updated_by_device_id: DeviceId::new(),
        hlc: HybridLogicalClock::default(),
        display_name: "通勤路上".to_owned(),
        created_at: None,
        cover_asset: None,
    });
    assert_eq!(rec.kind(), RecordKind::Playlist);
    assert_eq!(rec.object_uuid(), pl_uuid);
    assert_eq!(rec.owner_name(), Some("通勤路上"));
}

#[test]
fn portable_record_tombstone_is_flagged() {
    let obj_uuid = uuid::Uuid::new_v4();
    let rec = PortableRecord::Tombstone(TombstoneRecord {
        object_uuid: obj_uuid,
        deleted_kind: RecordKind::Song,
        revision: Revision::from_u64(3),
        updated_by_device_id: DeviceId::new(),
        hlc: HybridLogicalClock::new(1000, 1),
    });
    assert!(rec.is_tombstone());
    assert_eq!(rec.kind(), RecordKind::Tombstone);
    assert_eq!(rec.object_uuid(), obj_uuid);
}

#[test]
fn portable_record_favorite_tracks_song() {
    let song_uuid = uuid::Uuid::new_v4();
    let rec = PortableRecord::Favorite(FavoriteRecord {
        song_uuid,
        revision: Revision::from_u64(2),
        updated_by_device_id: DeviceId::new(),
        hlc: HybridLogicalClock::new(2000, 0),
        is_favorite: true,
    });
    assert_eq!(rec.kind(), RecordKind::Favorite);
    assert_eq!(rec.object_uuid(), song_uuid);
    assert_eq!(rec.owner_uuid(), song_uuid);
}

#[test]
fn portable_record_playlist_item_references_song_and_playlist() {
    let item_uuid = uuid::Uuid::new_v4();
    let pl_uuid = uuid::Uuid::new_v4();
    let song_uuid = uuid::Uuid::new_v4();
    let rec = PortableRecord::PlaylistItem(PlaylistItemRecord {
        item_uuid,
        revision: Revision::INITIAL,
        updated_by_device_id: DeviceId::new(),
        hlc: HybridLogicalClock::default(),
        playlist_uuid: pl_uuid,
        song_uuid,
        position: 0,
        added_at: None,
    });
    assert_eq!(rec.kind(), RecordKind::PlaylistItem);
    assert_eq!(rec.object_uuid(), item_uuid);
    // playlist_uuid and song_uuid are accessible through the inner record.
    if let PortableRecord::PlaylistItem(inner) = &rec {
        assert_eq!(inner.playlist_uuid, pl_uuid);
        assert_eq!(inner.song_uuid, song_uuid);
    } else {
        panic!("expected PlaylistItem variant");
    }
}

#[test]
fn portable_record_override_preserves_song_identity() {
    let song_uuid = uuid::Uuid::new_v4();
    let rec = PortableRecord::Override(OverrideRecord {
        song_uuid,
        revision: Revision::from_u64(5),
        updated_by_device_id: DeviceId::new(),
        hlc: HybridLogicalClock::new(3000, 2),
        title: Some("修改标题".to_owned()),
        artist: None,
        album: None,
        lyrics_text: Some("自定义歌词".to_owned()),
    });
    assert_eq!(rec.kind(), RecordKind::Override);
    assert_eq!(rec.object_uuid(), song_uuid);
    assert_eq!(rec.revision(), Revision::from_u64(5));
}

// ── Manifest ────────────────────────────────────────────────────────

#[test]
fn manifest_format_version_is_current() {
    let lib_id = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let manifest = LibraryManifest::new(lib_id, "0.1.0");
    assert_eq!(manifest.format_version, CURRENT_FORMAT_VERSION);
    assert!(manifest.is_compatible());
}

#[test]
fn manifest_incompatible_version_fails_check() {
    let manifest = LibraryManifest {
        format_version: 0,
        library_id: LibraryId::from_uuid(uuid::Uuid::new_v4()),
        written_by_app_version: "0.1.0".to_owned(),
    };
    assert!(!manifest.is_compatible());
}

#[test]
fn manifest_serializes_round_trips() {
    let lib_id = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let manifest = LibraryManifest::new(lib_id, "0.1.0");
    let json = serde_json::to_string(&manifest).unwrap();
    let back: LibraryManifest = serde_json::from_str(&json).unwrap();
    assert_eq!(manifest, back);
}

// ── Content hashing ─────────────────────────────────────────────────

#[test]
fn portable_record_content_hash_is_deterministic() {
    let song_uuid = uuid::Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
    let dev =
        DeviceId::from_uuid(uuid::Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap());
    let rec = SongRecord {
        song_uuid,
        revision: Revision::INITIAL,
        updated_by_device_id: dev,
        hlc: HybridLogicalClock::new(1000, 0),
        media_path: LibraryRelativePath::new("media/artist/song.flac").unwrap(),
        content_hash: "abc123".to_owned(),
        added_at: 1_700_001_002_003,
        title: Some("测试".to_owned()),
        artist: None,
        album: None,
    };
    let hash1 = rec.content_hash().unwrap();
    let hash2 = rec.content_hash().unwrap();
    assert_eq!(hash1, hash2);
    assert_eq!(hash1.len(), 64, "BLAKE3 hex digest is 64 chars");

    // Changing any field changes the hash.
    let mut rec2 = rec;
    rec2.title = Some("测试二".to_owned());
    let hash3 = rec2.content_hash().unwrap();
    assert_ne!(hash1, hash3);
}

#[test]
fn portable_record_json_round_trips() {
    let song_uuid = uuid::Uuid::new_v4();
    let dev = DeviceId::new();
    let rec = PortableRecord::Song(SongRecord {
        song_uuid,
        revision: Revision::from_u64(3),
        updated_by_device_id: dev,
        hlc: HybridLogicalClock::new(1_700_000_000, 1),
        media_path: LibraryRelativePath::new("media/周杰伦/周杰伦 - 晴天.flac").unwrap(),
        content_hash: "deadbeef".to_owned(),
        added_at: 1_700_001_002_003,
        title: Some("晴天".to_owned()),
        artist: Some("周杰伦".to_owned()),
        album: Some("叶惠美".to_owned()),
    });
    let json = rec.to_canonical_json().unwrap();
    let back: PortableRecord = serde_json::from_str(&json).unwrap();
    assert_eq!(rec, back);
}

#[test]
fn song_record_without_added_at_reads_back_as_zero() {
    // Records written before `added_at` existed carry no such field. The
    // `#[serde(default)]` must read them back as 0 — exactly the value the
    // historical `Song::new` reconstruction produced — rather than fail.
    let rec = PortableRecord::Song(SongRecord {
        song_uuid: uuid::Uuid::new_v4(),
        revision: Revision::INITIAL,
        updated_by_device_id: DeviceId::new(),
        hlc: HybridLogicalClock::new(1_700_000_000, 0),
        media_path: LibraryRelativePath::new("media/周杰伦/周杰伦 - 晴天.flac").unwrap(),
        content_hash: "abc123".to_owned(),
        added_at: 1_700_001_002_003,
        title: Some("晴天".to_owned()),
        artist: Some("周杰伦".to_owned()),
        album: None,
    });
    let mut value = serde_json::to_value(&rec).expect("serialize");
    value
        .as_object_mut()
        .expect("record object")
        .remove("added_at");
    let legacy: PortableRecord = serde_json::from_value(value).expect("read legacy record");
    let PortableRecord::Song(legacy_song) = legacy else {
        panic!("expected a song record");
    };
    assert_eq!(
        legacy_song.added_at, 0,
        "missing added_at falls back to the historical zero order"
    );
    assert_eq!(
        legacy_song.song_uuid,
        rec.object_uuid(),
        "identity survives"
    );
    assert_eq!(legacy_song.title, Some("晴天".to_owned()));
}

// ── Library manifest path constant ──────────────────────────────────

const V1_FAVORITE_FIXTURE: &str = r#"{
    "type":"favorite",
    "song_uuid":"00000000-0000-0000-0000-000000000001",
    "revision":7,
    "updated_by_device_id":"00000000-0000-0000-0000-000000000002",
    "hlc":{"wall_secs":1700000000,"counter":2},
    "is_favorite":true
}"#;

const V2_TOMBSTONE_FIXTURE: &str = r#"{
    "format_version":2,
    "record":{
        "type":"tombstone",
        "object_uuid":"00000000-0000-0000-0000-000000000001",
        "deleted_kind":"song",
        "revision":8,
        "updated_by_device_id":"00000000-0000-0000-0000-000000000002",
        "hlc":{"wall_secs":1700000001,"counter":0}
    },
    "sync":{
        "binding":{
            "library_id":"00000000-0000-0000-0000-000000000003",
            "authority_id":"00000000-0000-0000-0000-000000000004",
            "epoch":3
        },
        "object_version":42,
        "aliases":[{
            "kind":"song",
            "alias_uuid":"00000000-0000-0000-0000-000000000005",
            "canonical_uuid":"00000000-0000-0000-0000-000000000001"
        }]
    }
}"#;

#[test]
fn portable_v1_fixture_upgrades_without_fabricating_a_confirmation() {
    let legacy: PortableRecordDocument = serde_json::from_str(V1_FAVORITE_FIXTURE).unwrap();
    assert!(matches!(legacy, PortableRecordDocument::V1(_)));
    assert!(legacy.sync_metadata().is_none());
    let upgraded = PortableRecordDocument::V2(
        PortableRecordEnvelope::new(legacy.into_record(), None).unwrap(),
    );
    let json = upgraded.to_canonical_json().unwrap();
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["format_version"], 2);
    assert!(value.get("sync").is_none());
    assert_eq!(
        value["record"],
        serde_json::from_str::<serde_json::Value>(V1_FAVORITE_FIXTURE).unwrap()
    );
    assert_eq!(
        serde_json::from_str::<PortableRecordDocument>(&json).unwrap(),
        upgraded
    );
}

#[test]
fn portable_v2_fixture_retains_authority_typed_tombstone_and_historical_alias() {
    let document: PortableRecordDocument = serde_json::from_str(V2_TOMBSTONE_FIXTURE).unwrap();
    let metadata = document.sync_metadata().unwrap();
    assert_eq!(metadata.binding.epoch.as_u64(), 3);
    assert_eq!(metadata.object_version.commit_seq().as_u64(), 42);
    assert_eq!(
        document.record().object_key().unwrap(),
        metadata.aliases[0].canonical_key()
    );
    assert!(document.record().is_tombstone());
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&document.to_canonical_json().unwrap()).unwrap(),
        serde_json::from_str::<serde_json::Value>(V2_TOMBSTONE_FIXTURE).unwrap()
    );
    // A favorite sharing the exact song UUID has a different typed key.
    let favorite: PortableRecord = serde_json::from_str(V1_FAVORITE_FIXTURE).unwrap();
    assert_ne!(
        favorite.object_key().unwrap(),
        document.record().object_key().unwrap()
    );
}

#[test]
fn portable_v2_decoder_rejects_unknown_versions_and_invalid_typed_identities() {
    let fixture: serde_json::Value = serde_json::from_str(V2_TOMBSTONE_FIXTURE).unwrap();
    let invalid_changes = [
        ("/format_version", serde_json::json!(3)),
        ("/format_version", serde_json::json!(1)),
        ("/record/deleted_kind", serde_json::json!("tombstone")),
        ("/sync/binding/epoch", serde_json::json!(0)),
        ("/sync/aliases/0/kind", serde_json::json!("favorite")),
        ("/sync/aliases/0/kind", serde_json::json!("playlist_item")),
        (
            "/sync/aliases/0/canonical_uuid",
            serde_json::json!("00000000-0000-0000-0000-000000000006"),
        ),
        (
            "/sync/aliases/0/alias_uuid",
            serde_json::json!("00000000-0000-0000-0000-000000000001"),
        ),
    ];
    for (pointer, invalid_value) in invalid_changes {
        let mut invalid = fixture.clone();
        *invalid.pointer_mut(pointer).unwrap() = invalid_value;
        assert!(
            serde_json::from_value::<PortableRecordDocument>(invalid).is_err(),
            "accepted invalid {pointer}"
        );
    }
    // Even v1-shaped extra fields cannot bypass a claimed future version.
    let mut future: serde_json::Value = serde_json::from_str(V1_FAVORITE_FIXTURE).unwrap();
    future["format_version"] = serde_json::json!(3);
    assert!(serde_json::from_value::<PortableRecordDocument>(future).is_err());
}

#[test]
fn portable_v2_member_alias_is_scoped_to_the_canonical_members_playlist() {
    let id = uuid::Uuid::from_u128;
    let record = PortableRecord::PlaylistItem(PlaylistItemRecord {
        item_uuid: id(1),
        revision: Revision::INITIAL,
        updated_by_device_id: DeviceId::from_uuid(id(2)),
        hlc: HybridLogicalClock::default(),
        playlist_uuid: id(3),
        song_uuid: id(4),
        position: 5,
        added_at: None,
    });
    let confirmed: PortableRecordDocument = serde_json::from_str(V2_TOMBSTONE_FIXTURE).unwrap();
    let mut metadata = confirmed.sync_metadata().unwrap().clone();
    metadata.aliases = vec![PortableAlias::PlaylistItem {
        playlist_uuid: id(3),
        alias_uuid: id(6),
        canonical_uuid: id(1),
    }];
    let envelope = PortableRecordEnvelope::new(record.clone(), Some(metadata.clone())).unwrap();
    assert_eq!(
        serde_json::from_str::<PortableRecordEnvelope>(&envelope.to_canonical_json().unwrap())
            .unwrap(),
        envelope
    );
    metadata.aliases.push(metadata.aliases[0]);
    assert!(PortableRecordEnvelope::new(record.clone(), Some(metadata.clone())).is_err());
    metadata.aliases = vec![PortableAlias::PlaylistItem {
        playlist_uuid: id(7),
        alias_uuid: id(6),
        canonical_uuid: id(1),
    }];
    assert!(PortableRecordEnvelope::new(record, Some(metadata)).is_err());
}

#[test]
fn manifest_v2_explicitly_reads_v1_but_refuses_invalid_and_future_versions() {
    let mut manifest = LibraryManifest::new(LibraryId::from_uuid(uuid::Uuid::from_u128(1)), "test");
    assert_eq!(manifest.format_version, 2);
    assert!(manifest.is_compatible());
    manifest.format_version = 1;
    assert!(manifest.is_compatible());
    for unsupported in [0, 3, u64::MAX] {
        manifest.format_version = unsupported;
        assert!(!manifest.is_compatible());
    }
}

#[test]
fn manifest_path_is_valid_control_path() {
    assert!(ControlPath::new(MANIFEST_PATH).is_ok());
}

#[test]
fn records_root_is_valid_control_path_prefix() {
    assert!(ControlPath::new("echo/records").is_ok());
}

// ── Unicode path support ────────────────────────────────────────────

#[test]
fn media_path_accepts_unicode_artist_and_title() {
    let p = LibraryRelativePath::new("media/周杰伦/周杰伦 - 晴天.flac").unwrap();
    assert_eq!(p.as_str(), "media/周杰伦/周杰伦 - 晴天.flac");
}

#[test]
fn control_path_accepts_unicode() {
    let p = ControlPath::new("echo/records/playlists/ab/通勤路上.json").unwrap();
    assert_eq!(p.as_str(), "echo/records/playlists/ab/通勤路上.json");
}

// ── Component byte limits ───────────────────────────────────────────

#[test]
fn media_path_respects_component_byte_limit() {
    // A very long component (> 4096 bytes) should be rejected.
    let long = "a".repeat(4097);
    let path = format!("media/{long}/song.flac");
    assert!(
        LibraryRelativePath::new(&path).is_err(),
        "should reject overlong component"
    );
}
