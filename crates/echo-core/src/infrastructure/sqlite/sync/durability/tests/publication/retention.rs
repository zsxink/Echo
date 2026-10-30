//! Exact publication deltas survive coverage, immutable retries, and faults.
use super::*;
use crate::domain::library::durability::{DurableResource, DurableStateChange, StateKind};
use std::sync::atomic::{AtomicBool, Ordering};

#[test]
fn library_durability_mark_published_retains_facts_resources_across_window_and_old_retries() {
    let f = fixture();
    let plane = plane(&f);
    f.db.configure_library_control(plane.clone());
    let (resource, unrelated) = seed_retained_resource(&f, &plane);
    seed_other_sql_facts(&f);
    let frozen = f.db.with_reader(|c| export_sync_state(c, f.b)).unwrap();
    for kind in [
        StateKind::Pending,
        StateKind::PendingDisposition,
        StateKind::Alias,
        StateKind::Receipt,
        StateKind::Cursor,
        StateKind::ResourceLocation,
    ] {
        assert!(
            frozen.iter().any(|s| s.kind == kind),
            "fixture must include {kind:?}"
        );
    }
    let mut previous = None;
    let mut first = None;
    // 35 real operations cross the current 32-commit coverage window.
    for sequence in 1..=17 {
        let candidate = candidate(&f, sequence, previous);
        let checkpoint =
            f.db.stage_confirmation_at(f.b_binding, &candidate, Some(100 + sequence))
                .unwrap()
                .checkpoint
                .unwrap();
        f.db.mark_published(checkpoint).unwrap();
        let publication = published(&plane, f.b, checkpoint);
        assert_eq!(publication.batch.state.len(), 1);
        assert!(publication.batch.records.is_empty());
        if first.is_none() {
            let stage = plane
                .read_operation(
                    f.b,
                    identity(
                        "host-confirmation",
                        &candidate.mutation_id.as_uuid().to_string(),
                    ),
                )
                .unwrap()
                .unwrap();
            first = Some((candidate.clone(), checkpoint, publication, stage));
        }
        previous = Some(candidate.mutation_id);
    }
    let replay = plane.recover(f.b).unwrap();
    assert!(
        replay.checkpoint.is_some(),
        "exercise covered immutable receipts"
    );
    let (candidate, checkpoint, original, original_stage) = first.unwrap();
    assert_eq!(published(&plane, f.b, checkpoint), original);
    let head = replay.head().map(|(seq, digest)| (seq, digest.to_owned()));
    f.db.mark_published(checkpoint).unwrap();
    f.db.stage_confirmation_at(f.b_binding, &candidate, Some(999_999))
        .unwrap();
    assert_eq!(published(&plane, f.b, checkpoint), original);
    assert_eq!(
        plane
            .read_operation(f.b, original_stage.batch.operation_id)
            .unwrap()
            .unwrap(),
        original_stage
    );
    let after = plane.recover(f.b).unwrap();
    assert_eq!(
        after.head().map(|(seq, digest)| (seq, digest.to_owned())),
        head
    );
    assert!(after.states().any(|s| s == &unrelated));
    assert!(after
        .checkpoint
        .as_ref()
        .unwrap()
        .resources
        .contains(&resource));
    let fresh = wipe_and_project(&f, &plane);
    let retained = fresh.read_recovery_records(f.b).unwrap();
    assert!(retained.state.contains(&unrelated));
    assert!(retained.resources.contains(&resource));
    let latest: std::collections::BTreeMap<_, _> = after
        .states()
        .map(|s| (s.storage_key().unwrap(), s.clone()))
        .collect();
    for state in latest.values() {
        assert!(retained.state.contains(state), "lost {:?}", state.kind);
    }
    let path = f
        .directory
        .path()
        .join(f.b.to_string())
        .join(&resource.relative_path);
    assert_eq!(std::fs::read(path).unwrap(), RESOURCE_BYTES);
}

#[test]
fn library_durability_mark_published_saved_pending_recovery_retry_reuses_original_delta() {
    let f = fixture();
    let plane = plane(&f);
    f.db.configure_library_control(plane.clone());
    let candidate = candidate(&f, 1, None);
    let checkpoint =
        f.db.stage_confirmation_at(f.b_binding, &candidate, Some(123))
            .unwrap()
            .checkpoint
            .unwrap();
    let flag = AtomicBool::new(false);
    let faulted = Arc::new(plane.as_ref().clone().with_commit_hook(move |phase, _| {
        if phase == "committed" && !flag.swap(true, Ordering::SeqCst) {
            Err(Error::unavailable(
                "publication fault",
                "after durable marker",
            ))
        } else {
            Ok(())
        }
    }));
    f.db.configure_library_control(faulted);
    let error = f.db.mark_published(checkpoint).unwrap_err();
    assert_eq!(error.code(), "library_saved_pending_recovery");
    let original = published(&plane, f.b, checkpoint);
    assert_eq!(original.batch.state.len(), 1);
    assert!(original.batch.records.is_empty());
    let sql_state: String =
        f.db.with_reader(|c| {
            c.query_row(
                "SELECT publication_state FROM sync_commits WHERE commit_seq=1",
                [],
                |row| row.get(0),
            )
            .map_err(storage)
        })
        .unwrap();
    assert_ne!(sql_state, "published", "SQL projection must roll back");
    f.db.configure_library_control(plane.clone());
    f.db.mark_published(checkpoint).unwrap();
    assert_eq!(published(&plane, f.b, checkpoint), original);
    assert_eq!(
        plane.recover(f.b).unwrap().head().unwrap().0,
        original.library_commit_seq
    );
}

