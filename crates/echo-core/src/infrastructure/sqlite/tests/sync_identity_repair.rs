use super::*;
use crate::application::ports::LocalSyncQueuePort;
use crate::domain::sync::{
    AuthorityId, Epoch, LibraryId, ObjectKey, ResourceDigest, SharedResourceKind,
    SharedResourceManifest, SyncBinding,
};

// Two tag variants have the same encoded frames and logical destination, but
// remain distinct confirmed identities until an explicitly reviewed repair.
fn flac(tag: u8, frame: u8) -> Vec<u8> {
    let mut bytes = b"fLaC".to_vec();
    bytes.extend([0, 0, 0, 34]);
    bytes.extend([0; 34]);
    bytes.extend([0x84, 0, 0, 1, tag]);
    bytes.extend([0xff, 0xf8, frame, 0]);
    bytes
}

fn fixture(
    old_bytes: &[u8],
    canonical_bytes: &[u8],
) -> (tempfile::TempDir, SqliteDatabase, SongId, SongId) {
    let (directory, database, root) = database();
    let root_path = directory.path().join("library");
    std::fs::create_dir_all(&root_path).unwrap();
    let mut ids = Vec::new();
    for (path, bytes) in [("old.flac", old_bytes), ("canonical.flac", canonical_bytes)] {
        std::fs::write(root_path.join(path), bytes).unwrap();
        let mut imported = song(root, path, "Song", "Artist");
        let hash = blake3::hash(bytes);
        imported.apply_scan_facts(
            hash.to_hex().to_string(),
            bytes.len() as u64,
            1,
            crate::domain::media::AudioFormat::Flac,
        );
        SongRepository::upsert(&database, &imported).unwrap();
        let id = imported.id();
        database
            .set_local_sync_resources(
                ObjectKey::Song(id),
                vec![
                    SharedResourceManifest::new(
                        SharedResourceKind::Audio,
                        ResourceDigest::from_bytes(*hash.as_bytes()),
                        bytes.len() as u64,
                        "audio/flac".into(),
                        "song.flac",
                    )
                    .unwrap(),
                ],
            )
            .unwrap();
        let mut payload: serde_json::Value =
            serde_json::from_str(&outbox_latest_payload(&database, "song", &id.to_string()))
                .unwrap();
        payload["relative_path"] = "song.flac".into();
        database.writer.run(move |c| {
            c.execute("INSERT INTO sync_object_versions(object_type,object_uuid,object_version,commit_seq,payload_json) VALUES('song',?1,1,1,?2)", params![id.to_string(), payload.to_string()]).map_err(storage)?;
            Ok(())
        }).unwrap();
        ids.push(id);
    }
    database
        .writer
        .run(|c| {
            c.execute("UPDATE sync_mutations SET status='confirmed'", [])
                .map_err(storage)?;
            Ok(())
        })
        .unwrap();
    database
        .bind_sync_library(
            SyncBinding {
                library_id: LibraryId::new(),
                authority_id: AuthorityId::new(),
                epoch: Epoch::INITIAL,
            },
            false,
            root,
        )
        .unwrap();
    (directory, database, ids[0], ids[1])
}

fn alter(database: &SqliteDatabase, id: SongId, sql: &'static str) {
    database
        .writer
        .run(move |c| {
            c.execute(sql, params![id.to_string()]).map_err(storage)?;
            Ok(())
        })
        .unwrap();
}

#[test]
fn reviewed_identical_frames_enqueue_canonical_content_without_changing_catalog() {
    let (directory, database, old, canonical) = fixture(&flac(1, 7), &flac(2, 7));
    let plan = database
        .prepare_song_identity_repair(old, 1, canonical, 1)
        .unwrap();
    assert_eq!(plan.audio_bytes, 4);
    assert_eq!(
        plan.audio_blake3,
        blake3::hash(&[0xff, 0xf8, 7, 0]).to_hex().to_string()
    );
    assert!(plan.play_count_policy.contains("maximum"));
    let canonical_hash = blake3::hash(&flac(2, 7)).to_hex().to_string();
    database.enqueue_song_identity_repair(plan).unwrap();
    let pending = database.pending_local_mutations(10).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].object, ObjectKey::Song(old));
    assert_eq!(pending[0].payload["song_uuid"], old.to_string());
    assert_eq!(pending[0].payload["blake3_hash"], canonical_hash);
    assert_eq!(pending[0].resource_refs[0].digest.to_hex(), canonical_hash);
    assert_eq!(
        SongRepository::by_id(&database, old)
            .unwrap()
            .unwrap()
            .path()
            .normalized(),
        "old.flac"
    );
    assert_eq!(
        std::fs::read(directory.path().join("library/old.flac")).unwrap(),
        flac(1, 7)
    );
}

