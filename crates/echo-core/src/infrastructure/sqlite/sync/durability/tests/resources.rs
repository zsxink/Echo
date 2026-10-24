//! Real persisted resource declarations must not be decoded as network DTOs.
use super::*;
use crate::application::ports::{
    ControlPlanePort, DeviceIdProvider, HostCommitLedgerPort, SongRepository,
};
use crate::domain::{
    entities::Song,
    ids::{RelativeMediaPath, Revision, SongId},
    library::{
        durability::{DurableResource, FrozenMutation},
        LibraryManifest,
    },
    media::AudioFormat,
    sync::{ObjectKey, ResourceDigest, SharedResourceKind, SharedResourceManifest},
};
use crate::infrastructure::filesystem::{RootControlPlane, RootRegistry};

#[test]
fn library_durability_resources_real_queue_exports_and_commits_exact_frozen_manifests() {
    let fixture = fixture();
    let db = &fixture.db;
    let root = fixture.b;
    let object = ObjectKey::Song(SongId::from_uuid(fixture.object));
    let path = fixture.directory.path().join(root.to_string());
    std::fs::create_dir_all(path.join("media")).unwrap();
    let resources = resources(&path);
    db.set_local_sync_resources(object, resources.clone())
        .unwrap();
    let mut song = Song::new(
        SongId::from_uuid(fixture.object),
        root,
        RelativeMediaPath::new("media/resource.flac").unwrap(),
        Revision::INITIAL,
    );
    song.apply_scan_facts(
        resources[0].digest.to_hex(),
        resources[0].length,
        1,
        AudioFormat::Flac,
    );
    SongRepository::upsert(db, &song).unwrap();
    let pending = db.pending_local_mutations(32).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].resource_refs, resources);
    let original: String = db
        .with_reader(|c| {
            c.query_row(
                "SELECT resource_refs_json FROM sync_mutations WHERE mutation_id=?1",
                [pending[0].mutation_id.as_uuid().to_string()],
                |row| row.get(0),
            )
            .map_err(storage)
        })
        .unwrap();
    assert!(original.contains("\"digest\":["));
    assert!(!original.contains("content_hash"));
    let exported = db.with_reader(|c| export_sync_state(c, root)).unwrap();
    let state = exported
        .iter()
        .find(|s| s.kind == StateKind::Pending)
        .unwrap();
    let frozen: FrozenMutation = serde_json::from_value(state.payload["frozen"].clone()).unwrap();
    assert_eq!(frozen.original_resources_body, original);
    assert_eq!(frozen.candidate.resource_refs, resources);
    assert_eq!(frozen.candidate, pending[0]);
    let library = crate::domain::library::LibraryId::from_uuid(Uuid::new_v4());
    let registry = RootRegistry::new();
    registry.register(root, &path);
    let plane = RootControlPlane::new(registry);
    plane
        .write_manifest(root, &LibraryManifest::new(library, "frozen-resources"))
        .unwrap();
    let mut batch = LibraryBatch::new(Uuid::new_v4(), library, ChangeOrigin::LocalEdit, vec![]);
    db.with_reader(|c| enrich_recovery_batch(c, root, &mut batch))
        .unwrap();
    assert_eq!(batch.state, exported);
    batch.resources = resources
        .iter()
        .map(|r| DurableResource {
            relative_path: r.relative_path.as_str().to_owned(),
            content_hash: r.digest.to_hex(),
            byte_length: r.length,
        })
        .collect();
    plane.durability().unwrap().commit(root, &batch).unwrap();
    let replay = plane.durability().unwrap().recover(root).unwrap();
    assert!(replay.states().any(|retained| retained == state));
}
fn resources(path: &std::path::Path) -> Vec<SharedResourceManifest> {
    let declarations = [
        (
            SharedResourceKind::Audio,
            "media/resource.flac",
            "audio/flac",
            b"frozen audio".as_slice(),
        ),
        (
            SharedResourceKind::LyricsSidecar,
            "media/resource.lrc",
            "text/plain",
            b"[00:01]frozen lyrics".as_slice(),
        ),
    ];
    declarations
        .into_iter()
        .map(|(kind, name, media_type, bytes)| {
            std::fs::write(path.join(name), bytes).unwrap();
            SharedResourceManifest::new(
                kind,
                ResourceDigest::from_bytes(*blake3::hash(bytes).as_bytes()),
                bytes.len() as u64,
                media_type.to_owned(),
                name,
            )
            .unwrap()
        })
        .collect()
}

