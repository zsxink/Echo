//! Focused favorite pages retain the same recoverable state as complete capture.
use super::*;
use crate::application::ports::{
    DeviceIdProvider, HostCommitLedgerPort, LibrarySyncRecoveryPort, RemoteApplyPort, RemoteCommit,
};
use crate::domain::library::durability::{
    LibraryCommit, RecoverySessionRole, StateTarget, SyncRecoveryContext,
};
use rusqlite::OptionalExtension;

struct Feed {
    _directory: tempfile::TempDir,
    db: SqliteDatabase,
    anchor: CommitCheckpoint,
    previous: Option<MutationId>,
}

impl Feed {
    fn new(binding: SyncBinding) -> Self {
        let (directory, db, root) = database();
        db.bind_sync_library(binding, true, root).unwrap();
        Self {
            _directory: directory,
            db,
            anchor: CommitCheckpoint {
                binding,
                sequence: CommitSeq::GENESIS,
                digest: [0; 32],
            },
            previous: None,
        }
    }

    fn append(&mut self, object: ObjectKey, payload: serde_json::Value) -> RemoteCommit {
        let (kind, id) = match object {
            ObjectKey::Song(song) => ("song", song.to_string()),
            ObjectKey::Favorite(song) => ("favorite", song.to_string()),
            _ => unreachable!(),
        };
        let base = self.db.with_reader(move |c| {
            c.query_row("SELECT object_version FROM sync_object_versions WHERE object_type=?1 AND object_uuid=?2", params![kind,id], |row| row.get::<_, u64>(0))
                .optional().map_err(storage)
        }).unwrap().map(|seq| ObjectVersion::from_commit_seq(CommitSeq::from_u64(seq)));
        let candidate = MutationCandidate {
            resource_refs: Vec::new(),
            mutation_id: MutationId::new(),
            origin_device_id: DeviceId::from_uuid(self.db.current_device_id().as_uuid()),
            origin_seq: self.anchor.sequence.as_u64() + 1,
            previous_mutation_id: self.previous,
            object,
            base_object_version: base,
            payload,
        };
        let checkpoint = self
            .db
            .stage_confirmation(self.anchor.binding, &candidate)
            .unwrap()
            .checkpoint
            .unwrap();
        self.db.mark_published(checkpoint).unwrap();
        let published = self
            .db
            .published_commits_after(
                self.anchor.binding,
                self.anchor.sequence,
                self.anchor.digest,
                1,
            )
            .unwrap();
        let raw = &published[0];
        let body: serde_json::Value = serde_json::from_str(&raw.canonical_json).unwrap();
        let commit = RemoteCommit {
            sequence: raw.sequence,
            previous_digest: raw.previous_digest,
            digest: raw.digest,
            origin_device_id: candidate.origin_device_id,
            mutation_id: candidate.mutation_id,
            object,
            payload_json: body.pointer("/mutation/payload").unwrap().to_string(),
            canonical_json: raw.canonical_json.clone(),
            is_tombstone: false,
        };
        self.anchor = checkpoint;
        self.previous = Some(candidate.mutation_id);
        commit
    }
}

fn apply(replica: &Replica, anchor: CommitCheckpoint, commits: &[RemoteCommit]) {
    replica
        .db
        .apply_remote_batch(replica.binding, replica.peer, anchor, commits)
        .unwrap();
}

fn anchor(binding: SyncBinding, commit: Option<&RemoteCommit>) -> CommitCheckpoint {
    CommitCheckpoint {
        binding,
        sequence: commit.map_or(CommitSeq::GENESIS, |commit| commit.sequence),
        digest: commit.map_or([0; 32], |commit| commit.digest),
    }
}

fn replay(replica: &Replica) -> LibraryReplay {
    replica
        .plane
        .durability()
        .unwrap()
        .recover(replica.root)
        .unwrap()
}

fn full_reference(replica: &Replica, focused: &LibraryReplay) -> LibraryReplay {
    let mut full = focused.clone();
    let root = replica.root;
    let (records, state) = replica
        .db
        .with_reader(move |c| {
            Ok((
                crate::infrastructure::sqlite::durability_backfill::export_library_records(
                    c, root,
                )?,
                crate::infrastructure::sqlite::sync::durability::export_sync_state(c, root)?,
            ))
        })
        .unwrap();
    let last = full.commits.last_mut().unwrap();
    last.batch.records = records;
    last.batch.state = state;
    last.digest = LibraryCommit::calculate_digest(
        &last.batch,
        last.library_commit_seq,
        last.predecessor_digest.as_deref(),
    )
    .unwrap();
    full
}

fn rebuilt(replica: &Replica, replay: &LibraryReplay, name: &str) -> SqliteDatabase {
    let db = SqliteDatabase::open(replica.directory.path().join(name)).unwrap();
    LibraryRepository::upsert(
        &db,
        &LibraryRoot::new(
            replica.root,
            replica.directory.path().join("library"),
            true,
            true,
        ),
    )
    .unwrap();
    // Explicit fixture credentials keep active protocol facts available for a
    // comparison; directory records alone never authorize a network binding.
    db.bind_sync_library(replica.binding, false, replica.root)
        .unwrap();
    db.configure_library_control(replica.plane.clone());
    db.project_library_commits(replica.root, replay).unwrap();
    db
}

