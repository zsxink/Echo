//! Publication changes one retained ledger; legacy SQL-only baselines stay whole.
use super::*;
use crate::application::ports::{
    ControlPlanePort, DeviceIdProvider, HostCommitLedgerPort, LibraryDurabilityPort,
    LibraryProjectionPort,
};
use crate::domain::{
    ids::PlaylistId,
    library::{LibraryManifest, durability::LibraryCommit},
    sync::{CommitCheckpoint, DeviceId, MutationCandidate, MutationId, ObjectKey},
};
use crate::infrastructure::filesystem::{RootControlPlane, RootRegistry};

fn plane(f: &Fixture) -> Arc<RootControlPlane> {
    let registry = RootRegistry::new();
    for root in [f.a, f.b] {
        let path = f.directory.path().join(root.to_string());
        std::fs::create_dir_all(&path).unwrap();
        registry.register(root, &path);
    }
    let plane = Arc::new(RootControlPlane::new(registry));
    for root in [f.a, f.b] {
        plane
            .write_manifest(
                root,
                &LibraryManifest::new(
                    crate::domain::library::LibraryId::from_uuid(Uuid::new_v4()),
                    "publication delta",
                ),
            )
            .unwrap();
    }
    plane
}
fn candidate(f: &Fixture, sequence: u64, previous: Option<MutationId>) -> MutationCandidate {
    let playlist = PlaylistId::new();
    MutationCandidate {
        mutation_id: MutationId::new(),
        origin_device_id: DeviceId::from_uuid(f.db.current_device_id().as_uuid()),
        origin_seq: sequence,
        previous_mutation_id: previous,
        object: ObjectKey::Playlist(playlist),
        base_object_version: None,
        payload: json!({"playlist_uuid":playlist,"display_name":format!("playlist {sequence}"),"members":[],"created_at":42}),
        resource_refs: vec![],
    }
}
fn published(
    plane: &RootControlPlane,
    root: LibraryRootId,
    checkpoint: CommitCheckpoint,
) -> LibraryCommit {
    let operation = identity(
        "host-published",
        &format!(
            "{}:{}",
            checkpoint.binding.library_id.as_uuid(),
            checkpoint.sequence.as_u64()
        ),
    );
    plane.read_operation(root, operation).unwrap().unwrap()
}

#[test]
fn library_durability_mark_published_captures_only_exact_retained_ledger() {
    let f = fixture();
    let plane = plane(&f);
    f.db.configure_library_control(plane.clone());
    let mut previous = None;
    for sequence in 1..=3 {
        let candidate = candidate(&f, sequence, previous);
        let checkpoint =
            f.db.stage_confirmation_at(f.b_binding, &candidate, Some(100 + sequence))
                .unwrap()
                .checkpoint
                .unwrap();
        let before = f.db.with_reader(|c| export_sync_state(c, f.b)).unwrap();
        let ledger = before
            .iter()
            .find(|s| s.kind == StateKind::SyncLedger && s.payload["sequence"] == sequence)
            .unwrap();
        f.db.mark_published(checkpoint).unwrap();
        let commit = published(&plane, f.b, checkpoint);
        eprintln!(
            "publication seq={sequence} bytes={} records={} state={}",
            serde_json::to_vec(&commit.batch).unwrap().len(),
            commit.batch.records.len(),
            commit.batch.state.len()
        );
        if sequence < 3 {
            previous = Some(candidate.mutation_id);
            continue;
        }
        assert!(
            commit.batch.records.is_empty(),
            "publication must not repeat catalog records"
        );
        assert_eq!(
            commit.batch.state.len(),
            1,
            "publication must not repeat unrelated historical facts"
        );
        let mut expected = ledger.clone();
        expected.payload["publication"] = json!("published");
        assert_eq!(commit.batch.state[0], expected);
        commit.batch.state[0].validate().unwrap();
        previous = Some(candidate.mutation_id);
    }
}

fn wipe_and_project(f: &Fixture, plane: &RootControlPlane) -> SqliteDatabase {
    let fresh = SqliteDatabase::open(f.directory.path().join("wipe.sqlite")).unwrap();
    LibraryRepository::upsert(
        &fresh,
        &LibraryRoot::new(f.b, f.directory.path().join(f.b.to_string()), true, true),
    )
    .unwrap();
    fresh.configure_library_control(Arc::new(plane.clone()));
    fresh
        .project_library_commits(f.b, &plane.recover(f.b).unwrap())
        .unwrap();
    fresh
}