#[test]
fn library_durability_confirmed_export_preserves_real_wire_resources_and_origin_chain() {
    use crate::domain::sync::{DeviceId, MutationCandidate, MutationId};
    let fixture = fixture();
    let db = &fixture.db;
    let root = fixture.b;
    let path = fixture.directory.path().join(root.to_string());
    std::fs::create_dir_all(path.join("media")).unwrap();
    let resources = resources(&path);
    let song = SongId::new();
    let mut candidate = MutationCandidate {
        mutation_id: MutationId::new(),
        origin_device_id: DeviceId::from_uuid(db.current_device_id().as_uuid()),
        origin_seq: 1,
        previous_mutation_id: None,
        object: ObjectKey::Song(song),
        base_object_version: None,
        payload: json!({"song_uuid":song,"relative_path":"media/resource.flac",
            "title":"confirmed","artist":"artist","added_at":42,
            "blake3_hash":resources[0].digest.to_hex()}),
        resource_refs: resources,
    };
    let first = db
        .stage_confirmation_at(fixture.b_binding, &candidate, Some(100))
        .unwrap()
        .checkpoint
        .unwrap();
    db.mark_published(first).unwrap();
    candidate.previous_mutation_id = Some(candidate.mutation_id);
    candidate.mutation_id = MutationId::new();
    candidate.origin_seq = 2;
    candidate.base_object_version = Some(crate::domain::sync::ObjectVersion::from_commit_seq(
        first.sequence,
    ));
    candidate.payload["title"] = json!("updated");
    let second = db
        .stage_confirmation_at(fixture.b_binding, &candidate, Some(200))
        .unwrap()
        .checkpoint
        .unwrap();
    db.mark_published(second).unwrap();
    let states = db.with_reader(|c| export_sync_state(c, root)).unwrap();
    let confirmed = states
        .iter()
        .find(|s| {
            s.key.as_ref().is_some_and(|key| {
                key.target
                    == StateTarget::Confirmed {
                        object: candidate.object,
                    }
            })
        })
        .unwrap();
    assert_eq!(confirmed.payload["schema"], "echo.sync.confirmed.v2");
    confirmed.validate().unwrap();
    let original = confirmed.payload["original_body"].as_str().unwrap();
    let wire: Value = serde_json::from_str(original).unwrap();
    assert_eq!(
        wire["mutation"]["previous_mutation_id"],
        json!(candidate.previous_mutation_id)
    );
    assert_eq!(
        wire["mutation"]["base_object_version"],
        json!(candidate.base_object_version)
    );
    assert_eq!(wire["resource_refs"].as_array().unwrap().len(), 2);
    assert_eq!(wire["mutation"]["resource_refs"], wire["resource_refs"]);
    assert_eq!(
        wire["resource_refs"][0]["content_hash"],
        candidate.resource_refs[0].digest.to_hex()
    );
    assert_eq!(confirmed.payload["changed_at_ms"], 200);
    let round_trip: DurableStateChange =
        serde_json::from_str(&serde_json::to_string(confirmed).unwrap()).unwrap();
    assert_eq!(round_trip, *confirmed);
    for pointer in [
        "/mutation/previous_mutation_id",
        "/mutation/base_object_version",
        "/resource_refs/0/length",
        "/aliases",
    ] {
        let mut changed = wire.clone();
        *changed.pointer_mut(pointer).unwrap() = json!("tampered");
        let mut invalid = confirmed.clone();
        invalid.payload["original_body"] = json!(changed.to_string());
        assert!(invalid.validate().is_err(), "accepted changed {pointer}");
    }
}
