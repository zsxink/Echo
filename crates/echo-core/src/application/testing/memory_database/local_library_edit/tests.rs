//! The test adapter must preserve the same durable event identities as SQLite.
use super::*;
use crate::application::ports::LibraryDurabilityPort;
use crate::application::testing::small_fakes::MemoryControlPlane;
use crate::domain::library::durability::{FrozenMutation, StateTarget};
use crate::domain::library::{LibraryId, LibraryManifest};

#[test]
fn library_durability_memory_playlist_freezes_sequential_candidates() {
    let db = MemoryDatabase::new();
    let control = MemoryControlPlane::new();
    let root = LibraryRootId::new();
    let author = db.current_device_id();
    control
        .write_manifest(
            root,
            &LibraryManifest::new(LibraryId::from_uuid(uuid::Uuid::new_v4()), "test"),
        )
        .unwrap();
    let first = PlaylistId::new();
    let second = PlaylistId::new();
    for (id, name) in [(first, "first"), (second, "second")] {
        db.create_library_playlist(&control, root, id, name, author)
            .unwrap();
    }
    let replay = control.recover(root).unwrap();
    let frozen: Vec<FrozenMutation> = replay
        .states()
        .filter(|state| {
            matches!(
                state.key.as_ref().map(|key| &key.target),
                Some(StateTarget::Mutation { .. })
            )
        })
        .map(|state| {
            state.validate().unwrap();
            serde_json::from_value(state.payload["frozen"].clone()).unwrap()
        })
        .collect();
    assert_eq!(frozen.len(), 2);
    assert_eq!(frozen[0].candidate.origin_seq, 1);
    assert_eq!(frozen[1].candidate.origin_seq, 2);
    assert_eq!(
        frozen[1].candidate.previous_mutation_id,
        Some(frozen[0].candidate.mutation_id)
    );
    assert_eq!(frozen[0].local_revision.as_u64(), 1);
    assert_eq!(
        frozen[1].candidate.object,
        crate::domain::sync::ObjectKey::Playlist(second)
    );
    for commit in &replay.commits {
        assert_eq!(commit.batch.state.len(), 2);
        assert_eq!(commit.batch.records[0].record().revision().as_u64(), 1);
    }
}

#[test]
fn library_durability_memory_rename_keeps_durable_member_facts_in_candidate() {
    use crate::domain::ids::PlaylistItemId;
    use crate::domain::library::durability::{ChangeOrigin, LibraryBatch};
    use crate::domain::library::PlaylistItemRecord;
    let db = MemoryDatabase::new();
    let control = MemoryControlPlane::new();
    let root = LibraryRootId::new();
    let author = db.current_device_id();
    let library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    control
        .write_manifest(root, &LibraryManifest::new(library, "test"))
        .unwrap();
    let playlist = PlaylistId::new();
    db.create_library_playlist(&control, root, playlist, "before", author)
        .unwrap();
    let member = PlaylistItemId::new();
    let song = SongId::new();
    let record = PortableRecord::PlaylistItem(PlaylistItemRecord {
        item_uuid: member.as_uuid(),
        playlist_uuid: playlist.as_uuid(),
        song_uuid: song.as_uuid(),
        position: 7,
        added_at: Some(123),
        revision: Revision::from_u64(1),
        hlc: HybridLogicalClock::new(1, 0),
        updated_by_device_id: author,
    });
    control
        .commit(
            root,
            &LibraryBatch::new(
                uuid::Uuid::new_v4(),
                library,
                ChangeOrigin::LocalEdit,
                vec![PortableRecordEnvelope::new(record, None).unwrap()],
            ),
        )
        .unwrap();
    db.rename_library_playlist(&control, root, playlist, "after", author)
        .unwrap();
    let replay = control.recover(root).unwrap();
    let state = replay
        .commits
        .last()
        .unwrap()
        .batch
        .state
        .iter()
        .find(|state| {
            matches!(
                state.key.as_ref().map(|key| &key.target),
                Some(StateTarget::Mutation { .. })
            )
        })
        .unwrap();
    let frozen: FrozenMutation = serde_json::from_value(state.payload["frozen"].clone()).unwrap();
    assert_eq!(
        frozen.candidate.payload["members"],
        serde_json::json!([{
            "member_uuid": member.to_string(), "song_uuid": song.to_string(), "position": 7, "added_at": 123,
        }])
    );
}