fn snapshot(db: &SqliteDatabase, root: LibraryRootId) -> (Vec<String>, Vec<String>, Vec<String>) {
    let retained = db.read_recovery_records(root).unwrap();
    let mut records = retained
        .records
        .iter()
        .map(|r| serde_json::to_string(r).unwrap())
        .collect::<Vec<_>>();
    let mut states = retained
        .state
        .iter()
        .map(|s| serde_json::to_string(s).unwrap())
        .collect::<Vec<_>>();
    records.sort();
    states.sort();
    let rows = db.with_reader(|c| {
        let mut statement = c.prepare("SELECT json_array(uuid,relative_path,added_at,is_favorite,favorited_at,play_count,title,artist,album) FROM songs ORDER BY uuid").map_err(storage)?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0)).map_err(storage)?;
        rows.collect::<Result<Vec<_>,_>>().map_err(storage)
    }).unwrap();
    (records, states, rows)
}

#[test]
fn focused_favorite_page_is_bounded_and_rebuilds_like_full_capture() {
    let replica = replica();
    let mut feed = Feed::new(replica.binding);
    let song_ids = (0..24).map(|_| SongId::new()).collect::<Vec<_>>();
    let songs = song_ids.iter().map(|id| feed.append(ObjectKey::Song(*id), serde_json::json!({"song_uuid":id,"relative_path":format!("{id}.flac"),"title":format!("song-{id}"),"added_at":31}))).collect::<Vec<_>>();
    apply(&replica, anchor(replica.binding, None), &songs);
    let previous = anchor(replica.binding, songs.last());
    let mut commits = Vec::new();
    for (song, favorite, time) in [
        (song_ids[0], true, Some(11)),
        (song_ids[1], true, Some(22)),
        (song_ids[0], false, None),
    ] {
        commits.push(feed.append(
            ObjectKey::Favorite(song),
            serde_json::json!({"song_uuid":song,"is_favorite":favorite,"favorited_at":time}),
        ));
    }
    apply(&replica, previous, &commits);
    let focused = replay(&replica);
    let last = &focused.commits.last().unwrap().batch;
    assert_eq!(
        last.records.len(),
        2,
        "one after-image per affected favorite, independent of 24 songs"
    );
    assert!(last
        .records
        .iter()
        .all(|record| matches!(record.record(), PortableRecord::Favorite(_))));
    assert_eq!(
        last.state.len(),
        9,
        "3 ledgers + 3 receipts + 2 final confirmations + 1 cursor"
    );
    assert_eq!(
        last.state
            .iter()
            .filter(|state| state.kind == StateKind::Confirmed)
            .count(),
        2
    );
    assert!(last.resources.is_empty());
    for commit in &commits {
        assert!(last
            .state
            .iter()
            .any(|state| state.key.as_ref().is_some_and(|key| key.target
                == StateTarget::Ledger {
                    author: commit.origin_device_id,
                    mutation_id: commit.mutation_id
                })
                && state.payload["original_body"] == commit.canonical_json));
    }
    let full = full_reference(&replica, &focused);
    assert_eq!(
        full.commits.last().unwrap().batch.records.len(),
        song_ids.len() * 3
    );
    let focused_db = rebuilt(&replica, &focused, "focused.sqlite");
    let full_db = rebuilt(&replica, &full, "full.sqlite");
    assert_eq!(
        snapshot(&focused_db, replica.root),
        snapshot(&full_db, replica.root)
    );
    assert_eq!(
        focused_db
            .read_recovery_records(replica.root)
            .unwrap()
            .state
            .iter()
            .filter(|state| state.kind == StateKind::SyncLedger)
            .count(),
        songs.len() + commits.len(),
        "all original ledger facts survive in retained recovery state"
    );
    let recovery = [&focused_db, &full_db].map(|db| {
        db.retry_library_sync_state(SyncRecoveryContext {
            root: replica.root,
            local_library_id: replica
                .plane
                .read_manifest(replica.root)
                .unwrap()
                .unwrap()
                .library_id,
            local_author: DeviceId::from_uuid(db.current_device_id().as_uuid()),
            role: RecoverySessionRole::Replica,
            verified_checkpoint: feed.anchor,
        })
        .map(|_| ())
        .map_err(|error| error.to_string())
    });
    // Both captures retain the original replica v1 `{applied:true}` receipt.
    // The current recovery adapter rejects that incomplete outcome with
    // `missing field mutation_id`; this assertion records equivalence, not a
    // claim that live protocol recovery succeeded.
    assert_eq!(recovery[0], recovery[1]);
    assert!(recovery[0]
        .as_ref()
        .unwrap_err()
        .contains("missing field `mutation_id`"));
    focused_db
        .project_library_commits(replica.root, &focused)
        .unwrap();
    assert_eq!(
        snapshot(&focused_db, replica.root),
        snapshot(&full_db, replica.root),
        "replaying the same head is idempotent"
    );
    assert_eq!(
        replica
            .db
            .apply_remote_batch(replica.binding, replica.peer, previous, &commits)
            .unwrap()
            .applied,
        0
    );
    assert_eq!(
        replay(&replica),
        focused,
        "retry keeps the original immutable directory batch"
    );
}

