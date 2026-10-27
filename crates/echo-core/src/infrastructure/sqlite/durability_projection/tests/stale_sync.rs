//! A v2 `sync` block is a claim; only a verified ledger event may confirm it.
use super::*;
use crate::domain::library::durability::{ProjectionTarget, ProjectionUnresolvedReason};
use crate::domain::library::{
    DeviceId, HybridLogicalClock, LibraryRelativePath, PortableRecord, PortableRecordEnvelope,
    PortableSyncMetadata, SongRecord,
};
use crate::domain::sync::{
    AuthorityId, CommitSeq, DeviceId as SyncDeviceId, Epoch, MutationId, ObjectKey, ObjectVersion,
    SyncBinding,
};
use serde_json::{Value, json};

fn binding() -> SyncBinding {
    SyncBinding {
        library_id: crate::domain::sync::LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    }
}

fn song(id: SongId, title: &str) -> PortableRecord {
    PortableRecord::Song(SongRecord {
        song_uuid: id.as_uuid(),
        revision: Revision::from_u64(1),
        updated_by_device_id: DeviceId::new(),
        hlc: HybridLogicalClock::new(1_700_000_000, 0),
        media_path: LibraryRelativePath::new("media/song.flac").unwrap(),
        content_hash: "a".repeat(64),
        added_at: 1_700_000_000_000,
        title: Some(title.to_owned()),
        artist: Some("artist".to_owned()),
        album: None,
    })
}

/// A sealed host event at `sequence` whose payload is `payload`, plus the
/// ledger row a verified host would have persisted for it.
fn confirmed_event(
    db: &SqliteDatabase,
    binding: SyncBinding,
    sequence: u64,
    object: ObjectKey,
    payload: Value,
) {
    let mutation = json!({
        "protocol_version": 1,
        "mutation_id": MutationId::new(),
        "origin_device_id": SyncDeviceId::new(),
        "object": object,
        "payload": payload,
    });
    let body = json!({
        "protocol_version": 1,
        "binding": binding,
        "commit_seq": sequence,
        "previous_digest": serde_json::to_value([7u8; 32]).unwrap(),
        "mutation": mutation,
    });
    let raw = serde_json::to_string(&body).unwrap();
    let mut hash = blake3::Hasher::new();
    hash.update(b"echo.lan.commit.v1\0");
    hash.update(&1u16.to_be_bytes());
    hash.update(&sequence.to_be_bytes());
    hash.update(&[7u8; 32]);
    hash.update(&u32::try_from(raw.len()).unwrap().to_be_bytes());
    hash.update(raw.as_bytes());
    let digest = *hash.finalize().as_bytes();
    db.writer
        .run(move |c| {
            c.execute("INSERT INTO sync_commits(commit_seq,library_id,authority_id,epoch,origin_device_id,mutation_id,canonical_json,previous_digest,commit_digest,publication_state,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,'published',1)",params![sequence,binding.library_id.as_uuid().to_string(),binding.authority_id.as_uuid().to_string(),binding.epoch.as_u64(),body["mutation"]["origin_device_id"].as_str().unwrap(),body["mutation"]["mutation_id"].as_str().unwrap(),raw,[7u8;32].as_slice(),digest.as_slice()]).map_err(storage)?;
            Ok(())
        })
        .unwrap();
}

fn retained_json(db: &SqliteDatabase, root: LibraryRootId, object: ObjectKey) -> String {
    db.with_reader(|c| {
        c.query_row(
            "SELECT envelope_json FROM library_recovery_records WHERE library_root_uuid=?1 AND object_key=?2",
            params![root.to_string(), serde_json::to_string(&object).unwrap()],
            |row| row.get(0),
        )
        .map_err(storage)
    })
    .unwrap()
}

fn reported(
    db: &SqliteDatabase,
    root: LibraryRootId,
    object: ObjectKey,
) -> Option<ProjectionUnresolvedReason> {
    db.projection_status(root)
        .unwrap()
        .unresolved
        .into_iter()
        .find_map(|issue| match issue.target {
            ProjectionTarget::Record { object: reported } if reported == object => {
                Some(issue.reason)
            }
            _ => None,
        })
}

fn baselines(db: &SqliteDatabase, object: ObjectKey) -> u64 {
    let (kind, id) = match object {
        ObjectKey::Song(id) => ("song", id.to_string()),
        other => panic!("unsupported identity {other:?}"),
    };
    db.with_reader(|c| {
        c.query_row(
            "SELECT COUNT(*) FROM sync_object_versions WHERE object_type=?1 AND object_uuid=?2",
            params![kind, id],
            |row| row.get(0),
        )
        .map_err(storage)
    })
    .unwrap()
}

