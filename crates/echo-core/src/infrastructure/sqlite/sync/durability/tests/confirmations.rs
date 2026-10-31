//! Exact ledger enrichment is scoped by the original binding, never just seq.
use super::*;
use crate::application::ports::{DeviceIdProvider, HostCommitLedgerPort};
use crate::domain::ids::PlaylistId;
use crate::domain::sync::{DeviceId, MutationCandidate, MutationId, ObjectKey};

fn stage(f: &Fixture, binding: SyncBinding, playlist: PlaylistId, name: &str) -> String {
    let candidate = MutationCandidate {
        mutation_id: MutationId::new(),
        origin_device_id: DeviceId::from_uuid(f.db.current_device_id().as_uuid()),
        origin_seq: 1,
        previous_mutation_id: None,
        object: ObjectKey::Playlist(playlist),
        base_object_version: None,
        payload: json!({"playlist_uuid":playlist,"display_name":name,"members":[],"created_at":42}),
        resource_refs: vec![],
    };
    let checkpoint =
        f.db.stage_confirmation_at(binding, &candidate, Some(123))
            .unwrap()
            .checkpoint
            .unwrap();
    f.db.mark_published(checkpoint).unwrap();
    f.db.with_reader(|c| {
        c.query_row(
            "SELECT canonical_json FROM sync_commits WHERE commit_seq=1",
            [],
            |r| r.get(0),
        )
        .map_err(storage)
    })
    .unwrap()
}

#[test]
fn library_durability_confirmed_export_same_sequence_never_crosses_active_archived_scope() {
    let f = fixture();
    let playlist = PlaylistId::new();
    f.db.select_sync_context(f.a).unwrap();
    let a_raw = stage(&f, f.a_binding, playlist, "A");
    f.db.select_sync_context(f.b).unwrap();
    let b_raw = stage(&f, f.b_binding, playlist, "B");
    assert_ne!(a_raw, b_raw);
    for (root, binding, name, raw) in [
        (f.a, f.a_binding, "A", a_raw),
        (f.b, f.b_binding, "B", b_raw),
    ] {
        let states = f.db.with_reader(|c| export_sync_state(c, root)).unwrap();
        let confirmed = states
            .iter()
            .find(|s| {
                s.key.as_ref().is_some_and(|key| {
                    key.target
                        == StateTarget::Confirmed {
                            object: ObjectKey::Playlist(playlist),
                        }
                })
            })
            .unwrap();
        assert_eq!(confirmed.payload["schema"], "echo.sync.confirmed.v2");
        assert_eq!(confirmed.payload["sequence"], 1);
        assert_eq!(confirmed.payload["payload"]["display_name"], name);
        assert_eq!(confirmed.payload["original_body"], raw);
        assert_eq!(
            confirmed.key.as_ref().unwrap().namespace,
            StateNamespace::Bound { binding }
        );
        confirmed.validate().unwrap();
        // This old song row has seq 1 but no matching event: even a valid
        // same-sequence playlist ledger must not manufacture its confirmation.
        let historical = states
            .iter()
            .find(|s| {
                s.key.as_ref().is_some_and(|key| {
                    key.target
                        == StateTarget::Confirmed {
                            object: ObjectKey::Song(crate::domain::ids::SongId::from_uuid(
                                f.object,
                            )),
                        }
                })
            })
            .unwrap();
        assert_eq!(historical.payload["schema"], "echo.sync.confirmed.v1");
        assert!(historical.payload.get("original_body").is_none());
    }
}

#[test]
fn library_durability_confirmed_export_missing_ledger_preserves_v1_bytes() {
    let f = fixture();
    let mut states = f.db.with_reader(|c| export_sync_state(c, f.b)).unwrap();
    let before = serde_json::to_string(&states).unwrap();
    assert!(states.iter().all(|s| s.kind != StateKind::SyncLedger));
    assert!(states
        .iter()
        .filter(|s| s.kind == StateKind::Confirmed)
        .all(|s| s.payload["schema"] == "echo.sync.confirmed.v1"));
    complete_confirmations(&mut states).unwrap();
    assert_eq!(serde_json::to_string(&states).unwrap(), before);
}

#[test]
fn library_durability_confirmed_export_keeps_real_host_typed_song_alias() {
    let f = fixture();
    let author = DeviceId::from_uuid(f.db.current_device_id().as_uuid());
    let canonical = crate::domain::ids::SongId::new();
    let alias = crate::domain::ids::SongId::new();
    let mut previous = None;
    for (index, id) in [canonical, alias].into_iter().enumerate() {
        let candidate = MutationCandidate {
            mutation_id: MutationId::new(),
            origin_device_id: author,
            origin_seq: index as u64 + 1,
            previous_mutation_id: previous,
            object: ObjectKey::Song(id),
            base_object_version: None,
            payload: json!({"song_uuid":id,"relative_path":"media/alias.flac","blake3_hash":"4".repeat(64),"title":"same bytes","added_at":42}),
            resource_refs: vec![],
        };
        let checkpoint =
            f.db.stage_confirmation_at(f.b_binding, &candidate, Some(100 + index as u64))
                .unwrap()
                .checkpoint
                .unwrap();
        f.db.mark_published(checkpoint).unwrap();
        previous = Some(candidate.mutation_id);
    }
    let states = f.db.with_reader(|c| export_sync_state(c, f.b)).unwrap();
    let confirmed = states
        .iter()
        .find(|s| {
            s.key.as_ref().is_some_and(|key| {
                key.target
                    == StateTarget::Confirmed {
                        object: ObjectKey::Song(canonical),
                    }
            })
        })
        .unwrap();
    assert_eq!(confirmed.payload["schema"], "echo.sync.confirmed.v2");
    let raw: Value =
        serde_json::from_str(confirmed.payload["original_body"].as_str().unwrap()).unwrap();
    assert_eq!(
        raw["aliases"],
        json!([{"kind":"song","alias":{"alias":alias,"canonical":canonical}}])
    );
    confirmed.validate().unwrap();
    assert!(states
        .iter()
        .any(|s| s.payload["schema"] == "echo.sync.song-alias.v1"
            && s.payload["alias"] == json!(alias)
            && s.payload["canonical"] == json!(canonical)));
}