#[test]
fn focused_favorite_page_without_parent_falls_back_and_waiting_state_survives() {
    let replica = replica();
    let mut feed = Feed::new(replica.binding);
    let id = SongId::new();
    let favorite = feed.append(
        ObjectKey::Favorite(id),
        serde_json::json!({"song_uuid":id,"is_favorite":true,"favorited_at":71}),
    );
    apply(
        &replica,
        anchor(replica.binding, None),
        std::slice::from_ref(&favorite),
    );
    assert!(
        replay(&replica)
            .commits
            .last()
            .unwrap()
            .batch
            .records
            .is_empty(),
        "missing parent is preserved in sync evidence, never synthesized"
    );
    let song = feed.append(
        ObjectKey::Song(id),
        serde_json::json!({"song_uuid":id,"relative_path":format!("{id}.flac"),"added_at":17}),
    );
    apply(
        &replica,
        anchor(replica.binding, Some(&favorite)),
        std::slice::from_ref(&song),
    );
    let image = replay(&replica);
    let last = &image.commits.last().unwrap().batch;
    assert_eq!(
        last.records.len(),
        3,
        "Song pages use full capture and include deferred favorite state"
    );
    assert!(last.records.iter().any(|record| matches!(record.record(), PortableRecord::Favorite(record) if record.song_uuid == id.as_uuid() && record.is_favorite && record.favorited_at == Some(71))));
    let fresh = rebuilt(&replica, &image, "waiting.sqlite");
    assert!(SongRepository::by_id(&fresh, id)
        .unwrap()
        .unwrap()
        .favorite());
}

#[test]
fn focused_favorite_page_with_matching_mutation_keeps_the_terminal_disposition() {
    let replica = replica();
    let mut feed = Feed::new(replica.binding);
    let id = SongId::new();
    let song = feed.append(
        ObjectKey::Song(id),
        serde_json::json!({"song_uuid":id,"relative_path":format!("{id}.flac"),"added_at":17}),
    );
    apply(
        &replica,
        anchor(replica.binding, None),
        std::slice::from_ref(&song),
    );
    let favorite = feed.append(
        ObjectKey::Favorite(id),
        serde_json::json!({"song_uuid":id,"is_favorite":true,"favorited_at":71}),
    );
    let commit = favorite.clone();
    let root = replica.root;
    replica.db.writer.run(move |c| {
        let body: serde_json::Value = serde_json::from_str(&commit.canonical_json).unwrap();
        let mutation = &body["mutation"];
        c.execute(
            "INSERT INTO sync_mutations(mutation_id,origin_device_id,origin_seq,previous_mutation_id,object_type,object_uuid,operation,local_revision,base_object_version,payload_json,payload_digest,status,created_at,resource_refs_json,local_root_uuid) VALUES(?1,?2,?3,?4,'favorite',?5,'upsert',1,?6,?7,?8,'pending',10,'[]',?9)",
            params![commit.mutation_id.as_uuid().to_string(), commit.origin_device_id.as_uuid().to_string(), mutation["origin_seq"].as_u64(), mutation["previous_mutation_id"].as_str(), id.to_string(), mutation["base_object_version"].as_u64(), commit.payload_json, blake3::hash(commit.payload_json.as_bytes()).to_hex().to_string(), root.to_string()],
        ).map_err(storage)?;
        Ok(())
    }).unwrap();
    apply(
        &replica,
        anchor(replica.binding, Some(&song)),
        std::slice::from_ref(&favorite),
    );
    let image = replay(&replica);
    let batch = &image.commits.last().unwrap().batch;
    assert_eq!(
        batch.records.len(),
        3,
        "a matching local mutation forces complete capture even with an unchanged parent Song"
    );
    assert!(batch
        .state
        .iter()
        .any(|state| state.kind == StateKind::PendingDisposition
            && state.payload["disposition"]["mutation_id"]
                == serde_json::json!(favorite.mutation_id.as_uuid())
            && state.payload["disposition"]["status"] == "confirmed"));
    assert!(batch
        .state
        .iter()
        .any(|state| state.kind == StateKind::Pending
            && state.payload["frozen"]["original_body"] == favorite.payload_json));
    let full = full_reference(&replica, &image);
    let actual = rebuilt(&replica, &image, "disposition.sqlite");
    let expected = rebuilt(&replica, &full, "disposition-full.sqlite");
    assert_eq!(
        snapshot(&actual, replica.root),
        snapshot(&expected, replica.root)
    );
}