#[test]
fn library_durability_stale_sync_metadata_stays_an_unproven_retained_candidate() {
    let (_directory, db, root) = database();
    let binding = binding();
    let id = SongId::new();
    let object = ObjectKey::Song(id);
    // The host confirmed the *old* value; a later local edit kept the block.
    confirmed_event(
        &db,
        binding,
        CommitSeq::from_u64(4).as_u64(),
        object,
        json!({"song_uuid": id, "title": "old"}),
    );
    let envelope = PortableRecordEnvelope::new(
        song(id, "new"),
        Some(PortableSyncMetadata {
            binding,
            object_version: ObjectVersion::from_commit_seq(CommitSeq::from_u64(4)),
            aliases: vec![],
        }),
    )
    .unwrap();
    db.project_legacy_records(root, std::slice::from_ref(&envelope))
        .unwrap();
    // The directory value is still the source of truth...
    let title: String = db
        .with_reader(|c| {
            c.query_row(
                "SELECT title FROM songs WHERE uuid=?1",
                [id.to_string()],
                |r| r.get(0),
            )
            .map_err(storage)
        })
        .unwrap();
    assert_eq!(title, "new");
    // ...but the claim is reported, and its original bytes are untouched.
    assert_eq!(
        reported(&db, root, object),
        Some(ProjectionUnresolvedReason::UnverifiedConfirmationMetadata)
    );
    assert_eq!(
        retained_json(&db, root, object),
        serde_json::to_string(&envelope).unwrap()
    );
    // No false confirmed baseline is minted for the unproven value.
    assert_eq!(baselines(&db, object), 0);
}

#[test]
fn library_durability_matching_verified_event_confirms_the_retained_value() {
    let (_directory, db, root) = database();
    let binding = binding();
    let id = SongId::new();
    let object = ObjectKey::Song(id);
    confirmed_event(
        &db,
        binding,
        CommitSeq::from_u64(4).as_u64(),
        object,
        json!({"song_uuid": id, "title": "same"}),
    );
    let envelope = PortableRecordEnvelope::new(
        song(id, "same"),
        Some(PortableSyncMetadata {
            binding,
            object_version: ObjectVersion::from_commit_seq(CommitSeq::from_u64(4)),
            aliases: vec![],
        }),
    )
    .unwrap();
    db.project_legacy_records(root, &[envelope]).unwrap();
    assert_eq!(reported(&db, root, object), None);
}

#[test]
fn library_durability_claim_at_another_objects_version_is_unproven() {
    let (_directory, db, root) = database();
    let binding = binding();
    let id = SongId::new();
    let object = ObjectKey::Song(id);
    // Sequence 4 confirmed a playlist, not this song: the version is borrowed.
    confirmed_event(
        &db,
        binding,
        CommitSeq::from_u64(4).as_u64(),
        ObjectKey::Playlist(PlaylistId::new()),
        json!({"display_name": "another object"}),
    );
    let envelope = PortableRecordEnvelope::new(
        song(id, "local"),
        Some(PortableSyncMetadata {
            binding,
            object_version: ObjectVersion::from_commit_seq(CommitSeq::from_u64(4)),
            aliases: vec![],
        }),
    )
    .unwrap();
    db.project_legacy_records(root, &[envelope]).unwrap();
    assert_eq!(
        reported(&db, root, object),
        Some(ProjectionUnresolvedReason::UnverifiedConfirmationMetadata)
    );
    assert_eq!(baselines(&db, object), 0);
}

#[test]
fn library_durability_late_ack_clears_the_report_without_rewriting_the_value() {
    let (_directory, db, root) = database();
    let binding = binding();
    let id = SongId::new();
    let object = ObjectKey::Song(id);
    let envelope = PortableRecordEnvelope::new(
        song(id, "pending"),
        Some(PortableSyncMetadata {
            binding,
            object_version: ObjectVersion::from_commit_seq(CommitSeq::from_u64(4)),
            aliases: vec![],
        }),
    )
    .unwrap();
    // Offline: no ledger event exists yet, so the claim cannot be proven.
    db.project_legacy_records(root, std::slice::from_ref(&envelope))
        .unwrap();
    assert_eq!(
        reported(&db, root, object),
        Some(ProjectionUnresolvedReason::UnverifiedConfirmationMetadata)
    );
    // The acknowledgement arrives later, confirming exactly this value.
    confirmed_event(
        &db,
        binding,
        CommitSeq::from_u64(4).as_u64(),
        object,
        json!({"song_uuid": id, "title": "pending"}),
    );
    db.project_legacy_records(root, std::slice::from_ref(&envelope))
        .unwrap();
    assert_eq!(reported(&db, root, object), None);
    assert_eq!(
        retained_json(&db, root, object),
        serde_json::to_string(&envelope).unwrap()
    );
}

#[test]
fn library_durability_genesis_metadata_claims_no_confirmation() {
    let (_directory, db, root) = database();
    let id = SongId::new();
    let object = ObjectKey::Song(id);
    let envelope = PortableRecordEnvelope::new(
        song(id, "local"),
        Some(PortableSyncMetadata {
            binding: binding(),
            object_version: ObjectVersion::INITIAL,
            aliases: vec![],
        }),
    )
    .unwrap();
    db.project_legacy_records(root, &[envelope]).unwrap();
    assert_eq!(reported(&db, root, object), None);
    assert_eq!(baselines(&db, object), 0);
}
