//! Replica cursor validation, including empty responses and exact retries.

use echo_core::application::ports::{
    DeviceIdProvider, HostCommitLedgerPort, LibraryRepository, RemoteApplyPort, RemoteCommit,
};
use echo_core::domain::entities::LibraryRoot;
use echo_core::domain::ids::{LibraryRootId, SongId};
use echo_core::domain::sync::*;
use echo_core::infrastructure::sqlite::SqliteDatabase;

#[test]
fn empty_response_cannot_hide_a_fork_or_roll_back_the_anchor() {
    let (_directory, replica, binding, peer, commits) = prepared_replica();
    let genesis = CommitCheckpoint {
        binding,
        sequence: CommitSeq::GENESIS,
        digest: [0; 32],
    };
    let applied = replica
        .apply_remote_batch(binding, peer, genesis, &commits)
        .unwrap();
    let current = CommitCheckpoint {
        binding,
        sequence: applied.sequence,
        digest: applied.digest,
    };
    assert_eq!(
        replica
            .apply_remote_batch(binding, peer, current, &[])
            .unwrap()
            .applied,
        0
    );
    assert!(replica
        .apply_remote_batch(
            binding,
            peer,
            CommitCheckpoint {
                digest: [9; 32],
                ..current
            },
            &[]
        )
        .is_err());
    assert!(replica
        .apply_remote_batch(binding, peer, genesis, &[])
        .is_err());
    let first = CommitCheckpoint {
        binding,
        sequence: commits[0].sequence,
        digest: commits[0].digest,
    };
    assert!(replica
        .apply_remote_batch(binding, peer, first, &[])
        .is_err());
    assert!(replica
        .apply_remote_batch(
            binding,
            peer,
            CommitCheckpoint {
                sequence: CommitSeq::from_u64(3),
                ..current
            },
            &[]
        )
        .is_err());
    assert_eq!(
        replica
            .apply_remote_batch(binding, peer, genesis, &commits)
            .unwrap()
            .applied,
        0
    );
    assert_eq!(
        replica
            .apply_remote_batch(binding, peer, first, &commits[1..])
            .unwrap()
            .applied,
        0
    );
    assert!(replica
        .apply_remote_batch(
            binding,
            peer,
            CommitCheckpoint {
                digest: [9; 32],
                ..first
            },
            &commits[1..]
        )
        .is_err());
    assert_eq!(
        replica
            .apply_remote_batch(binding, peer, current, &[])
            .unwrap()
            .digest,
        current.digest
    );
}

fn prepared_replica() -> (
    tempfile::TempDir,
    SqliteDatabase,
    SyncBinding,
    DeviceId,
    Vec<RemoteCommit>,
) {
    let directory = tempfile::tempdir().unwrap();
    let host = SqliteDatabase::open(directory.path().join("host.db")).unwrap();
    let replica = SqliteDatabase::open(directory.path().join("replica.db")).unwrap();
    let binding = SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    for (db, is_host, name) in [(&host, true, "host"), (&replica, false, "replica")] {
        let root = LibraryRootId::new();
        LibraryRepository::upsert(
            db,
            &LibraryRoot::new(root, directory.path().join(name), true, true),
        )
        .unwrap();
        db.bind_sync_library(binding, is_host, root).unwrap();
    }
    let origin = DeviceId::from_uuid(host.current_device_id().as_uuid());
    let peer = DeviceId::new();
    let mut predecessor = None;
    for sequence in 1..=2 {
        let candidate = MutationCandidate {
            resource_refs: Vec::new(),
            mutation_id: MutationId::new(),
            origin_device_id: origin,
            origin_seq: sequence,
            previous_mutation_id: predecessor,
            object: ObjectKey::Song(SongId::new()),
            base_object_version: None,
            payload: serde_json::json!({"tombstone": true}),
        };
        let staged = host.stage_confirmation(binding, &candidate).unwrap();
        host.mark_published(staged.checkpoint.unwrap()).unwrap();
        predecessor = Some(candidate.mutation_id);
    }
    let commits: Vec<_> = host
        .published_commits_after(binding, CommitSeq::GENESIS, [0; 32], 2)
        .unwrap()
        .into_iter()
        .map(|published| {
            let body: serde_json::Value = serde_json::from_str(&published.canonical_json).unwrap();
            RemoteCommit {
                sequence: published.sequence,
                previous_digest: published.previous_digest,
                digest: published.digest,
                origin_device_id: serde_json::from_value(
                    body["mutation"]["origin_device_id"].clone(),
                )
                .unwrap(),
                mutation_id: serde_json::from_value(body["mutation"]["mutation_id"].clone())
                    .unwrap(),
                object: serde_json::from_value(body["mutation"]["object"].clone()).unwrap(),
                payload_json: body["mutation"]["payload"].to_string(),
                canonical_json: published.canonical_json,
                is_tombstone: true,
            }
        })
        .collect();
    (directory, replica, binding, peer, commits)
}
