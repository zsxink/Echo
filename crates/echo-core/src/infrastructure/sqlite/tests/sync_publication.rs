use super::*;
use crate::application::ports::{
    HostCommitLedgerPort, HostPeerAuthorizationPort, HostSnapshotPort, PublicationCheckpointPort,
    StagedConfirmation,
};
use crate::application::sync::ConfirmHostMutation;
use crate::domain::sync::{
    AuthorityId, CommitCheckpoint, CommitSeq, DeviceId, Epoch, LibraryId, MutationCandidate,
    MutationId, ObjectKey, SyncBinding,
};
use std::sync::{Barrier, Mutex};

fn host() -> (tempfile::TempDir, SqliteDatabase, SyncBinding) {
    let (directory, database, root) = database();
    let binding = SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    database.bind_sync_library(binding, true, root).unwrap();
    (directory, database, binding)
}

fn candidate(origin: DeviceId) -> MutationCandidate {
    let song = SongId::new();
    MutationCandidate {
        resource_refs: Vec::new(),
        mutation_id: MutationId::new(),
        origin_device_id: origin,
        origin_seq: 1,
        previous_mutation_id: None,
        object: ObjectKey::Song(song),
        base_object_version: None,
        payload: serde_json::json!({
            "song_uuid": song,
            "relative_path": format!("{song}.flac"),
            "title": "publication test",
            "added_at": 1,
        }),
    }
}

fn next(first: &MutationCandidate) -> MutationCandidate {
    MutationCandidate {
        origin_seq: first.origin_seq + 1,
        previous_mutation_id: Some(first.mutation_id),
        ..candidate(first.origin_device_id)
    }
}

fn local_candidate(database: &SqliteDatabase) -> MutationCandidate {
    candidate(DeviceId::from_uuid(database.current_device_id().as_uuid()))
}

#[derive(Default)]
struct Checkpoint {
    fail_once: Mutex<bool>,
    published: Mutex<Vec<CommitCheckpoint>>,
}

impl PublicationCheckpointPort for Checkpoint {
    fn advance(&self, checkpoint: CommitCheckpoint) -> Result<(), Error> {
        let mut fail = self.fail_once.lock().unwrap();
        if std::mem::take(&mut *fail) {
            return Err(Error::unavailable(
                "checkpoint",
                "injected persistence failure",
            ));
        }
        drop(fail);
        let mut published = self.published.lock().unwrap();
        if let Some(last) = published.last() {
            if last.binding != checkpoint.binding {
                return Err(Error::conflict("checkpoint binding mismatch"));
            }
            if checkpoint.sequence < last.sequence {
                return Ok(());
            }
            if checkpoint.sequence == last.sequence {
                return if checkpoint.digest == last.digest {
                    Ok(())
                } else {
                    Err(Error::conflict("checkpoint digest mismatch"))
                };
            }
        }
        published.push(checkpoint);
        drop(published);
        Ok(())
    }
}

#[test]
fn a_held_poll_recheck_reads_without_taking_the_writer() {
    let (_directory, database, binding) = host();
    let first = local_candidate(&database);
    ConfirmHostMutation::new(&database, &Checkpoint::default())
        .execute(binding, &first)
        .unwrap();
    let writes = database.subscribe_writes();
    while writes.try_recv().is_ok() {}
    // A held long-poll re-reads its page every 500 ms for each watching peer.
    // On the writer actor those reads queue behind confirmations, whose fsync
    // and checkpoint advance are exactly what the peer is waiting on.
    let page = database
        .published_commits_after(binding, CommitSeq::GENESIS, [0; 32], 10)
        .unwrap();
    assert_eq!(page.len(), 1);
    assert!(
        writes.try_recv().is_err(),
        "a recheck must not occupy the single writer actor"
    );
}