#[test]
fn library_durability_mark_published_saved_pending_projection_keeps_directory_delta() {
    let f = fixture();
    let plane = plane(&f);
    f.db.configure_library_control(plane.clone());
    let candidate = candidate(&f, 1, None);
    let checkpoint =
        f.db.stage_confirmation_at(f.b_binding, &candidate, Some(123))
            .unwrap()
            .checkpoint
            .unwrap();
    // The directory succeeds, then SQLite's actual deferred constraint rejects
    // COMMIT. This is different from rejecting the UPDATE before persistence.
    f.db.writer.run(|c| c.execute_batch("CREATE TABLE publication_commit_failure (parent TEXT REFERENCES playlists(uuid) DEFERRABLE INITIALLY DEFERRED); CREATE TRIGGER fail_publication_commit AFTER UPDATE OF publication_state ON sync_commits BEGIN INSERT INTO publication_commit_failure VALUES ('missing-playlist'); END;").map_err(storage)).unwrap();
    let error = f.db.mark_published(checkpoint).unwrap_err();
    assert_eq!(error.code(), "library_saved_pending_projection");
    let original = published(&plane, f.b, checkpoint);
    assert_eq!(original.batch.state.len(), 1);
    assert!(original.batch.records.is_empty());
    f.db.writer
        .run(|c| {
            c.execute_batch("DROP TRIGGER fail_publication_commit;")
                .map_err(storage)
        })
        .unwrap();
    f.db.mark_published(checkpoint).unwrap();
    assert_eq!(published(&plane, f.b, checkpoint), original);
    assert_eq!(
        plane.recover(f.b).unwrap().head().unwrap().0,
        original.library_commit_seq
    );
}

fn seed_other_sql_facts(f: &Fixture) {
    use crate::application::ports::SongRepository;
    use crate::domain::{
        entities::Song,
        ids::{RelativeMediaPath, Revision, SongId},
    };
    let song_id = SongId::new();
    let song = Song::new(
        song_id,
        f.b,
        RelativeMediaPath::new("local-only.flac").unwrap(),
        Revision::INITIAL,
    );
    SongRepository::upsert(&f.db, &song).unwrap();
    let root = f.b.to_string();
    f.db.writer.run(move |c| {
        c.execute("INSERT INTO sync_song_aliases(alias_song_uuid,canonical_song_uuid,commit_seq) VALUES(?1,?2,0)", params![Uuid::new_v4().to_string(), song_id.to_string()]).map_err(storage)?;
        c.execute("INSERT INTO sync_inbox(origin_device_id,mutation_id,payload_digest,outcome_json,commit_seq,received_at) VALUES(?1,?2,?3,'{\"legacy\":true}',0,12)", params![Uuid::new_v4().to_string(), Uuid::new_v4().to_string(), "a".repeat(64)]).map_err(storage)?;
        c.execute("INSERT INTO sync_peer_cursors(peer_device_id,applied_commit_seq,applied_digest,updated_at) VALUES(?1,0,?2,12)", params![Uuid::new_v4().to_string(), [0_u8;32].as_slice()]).map_err(storage)?;
        c.execute("INSERT INTO sync_remote_publications(operation_uuid,local_root_uuid,object_type,object_uuid,declaration_json,publication_json,completed) VALUES(?1,?2,'song',?3,'{}','{ \"legacy\" : true }',0)", params![Uuid::new_v4().to_string(), root, song_id.to_string()]).map_err(storage)?;
        Ok(())
    }).unwrap();
}

const RESOURCE_BYTES: &[u8] = b"resource held by an earlier operation";

fn seed_retained_resource(
    f: &Fixture,
    plane: &RootControlPlane,
) -> (DurableResource, DurableStateChange) {
    let library = plane.read_manifest(f.b).unwrap().unwrap().library_id;
    let bytes = RESOURCE_BYTES;
    let path = f
        .directory
        .path()
        .join(f.b.to_string())
        .join("retained.flac");
    std::fs::write(&path, bytes).unwrap();
    let resource = DurableResource {
        relative_path: "retained.flac".into(),
        content_hash: blake3::hash(bytes).to_hex().to_string(),
        byte_length: bytes.len() as u64,
    };
    let unrelated = DurableStateChange::new(
        StateKind::Claim,
        Uuid::new_v4(),
        json!({"retained":"unrelated evidence"}),
    );
    let mut seed = LibraryBatch::new(Uuid::new_v4(), library, ChangeOrigin::Recovery, vec![]);
    seed.state.push(unrelated.clone());
    seed.resources.push(resource.clone());
    plane.commit(f.b, &seed).unwrap();
    (resource, unrelated)
}