#[test]
fn library_durability_mark_published_legacy_unseeded_ledger_keeps_full_rescue() {
    let f = fixture();
    let candidate = candidate(&f, 1, None);
    let checkpoint =
        f.db.stage_confirmation_at(f.b_binding, &candidate, Some(123))
            .unwrap()
            .checkpoint
            .unwrap();
    let plane = plane(&f);
    f.db.configure_library_control(plane.clone());
    f.db.mark_published(checkpoint).unwrap();
    let commit = published(&plane, f.b, checkpoint);
    assert_eq!(commit.batch.records.len(), 1);
    assert!(commit.batch.state.len() > 1);
    let fresh = wipe_and_project(&f, &plane);
    let retained = fresh.read_recovery_records(f.b).unwrap();
    for state in &commit.batch.state {
        assert!(
            retained.state.contains(state),
            "lost SQL-only evidence: {:?}",
            state.kind
        );
    }
    assert_eq!(
        crate::application::ports::PlaylistRepository::name(
            &fresh,
            match candidate.object {
                ObjectKey::Playlist(id) => id,
                _ => unreachable!(),
            }
        )
        .unwrap(),
        Some("playlist 1".into())
    );
}

#[test]
fn library_durability_mark_published_never_uses_other_root_or_binding_same_sequence() {
    for copy_other_binding_into_target in [false, true] {
        let f = fixture();
        let plane = plane(&f);
        let candidate = candidate(&f, 1, None);
        f.db.select_sync_context(f.a).unwrap();
        f.db.configure_library_control(plane.clone());
        f.db.stage_confirmation_at(f.a_binding, &candidate, Some(123))
            .unwrap();
        let stage_operation = identity(
            "host-confirmation",
            &candidate.mutation_id.as_uuid().to_string(),
        );
        let mut other = plane
            .read_operation(f.a, stage_operation)
            .unwrap()
            .unwrap()
            .batch;
        // Stage the same event identity in another root without directory seeding.
        f.db.select_sync_context(f.b).unwrap();
        // A second database adapter shares the real file but has no control plane.
        let isolated = SqliteDatabase::open(f.directory.path().join("scope.sqlite")).unwrap();
        let checkpoint = isolated
            .stage_confirmation_at(f.b_binding, &candidate, Some(123))
            .unwrap()
            .checkpoint
            .unwrap();
        drop(isolated);
        if copy_other_binding_into_target {
            other.library_id = plane.read_manifest(f.b).unwrap().unwrap().library_id;
            plane.commit(f.b, &other).unwrap();
        }
        f.db.mark_published(checkpoint).unwrap();
        let commit = published(&plane, f.b, checkpoint);
        assert!(!commit.batch.records.is_empty());
        assert!(commit.batch.state.len() > 1);
        assert!(
            commit
                .batch
                .state
                .iter()
                .any(|s| s.kind == StateKind::SyncLedger
                    && s.key.as_ref().unwrap().namespace
                        == StateNamespace::Bound {
                            binding: f.b_binding
                        })
        );
    }
}

#[test]
fn library_durability_mark_published_changes_beyond_publication_force_full_capture() {
    let f = fixture();
    let plane = plane(&f);
    f.db.configure_library_control(plane.clone());
    let candidate = candidate(&f, 1, None);
    let checkpoint =
        f.db.stage_confirmation_at(f.b_binding, &candidate, Some(123))
            .unwrap()
            .checkpoint
            .unwrap();
    // The original authoritative ledger cannot prove this changed timestamp.
    f.db.writer
        .run(|c| {
            c.execute("UPDATE sync_commits SET created_at=created_at+1", [])
                .map_err(storage)?;
            Ok(())
        })
        .unwrap();
    f.db.mark_published(checkpoint).unwrap();
    let commit = published(&plane, f.b, checkpoint);
    assert!(!commit.batch.records.is_empty());
    assert!(commit.batch.state.len() > 1);
}

mod retention;

