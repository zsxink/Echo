//! Exact incremental retries use the durable completed snapshot checkpoint.

use echo_core::application::ports::{
    DeviceIdProvider, HostCommitLedgerPort, LibraryRepository, PublishedCommit, RemoteApplyPort,
    RemoteCommit, ReplicaSnapshotApplyPort, ReplicaSnapshotPage,
};
use echo_core::domain::entities::LibraryRoot;
use echo_core::domain::ids::{LibraryRootId, SongId};
use echo_core::domain::sync::{
    AuthorityId, CommitCheckpoint, CommitSeq, DeviceId, Epoch, LibraryId, MutationCandidate,
    MutationId, ObjectKey, SyncBinding,
};
use echo_core::infrastructure::sqlite::SqliteDatabase;
use serde::Deserialize;

fn database(
    directory: &std::path::Path,
    name: &str,
    binding: SyncBinding,
    host: bool,
) -> SqliteDatabase {
    let database = SqliteDatabase::open(directory.join(format!("{name}.db"))).unwrap();
    let root = LibraryRootId::new();
    LibraryRepository::upsert(
        &database,
        &LibraryRoot::new(root, directory.join(name), true, true),
    )
    .unwrap();
    database.bind_sync_library(binding, host, root).unwrap();
    database
}

#[derive(Deserialize)]
struct CanonicalCommit {
    mutation: MutationCandidate,
}

fn remote(published: PublishedCommit) -> RemoteCommit {
    let body: CanonicalCommit = serde_json::from_str(&published.canonical_json).unwrap();
    RemoteCommit {
        sequence: published.sequence,
        previous_digest: published.previous_digest,
        digest: published.digest,
        origin_device_id: body.mutation.origin_device_id,
        mutation_id: body.mutation.mutation_id,
        object: body.mutation.object,
        payload_json: serde_json::to_string(&body.mutation.payload).unwrap(),
        canonical_json: published.canonical_json,
        is_tombstone: true,
    }
}

fn published_chain(host: &SqliteDatabase, binding: SyncBinding) -> Vec<RemoteCommit> {
    let origin = DeviceId::from_uuid(host.current_device_id().as_uuid());
    let mut previous = None;
    for sequence in 1..=3 {
        let candidate = MutationCandidate {
            resource_refs: Vec::new(),
            mutation_id: MutationId::new(),
            origin_device_id: origin,
            origin_seq: sequence,
            previous_mutation_id: previous,
            object: ObjectKey::Song(SongId::new()),
            base_object_version: None,
            payload: serde_json::json!({"tombstone":true}),
        };
        let staged = host.stage_confirmation(binding, &candidate).unwrap();
        host.mark_published(staged.checkpoint.unwrap()).unwrap();
        previous = Some(candidate.mutation_id);
    }
    host.published_commits_after(binding, CommitSeq::GENESIS, [0; 32], 3)
        .unwrap()
        .into_iter()
        .map(remote)
        .collect()
}

struct Enrollment {
    directory: tempfile::TempDir,
    replica: SqliteDatabase,
    binding: SyncBinding,
    peer: DeviceId,
    anchor: CommitCheckpoint,
    commits: Vec<RemoteCommit>,
}

fn enroll() -> Enrollment {
    let directory = tempfile::tempdir().unwrap();
    let binding = SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    let host = database(directory.path(), "host", binding, true);
    let replica = database(directory.path(), "replica", binding, false);
    let peer = DeviceId::from_uuid(host.current_device_id().as_uuid());
    let commits = published_chain(&host, binding);
    let anchor = CommitCheckpoint {
        binding,
        sequence: commits[0].sequence,
        digest: commits[0].digest,
    };
    let page = ReplicaSnapshotPage {
        snapshot_token: uuid::Uuid::new_v4().simple().to_string(),
        checkpoint: anchor,
        page_index: 0,
        next_page_index: None,
        records: Vec::new(),
    };
    assert_eq!(
        replica.apply_snapshot_page(peer, &page).unwrap().checkpoint,
        Some(anchor)
    );
    let direct = rusqlite::Connection::open(directory.path().join("replica.db")).unwrap();
    assert_eq!(
        direct
            .query_row("SELECT COUNT(*) FROM sync_commits", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        0
    );
    Enrollment {
        directory,
        replica,
        binding,
        peer,
        anchor,
        commits: commits[1..].to_vec(),
    }
}

#[test]
fn exact_post_enrollment_incremental_retry_is_noop_after_restart() {
    let enrollment = enroll();
    let applied = enrollment
        .replica
        .apply_remote_batch(
            enrollment.binding,
            enrollment.peer,
            enrollment.anchor,
            &enrollment.commits,
        )
        .unwrap();
    assert_eq!(applied.applied, 2);
    assert_eq!(applied.sequence, CommitSeq::from_u64(3));
    assert_eq!(
        enrollment
            .replica
            .apply_remote_batch(
                enrollment.binding,
                enrollment.peer,
                enrollment.anchor,
                &enrollment.commits,
            )
            .unwrap()
            .applied,
        0
    );
    drop(enrollment.replica);
    let replica = SqliteDatabase::open(enrollment.directory.path().join("replica.db")).unwrap();
    let retry = replica
        .apply_remote_batch(
            enrollment.binding,
            enrollment.peer,
            enrollment.anchor,
            &enrollment.commits,
        )
        .unwrap();
    assert_eq!(retry.applied, 0);
    assert_eq!(retry.sequence, applied.sequence);
    assert_eq!(retry.digest, applied.digest);
    let direct =
        rusqlite::Connection::open(enrollment.directory.path().join("replica.db")).unwrap();
    assert_eq!(
        direct
            .query_row(
                "SELECT COUNT(*) FROM sync_commits WHERE commit_seq=1",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    assert_eq!(
        direct
            .query_row("SELECT COUNT(*) FROM sync_commits", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        2
    );
}

#[test]
fn independently_valid_fork_cannot_replace_completed_snapshot_anchor() {
    let enrollment = enroll();
    let applied = enrollment
        .replica
        .apply_remote_batch(
            enrollment.binding,
            enrollment.peer,
            enrollment.anchor,
            &enrollment.commits,
        )
        .unwrap();
    // A second real host produces a fully hashed chain for the same binding.
    // Its different H digest must fail the durable anchor check, even though
    // its H+1/H+2 sequence and canonical commit digests are internally valid.
    let other = database(
        enrollment.directory.path(),
        "fork",
        enrollment.binding,
        true,
    );
    let fork = published_chain(&other, enrollment.binding);
    let wrong_anchor = CommitCheckpoint {
        binding: enrollment.binding,
        sequence: fork[0].sequence,
        digest: fork[0].digest,
    };
    assert_ne!(wrong_anchor.digest, enrollment.anchor.digest);
    let error = enrollment
        .replica
        .apply_remote_batch(
            enrollment.binding,
            enrollment.peer,
            wrong_anchor,
            &fork[1..],
        )
        .unwrap_err();
    assert!(error.to_string().contains("replayed remote anchor differs"));
    let retry = enrollment
        .replica
        .apply_remote_batch(
            enrollment.binding,
            enrollment.peer,
            enrollment.anchor,
            &enrollment.commits,
        )
        .unwrap();
    assert_eq!(retry.applied, 0);
    assert_eq!(retry.sequence, applied.sequence);
    assert_eq!(retry.digest, applied.digest);
}
