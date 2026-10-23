//! Synthetic recovery evidence and full, typed state comparisons. No real user
//! paths, media, SQLite files or credentials are copied into these fixtures.

use std::collections::BTreeMap;

use crate::domain::ids::Revision;
use crate::domain::library::durability::{DurableResource, DurableStateChange, LibraryReplay};
use crate::domain::library::{
    DeviceId, FavoriteRecord, HybridLogicalClock, LibraryRelativePath, PlaylistItemRecord,
    PlaylistRecord, PortableAlias, PortableRecord, PortableRecordEnvelope, SongRecord,
};

/// Compare complete after-images, including every identity, timestamp, explicit
/// false, resource reference, override, bucket, typed tombstone and sync fact.
/// Keys are stable serialized typed identities; neither counts nor Debug text
/// can disguise a lost relationship. Differences name the exact affected key.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CanonicalLibraryState {
    pub records: BTreeMap<String, PortableRecordEnvelope>,
    pub state: BTreeMap<String, DurableStateChange>,
    pub resources: BTreeMap<String, DurableResource>,
}

impl CanonicalLibraryState {
    pub fn from_parts(
        records: &[PortableRecordEnvelope],
        state: &[DurableStateChange],
        resources: &[DurableResource],
    ) -> Self {
        let mut snapshot = Self::default();
        snapshot.apply(records, state, resources);
        snapshot
    }

    pub fn from_replay(replay: &LibraryReplay) -> Self {
        let mut snapshot = Self::default();
        if let Some(image) = &replay.checkpoint {
            snapshot.apply(&image.records, &image.state, &image.resources);
        }
        for commit in &replay.commits {
            snapshot.apply(
                &commit.batch.records,
                &commit.batch.state,
                &commit.batch.resources,
            );
        }
        snapshot
    }

    fn apply(
        &mut self,
        records: &[PortableRecordEnvelope],
        state: &[DurableStateChange],
        resources: &[DurableResource],
    ) {
        for envelope in records {
            let key = serde_json::to_string(
                &envelope
                    .record()
                    .object_key()
                    .expect("typed fixture record"),
            )
            .unwrap();
            self.records.insert(key, envelope.clone());
        }
        for fact in state {
            self.state.insert(
                format!("{}:{}", fact.kind.directory(), fact.id),
                fact.clone(),
            );
        }
        for resource in resources {
            self.resources
                .insert(resource.relative_path.clone(), resource.clone());
        }
    }

    pub fn assert_matches(&self, actual: &Self) {
        assert_eq!(
            self.records, actual.records,
            "portable object after-images differ"
        );
        assert_eq!(self.state, actual.state, "private recovery facts differ");
        assert_eq!(
            self.resources, actual.resources,
            "resource location/hash/length differ"
        );
    }
}

/// The shape reported in issue #72: fifteen indexed songs but only two song
/// records; eleven favorites and three members reference older identities, and
/// no playlist definition reached the portable directory. `database` and
/// `aliases` are evidence available only in the rescuable variant. The orphan
/// variant intentionally provides no way to infer a name or merge identities.
pub struct Issue72Fixture {
    pub media: BTreeMap<String, Vec<u8>>,
    pub database: Vec<PortableRecord>,
    pub portable: Vec<PortableRecord>,
    pub aliases: Vec<PortableAlias>,
}

impl Issue72Fixture {
    pub fn with_database_evidence() -> Self {
        Self::build(true)
    }
    pub fn orphaned() -> Self {
        Self::build(false)
    }