#[test]
fn library_durability_mark_published_keeps_exact_original_wire_resources_and_digest() {
    use crate::domain::{
        ids::SongId,
        sync::{ResourceDigest, SharedResourceKind, SharedResourceManifest},
    };
    let f = fixture();
    let plane = plane(&f);
    f.db.configure_library_control(plane.clone());
    let bytes = b"confirmed resource bytes";
    let root_path = f.directory.path().join(f.b.to_string());
    std::fs::write(root_path.join("confirmed.flac"), bytes).unwrap();
    let resource = SharedResourceManifest::new(
        SharedResourceKind::Audio,
        ResourceDigest::from_bytes(*blake3::hash(bytes).as_bytes()),
        bytes.len() as u64,
        "audio/flac".into(),
        "confirmed.flac",
    )
    .unwrap();
    let song = SongId::new();
    let candidate = MutationCandidate {
        mutation_id: MutationId::new(),
        origin_device_id: DeviceId::from_uuid(f.db.current_device_id().as_uuid()),
        origin_seq: 1,
        previous_mutation_id: None,
        object: ObjectKey::Song(song),
        base_object_version: None,
        payload: json!({"song_uuid":song,"relative_path":"confirmed.flac","title":"resource","blake3_hash":resource.digest.to_hex(),"added_at":42}),
        resource_refs: vec![resource.clone()],
    };
    let checkpoint =
        f.db.stage_confirmation_at(f.b_binding, &candidate, Some(123))
            .unwrap()
            .checkpoint
            .unwrap();
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
    let original = stage
        .batch
        .state
        .iter()
        .find(|s| s.kind == StateKind::SyncLedger)
        .unwrap();
    f.db.mark_published(checkpoint).unwrap();
    let publication = published(&plane, f.b, checkpoint);
    let mut expected = original.clone();
    expected.payload["publication"] = json!("published");
    assert_eq!(publication.batch.state, vec![expected]);
    assert!(publication.batch.records.is_empty());
    assert_eq!(
        publication.batch.state[0].payload["original_body"],
        original.payload["original_body"]
    );
    assert_eq!(
        publication.batch.state[0].payload["body"]["resource_refs"][0]["content_hash"],
        resource.digest.to_hex()
    );
    assert_eq!(
        publication.batch.state[0].payload["digest"],
        blake3::Hash::from_bytes(checkpoint.digest)
            .to_hex()
            .to_string()
    );
}

#[test]
fn library_durability_mark_published_rejects_saved_operation_from_old_authority_epoch() {
    use crate::application::ports::HostEpochRecoveryPort;
    let f = fixture();
    let plane = plane(&f);
    f.db.configure_library_control(plane.clone());
    let original = candidate(&f, 1, None);
    let old_checkpoint =
        f.db.stage_confirmation_at(f.b_binding, &original, Some(123))
            .unwrap()
            .checkpoint
            .unwrap();
    f.db.mark_published(old_checkpoint).unwrap();
    let old_operation = published(&plane, f.b, old_checkpoint);
    let replacement = SyncBinding {
        authority_id: AuthorityId::new(),
        epoch: Epoch::from_u64(2),
        ..f.b_binding
    };
    f.db.rotate_host_epoch(f.b, f.b_binding, replacement)
        .unwrap();
    let next = candidate(&f, 1, None);
    let checkpoint =
        f.db.stage_confirmation_at(replacement, &next, Some(456))
            .unwrap()
            .checkpoint
            .unwrap();
    assert_eq!(checkpoint.sequence, old_checkpoint.sequence);
    assert_eq!(
        checkpoint.binding.library_id,
        old_checkpoint.binding.library_id
    );
    let error =
        f.db.mark_published(checkpoint)
            .expect_err("old operation cannot authorize publication in a replacement binding");
    assert_eq!(error.code(), "conflict");
    assert_eq!(published(&plane, f.b, old_checkpoint), old_operation);
    let sql_state: String =
        f.db.with_reader(|c| {
            c.query_row(
                "SELECT publication_state FROM sync_commits WHERE commit_seq=1",
                [],
                |r| r.get(0),
            )
            .map_err(storage)
        })
        .unwrap();
    assert_ne!(
        sql_state, "published",
        "new SQL cannot commit against an old directory batch"
    );
}