#[test]
fn checkpoint_failure_blocks_later_ack_until_pending_predecessor_is_recovered() {
    let (directory, database, binding) = host();
    let first = local_candidate(&database);
    let second = next(&first);
    let checkpoint = Checkpoint::default();
    *checkpoint.fail_once.lock().unwrap() = true;
    let confirm = ConfirmHostMutation::new(&database, &checkpoint);
    assert!(confirm.execute(binding, &first).is_err());
    let pending = database.pending_publications(10).unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(
        database.stage_confirmation(binding, &first).unwrap(),
        pending[0]
    );
    assert!(matches!(
        confirm.execute(binding, &second),
        Err(Error::Unavailable { .. })
    ));
    assert!(checkpoint.published.lock().unwrap().is_empty());
    assert!(database
        .published_commits_after(binding, CommitSeq::GENESIS, [0; 32], 10)
        .unwrap()
        .is_empty());
    let sizes: (i64, i64, i64, i64) = database.with_reader(|connection| {
        connection.query_row(
            "SELECT (SELECT COUNT(*) FROM sync_commits), (SELECT COUNT(*) FROM sync_inbox), (SELECT COUNT(*) FROM sync_object_versions), (SELECT MAX(origin_seq) FROM sync_origin_cursors)",
            [], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
        ).map_err(super::super::support::storage)
    }).unwrap();
    assert_eq!(
        sizes,
        (1, 1, 1, 1),
        "backpressure must not partially consume a candidate"
    );

    drop(database);
    let reopened = SqliteDatabase::open(directory.path().join("echo.db")).unwrap();
    let confirm = ConfirmHostMutation::new(&reopened, &checkpoint);
    assert_eq!(confirm.recover_publications(10).unwrap(), 1);
    assert_eq!(
        confirm
            .execute(binding, &second)
            .unwrap()
            .checkpoint
            .unwrap()
            .sequence,
        CommitSeq::from_u64(2)
    );
    let commits = reopened
        .published_commits_after(binding, CommitSeq::GENESIS, [0; 32], 10)
        .unwrap();
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[1].previous_digest, commits[0].digest);
    assert_eq!(
        checkpoint
            .published
            .lock()
            .unwrap()
            .iter()
            .map(|value| value.sequence.as_u64())
            .collect::<Vec<_>>(),
        [1, 2]
    );
}

#[test]
fn earlier_published_mutation_retry_preserves_monotone_platform_checkpoint() {
    let (_directory, database, binding) = host();
    let first = local_candidate(&database);
    let checkpoint = Checkpoint::default();
    let confirm = ConfirmHostMutation::new(&database, &checkpoint);
    let accepted = confirm.execute(binding, &first).unwrap();
    let later = confirm.execute(binding, &next(&first)).unwrap();
    // A platform checkpoint implements the monotone high-water contract, even
    // when an older accepted inbox receipt is retried after later publication.
    assert_eq!(confirm.execute(binding, &first).unwrap(), accepted);
    assert_eq!(
        checkpoint.published.lock().unwrap().last().copied(),
        later.checkpoint
    );
    assert_eq!(checkpoint.published.lock().unwrap().len(), 2);
}

#[test]
fn concurrent_origins_admit_only_one_pending_publication() {
    let (_directory, database, binding) = host();
    let peer = DeviceId::new();
    database
        .authorize_sync_peer(
            binding,
            peer,
            "Peer".to_owned(),
            "secure-store-ref".to_owned(),
        )
        .unwrap();
    let candidates = [local_candidate(&database), candidate(peer)];
    let database = Arc::new(database);
    let barrier = Arc::new(Barrier::new(3));
    // Spawn all participants before entering the barrier; a lazy iterator
    // would defer spawning and leave the first participant waiting forever.
    let mut handles = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        let database = Arc::clone(&database);
        let barrier = Arc::clone(&barrier);
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            let result = database.stage_confirmation(binding, &candidate);
            (candidate, result)
        }));
    }
    barrier.wait();
    let results: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();
    assert_eq!(
        results.iter().filter(|(_, result)| result.is_ok()).count(),
        1
    );
    let accepted = results
        .iter()
        .find_map(|(_, result)| result.as_ref().ok())
        .unwrap();
    assert_eq!(
        database.pending_publications(10).unwrap().as_slice(),
        std::slice::from_ref(accepted)
    );
    let blocked = results.iter().find(|(_, result)| result.is_err()).unwrap();
    assert!(matches!(&blocked.1, Err(Error::Unavailable { .. })));
    database
        .mark_published(accepted.checkpoint.unwrap())
        .unwrap();
    let second = database.stage_confirmation(binding, &blocked.0).unwrap();
    assert_eq!(second.checkpoint.unwrap().sequence, CommitSeq::from_u64(2));
    database.mark_published(second.checkpoint.unwrap()).unwrap();
    assert_eq!(
        database
            .published_commits_after(binding, CommitSeq::GENESIS, [0; 32], 10)
            .unwrap()
            .len(),
        2
    );
}

