//! Replica snapshot and incremental apply publish through the directory first.
//!
//! The other snapshot suites run without a control plane, where the commit tail
//! is a plain SQL commit. These tests install the real `RootControlPlane` so the
//! order itself is under test: staged pages stay private, the visible set and
//! the applied cursor appear only after the directory accepted the same records,
//! and a fault at either publication window is recoverable with one operation.
use super::*;
use crate::application::ports::{
    ControlPlanePort, HostSnapshotRecord, LibraryProjectionPort, ReplicaSnapshotApplyPort,
    ReplicaSnapshotPage,
};
use crate::domain::ids::{PlaylistId, PlaylistItemId};
use crate::domain::library::durability::{LibraryReplay, StateKind};
use crate::domain::library::{LibraryId as DirectoryLibraryId, LibraryManifest, PortableRecord};
use crate::domain::sync::{
    AuthorityId, CommitCheckpoint, CommitSeq, DeviceId, Epoch, LibraryId, MutationCandidate,
    MutationId, ObjectKey, ObjectVersion, PlaylistMemberAlias, SyncBinding,
};
use crate::infrastructure::filesystem::{RootControlPlane, RootRegistry};

struct Replica {
    directory: tempfile::TempDir,
    db: SqliteDatabase,
    binding: SyncBinding,
    peer: DeviceId,
    root: LibraryRootId,
    plane: Arc<RootControlPlane>,
}

fn replica() -> Replica {
    let (directory, db, root) = database();
    let binding = SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    db.bind_sync_library(binding, false, root).unwrap();
    let path = directory.path().join("library");
    std::fs::create_dir_all(&path).unwrap();
    let registry = RootRegistry::new();
    registry.register(root, &path);
    let plane = Arc::new(RootControlPlane::new(registry));
    plane
        .write_manifest(
            root,
            &LibraryManifest::new(
                DirectoryLibraryId::from_uuid(uuid::Uuid::new_v4()),
                "replica snapshot",
            ),
        )
        .unwrap();
    db.configure_library_control(plane.clone());
    Replica {
        directory,
        db,
        binding,
        peer: DeviceId::new(),
        root,
        plane,
    }
}

impl Replica {
    fn page(&self) -> ReplicaSnapshotPage {
        ReplicaSnapshotPage {
            snapshot_token: uuid::Uuid::new_v4().simple().to_string(),
            checkpoint: CommitCheckpoint {
                binding: self.binding,
                sequence: CommitSeq::from_u64(12),
                digest: [7; 32],
            },
            page_index: 0,
            next_page_index: None,
            records: Vec::new(),
        }
    }