#[test]
fn library_durability_memory_failed_projection_reserves_the_saved_sequence() {
    let db = MemoryDatabase::new();
    let control = MemoryControlPlane::new();
    let root = LibraryRootId::new();
    let author = db.current_device_id();
    control
        .write_manifest(
            root,
            &LibraryManifest::new(LibraryId::from_uuid(uuid::Uuid::new_v4()), "test"),
        )
        .unwrap();
    let first = PlaylistId::new();
    db.set_fail_commit(true);
    assert!(matches!(
        db.create_library_playlist(&control, root, first, "saved pending", author),
        Err(Error::LibrarySavedPending { .. })
    ));
    assert!(db.name(first).unwrap().is_none());
    let saved = control.recover(root).unwrap().commits[0].batch.clone();
    db.set_fail_commit(false);
    let second = PlaylistId::new();
    db.create_library_playlist(&control, root, second, "later edit", author)
        .unwrap();
    assert_eq!(db.name(first).unwrap().as_deref(), Some("saved pending"));
    assert_eq!(db.name(second).unwrap().as_deref(), Some("later edit"));
    let replay = control.recover(root).unwrap();
    assert_eq!(replay.commits[0].batch, saved);
    let frozen: Vec<FrozenMutation> = replay
        .states()
        .filter(|state| {
            matches!(
                state.key.as_ref().map(|key| &key.target),
                Some(StateTarget::Mutation { .. })
            )
        })
        .map(|state| serde_json::from_value(state.payload["frozen"].clone()).unwrap())
        .collect();
    assert_eq!(frozen[1].candidate.origin_seq, 2);
    assert_eq!(
        frozen[1].candidate.previous_mutation_id,
        Some(frozen[0].candidate.mutation_id)
    );
}

#[test]
fn library_durability_memory_create_retry_completes_the_saved_playlist() {
    let db = MemoryDatabase::new();
    let control = MemoryControlPlane::new();
    let root = LibraryRootId::new();
    let author = db.current_device_id();
    control
        .write_manifest(
            root,
            &LibraryManifest::new(LibraryId::from_uuid(uuid::Uuid::new_v4()), "test"),
        )
        .unwrap();
    let first = PlaylistId::new();
    db.set_fail_commit(true);
    let error = db
        .create_library_playlist(&control, root, first, "retry me", author)
        .unwrap_err();
    let Error::LibrarySavedPending { operation, .. } = &error else {
        panic!("saved pending expected: {error:?}");
    };
    let operation = *operation;
    // A retry under the same unique name reports the SAME saved operation and
    // adds no second directory commit, even while projection still fails.
    let retry = db
        .create_library_playlist(&control, root, PlaylistId::new(), "retry me", author)
        .unwrap_err();
    let Error::LibrarySavedPending {
        operation: retried, ..
    } = &retry
    else {
        panic!("the same saved operation expected: {retry:?}");
    };
    assert_eq!(*retried, operation);
    assert_eq!(control.recover(root).unwrap().commits.len(), 1);
    db.set_fail_commit(false);
    // Once projection heals, a retry completes the saved creation under its
    // original identity instead of minting a second one.
    let completed = db
        .create_library_playlist(&control, root, PlaylistId::new(), "retry me", author)
        .unwrap();
    assert_eq!(completed, first);
    assert_eq!(db.name(first).unwrap().as_deref(), Some("retry me"));
    assert_eq!(control.recover(root).unwrap().commits.len(), 1);
}