fn two_staged(
    database: &SqliteDatabase,
    binding: SyncBinding,
) -> (StagedConfirmation, StagedConfirmation) {
    let first = local_candidate(database);
    let staged = database.stage_confirmation(binding, &first).unwrap();
    database.mark_published(staged.checkpoint.unwrap()).unwrap();
    (
        staged,
        database.stage_confirmation(binding, &next(&first)).unwrap(),
    )
}

#[test]
fn mark_published_cannot_skip_pending_predecessor_and_retries_are_idempotent() {
    let (_directory, database, binding) = host();
    let (first, second) = two_staged(&database, binding);
    // Simulate a pre-fix ledger with two staged commits after an interrupted
    // checkpoint write. Publication must repair them in order.
    database
        .writer
        .run(|connection| {
            connection
                .execute(
                    "UPDATE sync_commits SET publication_state='pending' WHERE commit_seq=1",
                    [],
                )
                .map_err(super::super::support::storage)?;
            Ok(())
        })
        .unwrap();
    let later_candidate: MutationCandidate = database
        .with_reader(|connection| {
            let body: String = connection
                .query_row(
                    "SELECT canonical_json FROM sync_commits WHERE commit_seq=2",
                    [],
                    |row| row.get(0),
                )
                .map_err(super::super::support::storage)?;
            let body: serde_json::Value =
                serde_json::from_str(&body).map_err(super::super::support::storage)?;
            serde_json::from_value(body["mutation"].clone()).map_err(super::super::support::storage)
        })
        .unwrap();
    let checkpoint = Checkpoint::default();
    assert!(ConfirmHostMutation::new(&database, &checkpoint)
        .execute(binding, &later_candidate)
        .is_err());
    assert!(checkpoint.published.lock().unwrap().is_empty());
    assert!(database.mark_published(second.checkpoint.unwrap()).is_err());
    assert_eq!(
        database.pending_publications(10).unwrap(),
        [first.clone(), second.clone()]
    );
    database.mark_published(first.checkpoint.unwrap()).unwrap();
    database.mark_published(first.checkpoint.unwrap()).unwrap();
    database.mark_published(second.checkpoint.unwrap()).unwrap();
    database.mark_published(first.checkpoint.unwrap()).unwrap();
    assert!(database.pending_publications(10).unwrap().is_empty());
}

#[test]
fn mark_published_rejects_missing_predecessor_without_updating_state() {
    let (_directory, database, binding) = host();
    let (_, second) = two_staged(&database, binding);
    database
        .writer
        .run(|connection| {
            connection
                .execute("DELETE FROM sync_commits WHERE commit_seq=1", [])
                .map_err(super::super::support::storage)?;
            Ok(())
        })
        .unwrap();
    assert!(database.mark_published(second.checkpoint.unwrap()).is_err());
    assert_eq!(database.pending_publications(10).unwrap(), [second]);
}

#[test]
fn mark_published_rejects_forked_predecessor_digest_without_updating_state() {
    let (_directory, database, binding) = host();
    let (_, second) = two_staged(&database, binding);
    database
        .writer
        .run(|connection| {
            connection
                .execute(
                    "UPDATE sync_commits SET previous_digest=randomblob(32) WHERE commit_seq=2",
                    [],
                )
                .map_err(super::super::support::storage)?;
            Ok(())
        })
        .unwrap();
    assert!(database.mark_published(second.checkpoint.unwrap()).is_err());
    assert_eq!(database.pending_publications(10).unwrap(), [second]);
}