    fn build(proven: bool) -> Self {
        let device = DeviceId::from_uuid(uuid::Uuid::from_u128(1));
        let hlc = HybridLogicalClock::new(1_700_000_000, 0);
        let playlist_uuid = uuid::Uuid::from_u128(500);
        let mut media = BTreeMap::new();
        let mut database = Vec::new();
        let mut portable = Vec::new();
        let mut aliases = Vec::new();
        for index in 0..15u128 {
            let id = uuid::Uuid::from_u128(100 + index);
            let old_id = uuid::Uuid::from_u128(1_000 + index);
            let path = format!("media/合成歌手/歌曲-{index:02}.flac");
            let bytes = format!("SYNTHETIC AUDIO {index}").into_bytes();
            let song = SongRecord {
                song_uuid: id,
                revision: Revision::INITIAL,
                updated_by_device_id: device,
                hlc,
                media_path: LibraryRelativePath::new(&path).unwrap(),
                content_hash: blake3::hash(&bytes).to_hex().to_string(),
                added_at: 1_700_000_000_000 + u64::try_from(index).unwrap(),
                title: Some(format!("歌曲-{index:02}")),
                artist: Some("合成歌手".into()),
                album: None,
            };
            media.insert(path, bytes);
            if index < 2 {
                portable.push(PortableRecord::Song(song.clone()));
            }
            database.push(PortableRecord::Song(song));
            if index < 11 {
                let favorite = FavoriteRecord {
                    song_uuid: id,
                    revision: Revision::INITIAL,
                    updated_by_device_id: device,
                    hlc,
                    is_favorite: true,
                };
                database.push(PortableRecord::Favorite(favorite.clone()));
                portable.push(PortableRecord::Favorite(FavoriteRecord {
                    song_uuid: old_id,
                    ..favorite
                }));
            }
            if index < 3 {
                let member = PlaylistItemRecord {
                    item_uuid: uuid::Uuid::from_u128(700 + index),
                    revision: Revision::INITIAL,
                    updated_by_device_id: device,
                    hlc,
                    playlist_uuid,
                    song_uuid: id,
                    position: u64::try_from(index * 10).unwrap(),
                    added_at: Some(1_700_000_001_000 + u64::try_from(index).unwrap()),
                };
                database.push(PortableRecord::PlaylistItem(member.clone()));
                portable.push(PortableRecord::PlaylistItem(PlaylistItemRecord {
                    song_uuid: old_id,
                    ..member
                }));
            }
            if proven {
                aliases.push(PortableAlias::Song {
                    alias_uuid: old_id,
                    canonical_uuid: id,
                });
            }
        }
        database.push(PortableRecord::Playlist(PlaylistRecord {
            playlist_uuid,
            revision: Revision::INITIAL,
            updated_by_device_id: device,
            hlc,
            display_name: "合成历史歌单".into(),
            created_at: Some(1_699_999_000_000),
            cover_asset: None,
        }));
        if !proven {
            database.clear();
        }
        Self {
            media,
            database,
            portable,
            aliases,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::continuation::ContinueFromRecords;
    use crate::application::ports::ControlPlanePort;
    use crate::application::testing::ScanFixture;
    use crate::domain::library::RecordKind;

    #[test]
    fn library_durability_issue72_fixture_reproduces_legacy_missing_relationships() {
        for evidence in [
            Issue72Fixture::with_database_evidence(),
            Issue72Fixture::orphaned(),
        ] {
            assert_eq!(evidence.media.len(), 15);
            for (kind, count) in [
                (RecordKind::Song, 2),
                (RecordKind::Favorite, 11),
                (RecordKind::PlaylistItem, 3),
                (RecordKind::Playlist, 0),
            ] {
                assert_eq!(
                    evidence
                        .portable
                        .iter()
                        .filter(|r| r.kind() == kind)
                        .count(),
                    count
                );
            }
            let fixture = ScanFixture::new();
            for record in &evidence.portable {
                fixture.control.write_record(fixture.root, record).unwrap();
            }
            let report = ContinueFromRecords::new(
                &fixture.control,
                fixture.deps.songs.as_ref(),
                fixture.deps.playlists.as_ref(),
                fixture.deps.uow.as_ref(),
            )
            .run(fixture.root)
            .unwrap();
            assert_eq!(report.projection.songs, 2);
            assert_eq!(report.projection.favorites, 0);
            assert_eq!(report.projection.playlists, 0);
            assert_eq!(report.projection.invalid, 14);
            let encoded = |records: &[PortableRecord]| {
                records
                    .iter()
                    .map(|r| serde_json::to_string(r).unwrap())
                    .collect::<std::collections::BTreeSet<_>>()
            };
            assert_eq!(
                encoded(&fixture.control.records_of(fixture.root)),
                encoded(&evidence.portable),
                "legacy continuation must preserve unresolved evidence"
            );
        }
    }

    #[test]
    fn library_durability_issue72_proven_and_orphan_fixtures_have_identical_portable_evidence() {
        let proven = Issue72Fixture::with_database_evidence();
        let orphan = Issue72Fixture::orphaned();
        assert_eq!(proven.portable, orphan.portable);
        assert_eq!(proven.media, orphan.media);
        assert_eq!(
            proven
                .database
                .iter()
                .filter(|r| r.kind() == RecordKind::Song)
                .count(),
            15
        );
        assert_eq!(proven.aliases.len(), 15);
        assert!(orphan.database.is_empty());
        assert!(orphan.aliases.is_empty());
    }

    #[test]
    fn library_durability_comparator_detects_equal_count_changes_to_user_facts() {
        use crate::domain::library::durability::StateKind;
        use crate::domain::library::{
            OverrideRecord, PlayStatsRecord, PortableAssetReference, TombstoneRecord,
        };
        let mut records = Issue72Fixture::with_database_evidence().database;
        let song = uuid::Uuid::from_u128(114);
        let device = DeviceId::from_uuid(uuid::Uuid::from_u128(1));
        let hlc = HybridLogicalClock::new(1_700_000_000, 0);
        records.extend([
            PortableRecord::Favorite(FavoriteRecord {
                song_uuid: song,
                revision: Revision::INITIAL,
                updated_by_device_id: device,
                hlc,
                is_favorite: false,
            }),
            PortableRecord::Override(OverrideRecord {
                song_uuid: song,
                revision: Revision::INITIAL,
                updated_by_device_id: device,
                hlc,
                title: Some("手工标题".into()),
                artist: None,
                album: None,
                lyrics_text: Some("[00:01.00]手工歌词".into()),
            }),
            PortableRecord::PlayStats(PlayStatsRecord {
                song_uuid: song,
                revision: Revision::INITIAL,
                updated_by_device_id: device,
                hlc,
                by_device: BTreeMap::from([(device.as_uuid(), 7)]),
            }),
            PortableRecord::Playlist(PlaylistRecord {
                playlist_uuid: uuid::Uuid::from_u128(600),
                revision: Revision::INITIAL,
                updated_by_device_id: device,
                hlc,
                display_name: "空歌单".into(),
                created_at: Some(1_699_999_000_000),
                cover_asset: Some(PortableAssetReference {
                    content_hash: blake3::hash(b"original cover").to_hex().to_string(),
                    byte_len: 14,
                    mime: "image/png".into(),
                }),
            }),
            PortableRecord::Tombstone(TombstoneRecord {
                object_uuid: uuid::Uuid::from_u128(900),
                deleted_kind: RecordKind::Favorite,
                revision: Revision::INITIAL,
                updated_by_device_id: device,
                hlc,
            }),
        ]);
        let envelopes: Vec<_> = records
            .iter()
            .map(|r| PortableRecordEnvelope::new(r.clone(), None).unwrap())
            .collect();
        let pending = DurableStateChange::new(
            StateKind::Pending,
            uuid::Uuid::from_u128(2_000),
            serde_json::json!({"mutation_id":"frozen-candidate", "predecessor":"original-predecessor", "changed_at_ms":1_700_000_001_000u64}),
        );
        let alias = DurableStateChange::new(
            StateKind::Alias,
            uuid::Uuid::from_u128(2_001),
            serde_json::json!({"kind":"song", "alias":uuid::Uuid::from_u128(1_000), "canonical":uuid::Uuid::from_u128(100)}),
        );
        let state = vec![pending, alias];
        let resources = vec![DurableResource {
            relative_path: "echo/assets/original".into(),
            content_hash: blake3::hash(b"original cover").to_hex().to_string(),
            byte_length: 14,
        }];
        let expected = CanonicalLibraryState::from_parts(&envelopes, &state, &resources);
        let mut reversed = envelopes.clone();
        reversed.reverse();
        expected.assert_matches(&CanonicalLibraryState::from_parts(
            &reversed, &state, &resources,
        ));
        // Every mutation preserves the number of rows, the weak comparison
        // that missed the original loss of identities and relationships.
        for (kind, field, value) in [
            (
                RecordKind::Song,
                "song_uuid",
                serde_json::json!(uuid::Uuid::from_u128(9_000)),
            ),
            (
                RecordKind::Song,
                "media_path",
                serde_json::to_value(LibraryRelativePath::new("media/moved.flac").unwrap())
                    .unwrap(),
            ),
            (
                RecordKind::Song,
                "content_hash",
                serde_json::json!(blake3::hash(b"different").to_hex().to_string()),
            ),
            (RecordKind::Song, "added_at", serde_json::json!(123)),
            (
                RecordKind::Favorite,
                "is_favorite",
                serde_json::json!(false),
            ),
            (RecordKind::Playlist, "created_at", serde_json::json!(123)),
            (
                RecordKind::PlaylistItem,
                "item_uuid",
                serde_json::json!(uuid::Uuid::from_u128(9_001)),
            ),
            (
                RecordKind::PlaylistItem,
                "song_uuid",
                serde_json::json!(song),
            ),
            (RecordKind::PlaylistItem, "position", serde_json::json!(999)),
            (RecordKind::PlaylistItem, "added_at", serde_json::json!(123)),
            (
                RecordKind::Override,
                "lyrics_text",
                serde_json::json!("changed"),
            ),
            (
                RecordKind::PlayStats,
                "by_device",
                serde_json::json!({device.as_uuid().to_string(): 8}),
            ),
            (
                RecordKind::Tombstone,
                "deleted_kind",
                serde_json::json!("song"),
            ),
        ] {
            let mut changed = envelopes.clone();
            let index = changed
                .iter()
                .position(|e| e.record().kind() == kind)
                .unwrap();
            let mut json = serde_json::to_value(&changed[index]).unwrap();
            json["record"][field] = value;
            changed[index] = serde_json::from_value(json).unwrap();
            assert_ne!(
                expected,
                CanonicalLibraryState::from_parts(&changed, &state, &resources),
                "missed {kind}:{field}"
            );
        }
        let mut changed_state = state.clone();
        changed_state[0].payload["predecessor"] = serde_json::json!("different");
        assert_ne!(
            expected,
            CanonicalLibraryState::from_parts(&envelopes, &changed_state, &resources)
        );
        changed_state = state.clone();
        changed_state[1].payload["canonical"] = serde_json::json!(song);
        assert_ne!(
            expected,
            CanonicalLibraryState::from_parts(&envelopes, &changed_state, &resources)
        );
        let mut changed_resources = resources.clone();
        changed_resources[0].byte_length += 1;
        assert_ne!(
            expected,
            CanonicalLibraryState::from_parts(&envelopes, &state, &changed_resources)
        );
    }
}