    fn counts(&self) -> (i64, i64, i64) {
        self.db
            .with_reader(|c| {
                c.query_row(
                    "SELECT (SELECT COUNT(*) FROM songs),\
                     (SELECT COUNT(*) FROM playlists),\
                     (SELECT COUNT(*) FROM sync_peer_cursors)",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(storage)
            })
            .unwrap()
    }

    fn directory_commits(&self) -> usize {
        self.plane
            .durability()
            .unwrap()
            .recover(self.root)
            .unwrap()
            .commits
            .len()
    }
}

fn count(db: &SqliteDatabase, table: &str) -> i64 {
    db.with_reader(|connection| {
        connection
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .map_err(storage)
    })
    .unwrap()
}

/// Build a brand-new SQLite from the directory replay alone, then report its
/// visible rows, its applied watermark and any leftover private staging.
fn rebuild(replica: &Replica, replay: &LibraryReplay) -> (SqliteDatabase, (i64, i64, i64, i64)) {
    let fresh = SqliteDatabase::open(replica.directory.path().join("fresh.sqlite")).unwrap();
    LibraryRepository::upsert(
        &fresh,
        &LibraryRoot::new(
            replica.root,
            replica.directory.path().join("library"),
            true,
            true,
        ),
    )
    .unwrap();
    fresh.configure_library_control(Arc::new(replica.plane.as_ref().clone()));
    fresh.project_library_commits(replica.root, replay).unwrap();
    let rows = fresh
        .with_reader(|c| {
            c.query_row(
                "SELECT (SELECT COUNT(*) FROM songs),\
                 (SELECT COUNT(*) FROM playlists),\
                 (SELECT COUNT(*) FROM library_projection_watermarks),\
                 (SELECT COUNT(*) FROM sync_replica_snapshot_pages)",
                [],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .map_err(storage)
        })
        .unwrap();
    (fresh, rows)
}

fn object(key: ObjectKey, payload: serde_json::Value) -> HostSnapshotRecord {
    HostSnapshotRecord::Object {
        candidate: MutationCandidate {
            resource_refs: Vec::new(),
            mutation_id: MutationId::new(),
            origin_device_id: DeviceId::new(),
            origin_seq: 1,
            previous_mutation_id: None,
            object: key,
            base_object_version: None,
            payload,
        },
        version: ObjectVersion::from_commit_seq(CommitSeq::from_u64(4)),
        resources: Vec::new(),
    }
}

fn song(id: SongId) -> HostSnapshotRecord {
    object(
        ObjectKey::Song(id),
        serde_json::json!({
            "song_uuid":id,
            "relative_path":format!("{id}.flac"),
            "title":"Snapshot song",
            "added_at":77,
        }),
    )
}

/// An incremental page shares the same tail: its business rows and the applied
/// watermark become visible only after the directory accepted the same records.
#[test]
fn incremental_page_projects_rows_and_watermark_after_the_directory() {
    use crate::application::ports::{
        DeviceIdProvider, HostCommitLedgerPort, RemoteApplyPort, RemoteCommit,
    };
    use crate::domain::sync::CommitCheckpoint;

    let replica = replica();
    let (_host_directory, host, host_root) = database();
    host.bind_sync_library(replica.binding, true, host_root)
        .unwrap();
    let song_id = SongId::new();
    let origin = DeviceId::from_uuid(host.current_device_id().as_uuid());
    let candidate = MutationCandidate {
        resource_refs: Vec::new(),
        mutation_id: MutationId::new(),
        origin_device_id: origin,
        origin_seq: 1,
        previous_mutation_id: None,
        object: ObjectKey::Song(song_id),
        base_object_version: None,
        payload: serde_json::json!({
            "song_uuid":song_id,
            "relative_path":format!("{song_id}.flac"),
            "title":"Incremental song",
            "added_at":31,
        }),
    };
    let checkpoint = host
        .stage_confirmation(replica.binding, &candidate)
        .unwrap()
        .checkpoint
        .unwrap();
    host.mark_published(checkpoint).unwrap();
    let published = host
        .published_commits_after(replica.binding, CommitSeq::GENESIS, [0; 32], 4)
        .unwrap();
    let canonical: serde_json::Value = serde_json::from_str(&published[0].canonical_json).unwrap();
    let commit = RemoteCommit {
        sequence: published[0].sequence,
        previous_digest: published[0].previous_digest,
        digest: published[0].digest,
        origin_device_id: origin,
        mutation_id: candidate.mutation_id,
        object: candidate.object,
        payload_json: canonical.pointer("/mutation/payload").unwrap().to_string(),
        canonical_json: published[0].canonical_json.clone(),
        is_tombstone: false,
    };
    let anchor = CommitCheckpoint {
        binding: replica.binding,
        sequence: CommitSeq::GENESIS,
        digest: [0; 32],
    };
    let applied = replica
        .db
        .apply_remote_batch(
            replica.binding,
            replica.peer,
            anchor,
            std::slice::from_ref(&commit),
        )
        .unwrap();
    assert_eq!(applied.applied, 1);
    assert_eq!(replica.counts(), (1, 0, 1));

    let replay = replica
        .plane
        .durability()
        .unwrap()
        .recover(replica.root)
        .unwrap();
    assert_eq!(replay.commits.len(), 1);
    assert_eq!(
        count(&replica.db, "library_projection_watermarks"),
        1,
        "the applied watermark advances with the visible rows"
    );
    assert_eq!(
        count(&replica.db, "library_applied_operations"),
        1,
        "the applied operation is recorded in the same transaction"
    );

    // The same page replayed is a no-op that neither duplicates the row nor
    // opens a second directory operation.
    assert_eq!(
        replica
            .db
            .apply_remote_batch(replica.binding, replica.peer, anchor, &[commit])
            .unwrap()
            .applied,
        0
    );
    assert_eq!(replica.counts(), (1, 0, 1));
    assert_eq!(replica.directory_commits(), 1);
    // A fresh projection rebuilt from the directory alone reproduces the row,
    // so the incremental apply is recoverable without the old database.
    let (_, rebuilt) = rebuild(&replica, &replay);
    assert_eq!(rebuilt.0, 1);
}

/// A snapshot is published as one directory operation; a fresh projection built
/// from that operation alone must reproduce exactly the visible set.
#[test]
fn snapshot_publishes_the_visible_set_only_through_the_directory() {
    let replica = replica();
    let song_id = SongId::new();
    let favorite = SongId::new();
    let playlist = PlaylistId::new();
    let member = PlaylistItemId::new();

    let mut first = replica.page();
    first.next_page_index = Some(1);
    first.records = vec![song(song_id)];
    let staged = replica
        .db
        .apply_snapshot_page(replica.peer, &first)
        .unwrap();
    assert_eq!(staged.checkpoint, None);
    // A non-final page is private staging: no business row, no applied cursor,
    // and no directory operation, because nothing is visible yet.
    assert_eq!(replica.counts(), (0, 0, 0));
    assert_eq!(replica.directory_commits(), 0);
    assert_eq!(count(&replica.db, "sync_replica_snapshot_pages"), 1);

    let last = ReplicaSnapshotPage {
        page_index: 1,
        next_page_index: None,
        records: vec![
            song(favorite),
            object(
                ObjectKey::Favorite(favorite),
                serde_json::json!({
                    "song_uuid":favorite,
                    "is_favorite":true,
                    "favorited_at":50,
                }),
            ),
            object(
                ObjectKey::Playlist(playlist),
                serde_json::json!({
                    "playlist_uuid":playlist,
                    "display_name":"Empty snapshot list",
                    "created_at":12,
                    "members":[{
                        "song_uuid":song_id,
                        "member_uuid":member,
                        "position":0,
                        "added_at":13,
                    }],
                }),
            ),
        ],
        ..first
    };
    let applied = replica.db.apply_snapshot_page(replica.peer, &last).unwrap();
    assert_eq!(applied.checkpoint, Some(last.checkpoint));
    assert_eq!(replica.counts(), (2, 1, 1));

    // The visible set came from a single directory operation that carries the
    // same typed records, so a projection rebuilt from the directory alone is
    // identical rather than a merge with whatever SQL happened to hold.
    let replay = replica
        .plane
        .durability()
        .unwrap()
        .recover(replica.root)
        .unwrap();
    assert_eq!(replay.commits.len(), 1);
    let batch = &replay.commits[0].batch;
    assert!(batch.state.iter().any(|state| {
        state.kind == StateKind::Cursor
            && state.payload["schema"] == serde_json::json!("echo.sync.peer-cursor.v1")
            && state.payload["sequence"] == serde_json::json!(12)
    }));

    let (fresh, rebuilt) = rebuild(&replica, &replay);
    assert_eq!(rebuilt, (2, 1, 1, 0));
    // The peer cursor is retained as typed evidence rather than dropped: its
    // authenticated dependencies are a separate recovery concern (7.4), so a
    // rebuild records it as unresolved instead of silently losing it.
    assert!(fresh
        .read_recovery_records(replica.root)
        .unwrap()
        .state
        .iter()
        .any(|state| state.kind == StateKind::Cursor));
}

/// Both publication windows are recoverable: a fault before the commit marker
/// leaves nothing saved, and a fault after it leaves the operation saved with
/// the visible set absent until the same snapshot page is retried.
#[test]
fn snapshot_commit_faults_recover_with_one_operation_and_no_partial_set() {
    for phase in ["prepared", "committed"] {
        let replica = replica();
        let playlist = PlaylistId::new();
        let mut page = replica.page();
        page.records = vec![object(
            ObjectKey::Playlist(playlist),
            serde_json::json!({
                "playlist_uuid":playlist,
                "display_name":format!("faulted at {phase}"),
                "created_at":12,
                "members":[],
            }),
        )];
        let faulted = Arc::new(replica.plane.as_ref().clone().with_commit_hook(
            move |observed, _| {
                if observed == phase {
                    Err(Error::unavailable(
                        "snapshot directory",
                        "injected publication fault",
                    ))
                } else {
                    Ok(())
                }
            },
        ));
        replica.db.configure_library_control(faulted);
        let failure = replica
            .db
            .apply_snapshot_page(replica.peer, &page)
            .expect_err("a faulted directory commit cannot publish a visible set");
        if phase == "committed" {
            assert_eq!(
                failure.code(),
                "library_saved_pending_recovery",
                "{failure}"
            );
        }
        assert_eq!(
            replica.counts(),
            (0, 0, 0),
            "{phase}: no visible row or applied cursor may precede the directory"
        );

        // The staging page is still there, so the identical retry is the same
        // operation rather than a second one.
        replica.db.configure_library_control(replica.plane.clone());
        replica.db.apply_snapshot_page(replica.peer, &page).unwrap();
        assert_eq!(replica.counts(), (0, 1, 1), "{phase}");
        assert_eq!(
            replica.directory_commits(),
            1,
            "{phase}: the retry must reuse the saved operation"
        );
        assert_eq!(
            replica
                .db
                .with_reader(|c| {
                    c.query_row(
                        "SELECT COUNT(*) FROM library_projection_watermarks",
                        [],
                        |row| row.get::<_, i64>(0),
                    )
                    .map_err(storage)
                })
                .unwrap(),
            1,
            "{phase}: the watermark is projected with the visible set"
        );
    }
}

/// Cancelling a half-received snapshot removes only the private staging; no
/// directory operation and no visible row may survive it.
#[test]
fn discarded_snapshot_leaves_no_directory_operation_and_allows_a_fresh_one() {
    let replica = replica();
    let mut first = replica.page();
    first.next_page_index = Some(1);
    first.records = vec![song(SongId::new())];
    replica
        .db
        .apply_snapshot_page(replica.peer, &first)
        .unwrap();
    replica
        .db
        .discard_snapshot(replica.peer, &first.snapshot_token)
        .unwrap();
    assert_eq!(count(&replica.db, "sync_replica_snapshot_sessions"), 0);
    assert_eq!(count(&replica.db, "sync_replica_snapshot_pages"), 0);
    assert_eq!(replica.counts(), (0, 0, 0));
    assert_eq!(replica.directory_commits(), 0);

    let fresh = replica.page();
    replica
        .db
        .apply_snapshot_page(replica.peer, &fresh)
        .unwrap();
    assert_eq!(replica.counts(), (0, 0, 1));
    assert_eq!(replica.directory_commits(), 1);
}

/// A metadata-only favorite and an empty playlist are user values that carry no
/// resources; they must reach the directory as typed records, not be dropped for
/// lacking a resource reference.
#[test]
fn snapshot_keeps_metadata_only_favorite_and_empty_playlist_through_the_directory() {
    let replica = replica();
    let song_id = SongId::new();
    let playlist = PlaylistId::new();
    let alias = SongId::new();
    let mut page = replica.page();
    page.records = vec![
        song(song_id),
        // A favorite is a metadata-only user value: it carries no resource
        // reference of its own and must still reach the directory.
        object(
            ObjectKey::Favorite(song_id),
            serde_json::json!({
                "song_uuid":song_id,
                "is_favorite":true,
                "favorited_at":50,
            }),
        ),
        object(
            ObjectKey::Playlist(playlist),
            serde_json::json!({
                "playlist_uuid":playlist,
                "display_name":"No members yet",
                "created_at":12,
                "members":[],
            }),
        ),
        HostSnapshotRecord::SongAlias {
            alias,
            canonical: song_id,
        },
        HostSnapshotRecord::PlaylistMemberAlias(PlaylistMemberAlias {
            playlist_id: playlist,
            alias_member_id: PlaylistItemId::new(),
            canonical_member_id: PlaylistItemId::new(),
        }),
    ];
    replica.db.apply_snapshot_page(replica.peer, &page).unwrap();
    assert_eq!(replica.counts(), (1, 1, 1));
    assert_eq!(count(&replica.db, "playlist_songs"), 0);

    let replay = replica
        .plane
        .durability()
        .unwrap()
        .recover(replica.root)
        .unwrap();
    let batch = &replay.commits[0].batch;
    // The empty playlist is a real record, so a rebuild cannot lose it.
    assert!(batch.records.iter().any(|envelope| matches!(
        envelope.record(),
        PortableRecord::Playlist(record)
            if record.playlist_uuid == playlist.as_uuid() && record.display_name == "No members yet"
    )));
    assert!(batch.records.iter().any(|envelope| matches!(
        envelope.record(),
        PortableRecord::Favorite(record) if record.song_uuid == song_id.as_uuid()
    )));
    // Aliases are retained as typed state, not inferred from the visible value.
    assert!(batch.state.iter().any(|state| {
        state.kind == StateKind::Alias
            && state.payload["alias"] == serde_json::json!(alias.to_string())
    }));
}