#[test]
fn repair_requires_replica_and_exact_live_confirmed_versions() {
    let (_directory, database, old, canonical) = fixture(&flac(1, 7), &flac(2, 7));
    assert!(
        database
            .prepare_song_identity_repair(old, 1, old, 1)
            .is_err()
    );
    assert!(
        database
            .prepare_song_identity_repair(old, 2, canonical, 1)
            .is_err()
    );
    assert!(
        database
            .prepare_song_identity_repair(old, 1, SongId::new(), 1)
            .is_err()
    );
    alter(
        &database,
        old,
        "UPDATE sync_binding SET role='host' WHERE ?1 IS NOT NULL",
    );
    assert!(
        database
            .prepare_song_identity_repair(old, 1, canonical, 1)
            .is_err()
    );
    alter(
        &database,
        old,
        "DELETE FROM sync_binding WHERE ?1 IS NOT NULL",
    );
    assert!(
        database
            .prepare_song_identity_repair(old, 1, canonical, 1)
            .is_err()
    );
}

#[test]
fn repair_rejects_state_that_would_hide_user_edits_or_pending_work() {
    for sql in [
        "UPDATE songs SET is_favorite=1 WHERE uuid=?1",
        "INSERT INTO song_overrides(song_uuid,title,updated_at) VALUES(?1,'Custom',1)",
        "UPDATE sync_mutations SET status='pending' WHERE object_uuid=?1",
        "DELETE FROM sync_resource_manifests WHERE object_uuid=?1",
        "UPDATE songs SET file_size=file_size+1 WHERE uuid=?1",
        "UPDATE sync_object_versions SET payload_json=json_set(payload_json,'$.relative_path','other.flac') WHERE object_uuid=?1",
        "UPDATE library_roots SET write_safety_locked=1 WHERE ?1 IS NOT NULL",
    ] {
        let (_directory, database, old, canonical) = fixture(&flac(1, 7), &flac(2, 7));
        alter(&database, old, sql);
        assert!(
            database
                .prepare_song_identity_repair(old, 1, canonical, 1)
                .is_err(),
            "{sql}"
        );
        assert_eq!(
            database.pending_local_mutations(10).unwrap().len(),
            usize::from(sql.contains("status='pending'"))
        );
    }
}

#[test]
fn enqueue_rechecks_state_and_reviewed_audio_proof() {
    let (_directory, database, old, canonical) = fixture(&flac(1, 7), &flac(2, 7));
    let plan = database
        .prepare_song_identity_repair(old, 1, canonical, 1)
        .unwrap();
    alter(
        &database,
        old,
        "UPDATE songs SET play_count=play_count+1 WHERE uuid=?1",
    );
    assert!(database.enqueue_song_identity_repair(plan).is_err());
    assert!(database.pending_local_mutations(10).unwrap().is_empty());

    let (_directory, database, old, canonical) = fixture(&flac(1, 7), &flac(2, 7));
    let mut plan = database
        .prepare_song_identity_repair(old, 1, canonical, 1)
        .unwrap();
    plan.audio_blake3 = "incorrect review checksum".into();
    assert!(database.enqueue_song_identity_repair(plan).is_err());
    assert!(database.pending_local_mutations(10).unwrap().is_empty());
}

#[test]
fn repair_rejects_different_frames_invalid_flac_and_changed_disk_content() {
    for bytes in [
        flac(2, 8),
        b"not flac".to_vec(),
        b"fLaC".to_vec(),
        {
            let mut bytes = flac(2, 7);
            bytes[4] = 127;
            bytes
        },
        {
            let mut bytes = flac(2, 7);
            bytes.truncate(43);
            bytes
        },
    ] {
        let (_directory, database, old, canonical) = fixture(&flac(1, 7), &bytes);
        assert!(
            database
                .prepare_song_identity_repair(old, 1, canonical, 1)
                .is_err()
        );
    }
    let (directory, database, old, canonical) = fixture(&flac(1, 7), &flac(2, 7));
    let plan = database
        .prepare_song_identity_repair(old, 1, canonical, 1)
        .unwrap();
    std::fs::write(directory.path().join("library/old.flac"), flac(1, 9)).unwrap();
    assert!(database.enqueue_song_identity_repair(plan).is_err());
    assert!(database.pending_local_mutations(10).unwrap().is_empty());
    std::fs::remove_file(directory.path().join("library/old.flac")).unwrap();
    assert!(
        database
            .prepare_song_identity_repair(old, 1, canonical, 1)
            .is_err()
    );
}

#[cfg(unix)]
#[test]
fn repair_never_reads_audio_through_a_symlink_outside_the_root() {
    let (directory, database, old, canonical) = fixture(&flac(1, 7), &flac(2, 7));
    let old_path = directory.path().join("library/old.flac");
    let outside = directory.path().join("outside.flac");
    std::fs::rename(&old_path, &outside).unwrap();
    std::os::unix::fs::symlink(&outside, &old_path).unwrap();
    assert!(
        database
            .prepare_song_identity_repair(old, 1, canonical, 1)
            .is_err()
    );
}