#[test]
fn revoked_peer_cannot_replay_accepted_inbox_result() {
    let (_directory, database, binding) = host();
    let peer = DeviceId::new();
    database
        .authorize_sync_peer(
            binding,
            peer,
            "Peer".to_owned(),
            "secure-store-ref".to_owned(),
        )
        .unwrap();
    assert!(database.peer_is_authorized(binding, peer).unwrap());
    let first = candidate(peer);
    let accepted = database.stage_confirmation(binding, &first).unwrap();
    database.revoke_sync_peer(peer).unwrap();
    assert!(!database.peer_is_authorized(binding, peer).unwrap());
    assert!(!database
        .peer_is_authorized(
            SyncBinding {
                epoch: Epoch::from_u64(2),
                ..binding
            },
            peer,
        )
        .unwrap());
    assert!(matches!(
        database.stage_confirmation(binding, &first),
        Err(Error::Conflict { .. })
    ));
    // Startup recovery is independent of ingress authorization; revocation does
    // not remove an already accepted durable commit.
    let checkpoint = Checkpoint::default();
    assert_eq!(
        ConfirmHostMutation::new(&database, &checkpoint)
            .recover_publications(10)
            .unwrap(),
        1
    );
    assert_eq!(
        database
            .published_commits_after(binding, CommitSeq::GENESIS, [0; 32], 10)
            .unwrap()[0]
            .digest,
        accepted.checkpoint.unwrap().digest
    );
}

#[test]
fn genesis_pull_requires_matching_host_binding_even_with_an_empty_log() {
    let (_directory, database, root) = database();
    let binding = SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    assert!(database
        .published_commits_after(binding, CommitSeq::GENESIS, [0; 32], 10)
        .is_err());
    database.bind_sync_library(binding, true, root).unwrap();
    assert!(database
        .published_commits_after(binding, CommitSeq::GENESIS, [0; 32], 10)
        .unwrap()
        .is_empty());
    for mismatch in [
        SyncBinding {
            library_id: LibraryId::new(),
            ..binding
        },
        SyncBinding {
            authority_id: AuthorityId::new(),
            ..binding
        },
        SyncBinding {
            epoch: Epoch::from_u64(2),
            ..binding
        },
    ] {
        assert!(database
            .published_commits_after(mismatch, CommitSeq::GENESIS, [0; 32], 10)
            .is_err());
    }
    let (_directory, replica, root) = super::database();
    replica.bind_sync_library(binding, false, root).unwrap();
    assert!(replica
        .published_commits_after(binding, CommitSeq::GENESIS, [0; 32], 10)
        .is_err());
}

#[test]
fn sync_wakeup_only_follows_first_remote_publication() {
    let (_directory, database, binding) = host();
    let peer = DeviceId::new();
    database
        .authorize_sync_peer(binding, peer, "peer".into(), "vault-peer".into())
        .unwrap();
    let (wake, wakeups) = std::sync::mpsc::channel();
    database.register_sync_waker(wake);

    let local = local_candidate(&database);
    let local_checkpoint = database
        .stage_confirmation(binding, &local)
        .unwrap()
        .checkpoint
        .unwrap();
    database.mark_published(local_checkpoint).unwrap();
    assert!(
        wakeups.try_recv().is_err(),
        "host confirmation does not schedule another cycle"
    );

    let remote = candidate(peer);
    let remote_checkpoint = database
        .stage_confirmation(binding, &remote)
        .unwrap()
        .checkpoint
        .unwrap();
    assert!(
        wakeups.try_recv().is_err(),
        "staged but unpublished resources are not ready"
    );
    let mut invalid = remote_checkpoint;
    invalid.digest[0] ^= 1;
    database
        .mark_published(invalid)
        .expect_err("invalid checkpoint must fail");
    assert!(
        wakeups.try_recv().is_err(),
        "failed publication cannot wake sync"
    );
    database.mark_published(remote_checkpoint).unwrap();
    wakeups
        .try_recv()
        .expect("newly published peer work wakes the host");
    assert!(wakeups.try_recv().is_err());
    database.mark_published(remote_checkpoint).unwrap();
    assert!(
        wakeups.try_recv().is_err(),
        "publication retries cannot self-wake"
    );

    database.create_host_snapshot(binding, 32, 60_000).unwrap();
    assert!(
        wakeups.try_recv().is_err(),
        "snapshot session writes cannot self-wake"
    );
}
