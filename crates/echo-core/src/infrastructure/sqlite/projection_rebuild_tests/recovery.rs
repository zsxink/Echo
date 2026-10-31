use super::*;

#[test]
fn library_durability_rebuild_rejects_a_projection_that_changed_protocol_history() {
    use crate::domain::sync::{AuthorityId, Epoch, SyncBinding};
    use rusqlite::Connection;

    let directory = tempfile::tempdir().unwrap();
    let database_path = directory.path().join("active.sqlite");
    let database = SqliteDatabase::open(&database_path).unwrap();
    let root = LibraryRootId::new();
    LibraryRepository::upsert(
        &database,
        &LibraryRoot::new(root, directory.path().join("library"), true, true),
    )
    .unwrap();
    let old_library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let old = replay(
        old_library,
        Issue72Fixture::with_database_evidence().database,
    );
    database.project_library_commits(root, &old).unwrap();
    let binding = SyncBinding {
        library_id: crate::domain::sync::LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    database.bind_sync_library(binding, true, root).unwrap();
    let connection = Connection::open(&database_path).unwrap();
    connection
        .execute(
            "INSERT INTO sync_commits(commit_seq,library_id,authority_id,epoch,origin_device_id,mutation_id,canonical_json,previous_digest,commit_digest,publication_state,created_at) VALUES(1,?1,?2,?3,?4,?5,'{}',?6,?7,'published',1)",
            rusqlite::params![
                binding.library_id.as_uuid().to_string(),
                binding.authority_id.as_uuid().to_string(),
                i64::try_from(binding.epoch.as_u64()).unwrap(),
                uuid::Uuid::new_v4().to_string(),
                uuid::Uuid::new_v4().to_string(),
                vec![0u8; 32],
                vec![3u8; 32],
            ],
        )
        .unwrap();
    // A rebuild that reaches into protocol history must be rejected, not
    // committed: the ledger row is evidence no directory replay can restore.
    connection
        .execute_batch(&format!(
            "CREATE TRIGGER drop_protocol_history AFTER INSERT ON songs WHEN NEW.library_root_uuid='{}' BEGIN DELETE FROM sync_commits; END;",
            root
        ))
        .unwrap();
    drop(connection);
    let before = ProtocolContext::read(&Connection::open(&database_path).unwrap()).unwrap();
    let old_status = database.projection_status(root).unwrap();

    let new_library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let replacement = replay(new_library, Issue72Fixture::orphaned().portable);
    let failure = match database.rebuild_library_projection(root, new_library, &replacement) {
        Ok(_) => panic!("rebuild must reject changed protocol history"),
        Err(error) => error.to_string(),
    };
    assert!(failure.contains("protocol history"), "{failure}");
    let connection = Connection::open(&database_path).unwrap();
    assert_eq!(ProtocolContext::read(&connection).unwrap(), before);
    drop(connection);
    assert_eq!(database.projection_status(root).unwrap(), old_status);
    assert_eq!(
        SongRepository::all_in_root(&database, root).unwrap().len(),
        15
    );
}

#[test]
fn library_durability_rebuild_preserves_sync_context_protocol_history() {
    use crate::domain::sync::{AuthorityId, DeviceId, Epoch, SyncBinding};
    use rusqlite::Connection;

    let directory = tempfile::tempdir().unwrap();
    let database_path = directory.path().join("active.sqlite");
    let database = SqliteDatabase::open(&database_path).unwrap();
    let root = LibraryRootId::new();
    let other_root = LibraryRootId::new();
    for (id, active) in [(root, true), (other_root, false)] {
        LibraryRepository::upsert(
            &database,
            &LibraryRoot::new(
                id,
                directory.path().join(format!("library-{id}")),
                active,
                true,
            ),
        )
        .unwrap();
    }
    let old_library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let old = replay(
        old_library,
        Issue72Fixture::with_database_evidence().database,
    );
    database.project_library_commits(root, &old).unwrap();

    // Two roots own sync contexts; `other_root` is archived because the active
    // context belongs to `root`. Both carry protocol evidence a rebuild must
    // not touch: a commit ledger row, a confirmed object version, and an inbox
    // receipt, which no directory replay may re-derive (LD-07 keeps unconfirmed
    // intent and conflicts across a rebuild).
    let root_binding = SyncBinding {
        library_id: crate::domain::sync::LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    let other_binding = SyncBinding {
        library_id: crate::domain::sync::LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::INITIAL,
    };
    database
        .bind_sync_library(root_binding, true, root)
        .unwrap();
    database
        .bind_sync_library(other_binding, true, other_root)
        .unwrap();
    let connection = Connection::open(&database_path).unwrap();
    for (prefix, binding) in [("", root_binding), ("archived_", other_binding)] {
        let context = if prefix.is_empty() { "root" } else { "other" };
        connection
            .execute(
                &format!(
                    "INSERT INTO {prefix}sync_commits(commit_seq,library_id,authority_id,epoch,origin_device_id,mutation_id,canonical_json,previous_digest,commit_digest,publication_state,created_at) VALUES(1,?1,?2,?3,?4,?5,?6,?7,?8,'published',1)"
                ),
                rusqlite::params![
                    binding.library_id.as_uuid().to_string(),
                    binding.authority_id.as_uuid().to_string(),
                    i64::try_from(binding.epoch.as_u64()).unwrap(),
                    DeviceId::new().as_uuid().to_string(),
                    uuid::Uuid::new_v4().to_string(),
                    format!("{{\"probe\":\"{context}\"}}"),
                    vec![0u8; 32],
                    // Distinct digests: `commit_digest` is UNIQUE.
                    if prefix.is_empty() {
                        vec![1u8; 32]
                    } else {
                        vec![2u8; 32]
                    },
                ],
            )
            .unwrap();
    }
    drop(connection);

    let protocol_before =
        ProtocolContext::read(&Connection::open(&database_path).unwrap()).unwrap();
    assert!(protocol_before
        .live
        .iter()
        .any(|(table, count)| *table == "sync_commits" && *count == 1));
    assert!(protocol_before
        .archived
        .iter()
        .any(|(table, count)| *table == "sync_commits" && *count == 1));

    let new_library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let replacement = replay(new_library, Issue72Fixture::orphaned().portable);
    let report = database
        .rebuild_library_projection(root, new_library, &replacement)
        .unwrap();
    assert_eq!(database.projection_status(root).unwrap(), report.projection);
    assert_eq!(
        ProtocolContext::read(&Connection::open(&database_path).unwrap()).unwrap(),
        protocol_before
    );
    assert_eq!(
        SongRepository::all_in_root(&database, other_root)
            .unwrap()
            .len(),
        0
    );
}

#[test]
fn library_durability_rebuild_isolates_a_projection_belonging_to_another_library() {
    let directory = tempfile::tempdir().unwrap();
    let database = SqliteDatabase::open(directory.path().join("active.sqlite")).unwrap();
    let root = LibraryRootId::new();
    LibraryRepository::upsert(
        &database,
        &LibraryRoot::new(root, directory.path().join("library"), true, true),
    )
    .unwrap();
    let old_library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    database
        .project_library_commits(
            root,
            &replay(
                old_library,
                Issue72Fixture::with_database_evidence().database,
            ),
        )
        .unwrap();
    let old_ids = song_ids(&database, root);
    assert!(!old_ids.is_empty());

    // The verified directory now names a different library: the retained view
    // cannot be merged with it, so adoption replaces the whole root.
    let new_library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let replacement = replay(new_library, Issue72Fixture::orphaned().portable);
    let incremental = database
        .project_library_commits(root, &replacement)
        .expect_err("a foreign-library projection is not merged in place");
    // The existing watermark already covers this sequence with a different
    // applied operation, so the chain cannot be continued in place.
    assert!(
        incremental
            .to_string()
            .contains("differs from library chain")
            || incremental.to_string().contains("another library"),
        "{incremental}"
    );
    assert_eq!(song_ids(&database, root), old_ids);

    let report = database
        .rebuild_library_projection(root, new_library, &replacement)
        .unwrap();
    assert_eq!(database.projection_status(root).unwrap(), report.projection);
    let adopted: std::collections::BTreeSet<_> = song_ids(&database, root).into_iter().collect();
    let expected: std::collections::BTreeSet<_> = replacement.commits[0]
        .batch
        .records
        .iter()
        .filter_map(|envelope| match envelope.record() {
            PortableRecord::Song(song) => Some(song.song_uuid),
            _ => None,
        })
        .collect();
    assert_eq!(adopted, expected);
}

#[test]
fn library_durability_rebuild_rejects_a_broken_directory_chain() {
    let directory = tempfile::tempdir().unwrap();
    let database = SqliteDatabase::open(directory.path().join("active.sqlite")).unwrap();
    let root = LibraryRootId::new();
    LibraryRepository::upsert(
        &database,
        &LibraryRoot::new(root, directory.path().join("library"), true, true),
    )
    .unwrap();
    let old_library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    database
        .project_library_commits(
            root,
            &replay(
                old_library,
                Issue72Fixture::with_database_evidence().database,
            ),
        )
        .unwrap();
    let before_ids = song_ids(&database, root);
    let before_status = database.projection_status(root).unwrap();

    // Sequence 2 without sequence 1 is a gap, not a chain: the candidate can
    // never be verified against a head the directory cannot prove.
    let new_library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let mut broken = chain(
        new_library,
        vec![
            Issue72Fixture::orphaned().portable,
            Issue72Fixture::orphaned().portable,
        ],
    );
    broken.commits.remove(0);
    let failure = database
        .rebuild_library_projection(root, new_library, &broken)
        .expect_err("a gapped chain must not be adopted");
    assert!(
        failure.to_string().contains("incomplete"),
        "unexpected failure: {failure}"
    );
    assert_eq!(song_ids(&database, root), before_ids);
    assert_eq!(database.projection_status(root).unwrap(), before_status);
    assert!(no_candidate_directories(directory.path()));
}

#[test]
fn library_durability_projection_ahead_of_directory_rebuilds_from_the_manifest_library() {
    use crate::domain::library::LibraryManifest;
    use crate::infrastructure::filesystem::{RootControlPlane, RootRegistry};
    use rusqlite::Connection;

    let directory = tempfile::tempdir().unwrap();
    let library_path = directory.path().join("library");
    std::fs::create_dir_all(&library_path).unwrap();
    let root = LibraryRootId::new();
    let library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let registry = RootRegistry::new();
    registry.register(root, &library_path);
    let control = RootControlPlane::new(registry);
    control
        .write_manifest(root, &LibraryManifest::new(library, "test"))
        .unwrap();
    let records = Issue72Fixture::orphaned().portable;
    let batch = LibraryBatch::new(
        uuid::Uuid::new_v4(),
        library,
        ChangeOrigin::Recovery,
        records
            .into_iter()
            .map(|record| PortableRecordEnvelope::new(record, None).unwrap())
            .collect(),
    );
    control.durability().unwrap().commit(root, &batch).unwrap();
    let replay = control.durability().unwrap().recover(root).unwrap();

    let database_path = directory.path().join("active.sqlite");
    let database = SqliteDatabase::open(&database_path).unwrap();
    LibraryRepository::upsert(&database, &LibraryRoot::new(root, library_path, true, true))
        .unwrap();
    database.configure_library_control(std::sync::Arc::new(control));
    database.project_library_commits(root, &replay).unwrap();
    Connection::open(&database_path)
        .unwrap()
        .execute(
            "UPDATE library_projection_watermarks SET commit_seq=99 WHERE library_root_uuid=?1",
            [root.to_string()],
        )
        .unwrap();

    database.project_library_commits(root, &replay).unwrap();
    assert_eq!(
        database
            .projection_status(root)
            .unwrap()
            .applied_library_commit_seq,
        1
    );
    assert!(!SongRepository::all_in_root(&database, root)
        .unwrap()
        .is_empty());
}

#[test]
fn library_durability_rebuild_preserves_unjournaled_legacy_snapshots() {
    use crate::application::ports::PlaylistRepository;
    use crate::domain::ids::{PlaylistId, SongId};
    use crate::domain::library::LibraryManifest;
    use crate::infrastructure::filesystem::{RootControlPlane, RootRegistry};

    let directory = tempfile::tempdir().unwrap();
    let library_path = directory.path().join("library");
    std::fs::create_dir_all(&library_path).unwrap();
    let root = LibraryRootId::new();
    let library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let registry = RootRegistry::new();
    registry.register(root, &library_path);
    let control = RootControlPlane::new(registry);
    control
        .write_manifest(root, &LibraryManifest::new(library, "test"))
        .unwrap();
    let records = Issue72Fixture::with_database_evidence().database;
    let song = records
        .iter()
        .find(|r| matches!(r, PortableRecord::Song(_)))
        .unwrap()
        .clone();
    let playlist = records
        .iter()
        .find(|r| matches!(r, PortableRecord::Playlist(_)))
        .unwrap()
        .clone();
    let favorite = records
        .iter()
        .find(|r| matches!(r, PortableRecord::Favorite(f) if f.song_uuid == song.object_uuid()))
        .unwrap()
        .clone();
    control.write_record(root, &song).unwrap();
    control.write_record(root, &playlist).unwrap();
    let batch = LibraryBatch::new(
        uuid::Uuid::new_v4(),
        library,
        ChangeOrigin::LocalEdit,
        vec![PortableRecordEnvelope::new(favorite, None).unwrap()],
    );
    control.durability().unwrap().commit(root, &batch).unwrap();
    let replay = control.durability().unwrap().recover(root).unwrap();
    let database_path = directory.path().join("active.sqlite");
    let database = SqliteDatabase::open(&database_path).unwrap();
    LibraryRepository::upsert(&database, &LibraryRoot::new(root, library_path, true, true))
        .unwrap();
    database.configure_library_control(std::sync::Arc::new(control));
    database
        .project_legacy_records(
            root,
            &[
                PortableRecordEnvelope::new(song.clone(), None).unwrap(),
                PortableRecordEnvelope::new(playlist.clone(), None).unwrap(),
            ],
        )
        .unwrap();
    database.project_library_commits(root, &replay).unwrap();
    rusqlite::Connection::open(&database_path)
        .unwrap()
        .execute(
            "UPDATE library_projection_watermarks SET commit_seq=99 WHERE library_root_uuid=?1",
            [root.to_string()],
        )
        .unwrap();
    database.project_library_commits(root, &replay).unwrap();
    assert!(
        SongRepository::by_id(&database, SongId::from_uuid(song.object_uuid()))
            .unwrap()
            .unwrap()
            .favorite()
    );
    assert_eq!(
        PlaylistRepository::name(&database, PlaylistId::from_uuid(playlist.object_uuid()))
            .unwrap()
            .as_deref(),
        Some("合成历史歌单")
    );
}

#[test]
fn library_durability_rebuild_adopts_a_projection_behind_the_directory_head() {
    let directory = tempfile::tempdir().unwrap();
    let database = SqliteDatabase::open(directory.path().join("active.sqlite")).unwrap();
    let root = LibraryRootId::new();
    LibraryRepository::upsert(
        &database,
        &LibraryRoot::new(root, directory.path().join("library"), true, true),
    )
    .unwrap();
    let library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let full = chain(
        library,
        vec![
            Issue72Fixture::orphaned().portable,
            Issue72Fixture::orphaned().portable,
        ],
    );
    // The database only knows the first link: it is behind the verified head.
    let behind = LibraryReplay {
        checkpoint: None,
        commits: vec![full.commits[0].clone()],
    };
    database.project_library_commits(root, &behind).unwrap();
    assert_eq!(
        database
            .projection_status(root)
            .unwrap()
            .applied_library_commit_seq,
        1
    );

    let report = database
        .rebuild_library_projection(root, library, &full)
        .unwrap();
    let status = database.projection_status(root).unwrap();
    // The whole chain is projected, not only the newest link, and the adopted
    // head is the directory head the candidate was verified against.
    assert_eq!(status.applied_library_commit_seq, 2);
    assert_eq!(status, report.projection);
    assert_eq!(
        status.applied_head_digest.as_deref(),
        full.commits.last().map(|commit| commit.digest.as_str())
    );
    assert!(!song_ids(&database, root).is_empty());
    assert!(no_candidate_directories(directory.path()));
}

#[test]
fn library_durability_rebuild_rejects_a_candidate_that_changed_after_projection() {
    let directory = tempfile::tempdir().unwrap();
    let database = SqliteDatabase::open(directory.path().join("active.sqlite")).unwrap();
    let root = LibraryRootId::new();
    LibraryRepository::upsert(
        &database,
        &LibraryRoot::new(root, directory.path().join("library"), true, true),
    )
    .unwrap();
    let old_library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    database
        .project_library_commits(
            root,
            &replay(
                old_library,
                Issue72Fixture::with_database_evidence().database,
            ),
        )
        .unwrap();
    let before_ids = song_ids(&database, root);
    let before_status = database.projection_status(root).unwrap();
    assert!(!before_ids.is_empty());

    let new_library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let replacement = replay(new_library, Issue72Fixture::orphaned().portable);
    inject_candidate_corruption(root, |candidate| {
        candidate
            .execute(
                "UPDATE library_recovery_records SET envelope_json='{\"record\":{\"unknown\":1}}' WHERE row_kind='record'",
                [],
            )
            .map_err(storage)?;
        Ok(())
    });
    assert!(database
        .rebuild_library_projection(root, new_library, &replacement)
        .is_err());
    // A candidate whose bytes no longer match the directory never replaces the
    // active projection, and no private candidate directory is left behind.
    assert_eq!(song_ids(&database, root), before_ids);
    assert_eq!(database.projection_status(root).unwrap(), before_status);
    assert!(no_candidate_directories(directory.path()));
}

#[test]
fn library_durability_rebuild_clears_search_rows_and_artist_covers_of_the_replaced_root() {
    use rusqlite::Connection;

    let directory = tempfile::tempdir().unwrap();
    let database_path = directory.path().join("active.sqlite");
    let database = SqliteDatabase::open(&database_path).unwrap();
    let root = LibraryRootId::new();
    let other_root = LibraryRootId::new();
    for (id, active) in [(root, true), (other_root, false)] {
        LibraryRepository::upsert(
            &database,
            &LibraryRoot::new(
                id,
                directory.path().join(format!("library-{id}")),
                active,
                true,
            ),
        )
        .unwrap();
    }
    let old_library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    database
        .project_library_commits(
            root,
            &replay(
                old_library,
                Issue72Fixture::with_database_evidence().database,
            ),
        )
        .unwrap();
    let old_ids: std::collections::BTreeSet<String> = song_ids(&database, root)
        .into_iter()
        .map(|id| id.to_string())
        .collect();
    let connection = Connection::open(&database_path).unwrap();
    let indexed = |connection: &Connection, ids: &std::collections::BTreeSet<String>| -> u64 {
        let mut total = 0;
        for id in ids {
            total += connection
                .query_row(
                    "SELECT COUNT(*) FROM song_search WHERE song_uuid=?1",
                    [id],
                    |row| row.get::<_, u64>(0),
                )
                .unwrap();
        }
        total
    };
    assert_eq!(indexed(&connection, &old_ids), old_ids.len() as u64);
    for (root_id, artist) in [(&root, "stale-artist"), (&other_root, "other-artist")] {
        connection
            .execute(
                "INSERT INTO artist_covers(library_root_uuid,artist_key,cover_asset_key,updated_at) VALUES(?1,?2,'cover',0)",
                rusqlite::params![root_id.to_string(), artist],
            )
            .unwrap();
    }
    drop(connection);

    let new_library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let replacement = replay(new_library, Issue72Fixture::orphaned().portable);
    database
        .rebuild_library_projection(root, new_library, &replacement)
        .unwrap();
    let new_ids: std::collections::BTreeSet<String> = song_ids(&database, root)
        .into_iter()
        .map(|id| id.to_string())
        .collect();
    // The fixture reuses deterministic song identities, so only the songs the
    // rebuilt replay dropped are the ones that must stop being searchable.
    let removed: std::collections::BTreeSet<String> =
        old_ids.difference(&new_ids).cloned().collect();
    assert!(!removed.is_empty());
    let connection = Connection::open(&database_path).unwrap();
    // The fts5 mirror has no trigger and no foreign key: without an explicit
    // root-scoped delete the replaced root would answer searches with songs it
    // no longer contains.
    assert_eq!(indexed(&connection, &removed), 0);
    assert_eq!(indexed(&connection, &new_ids), new_ids.len() as u64);
    let covers = |connection: &Connection, root_id: &LibraryRootId| -> u64 {
        connection
            .query_row(
                "SELECT COUNT(*) FROM artist_covers WHERE library_root_uuid=?1",
                [root_id.to_string()],
                |row| row.get(0),
            )
            .unwrap()
    };
    // A rebuilt replay without that cover must not keep the old assignment;
    // other roots keep theirs.
    assert_eq!(covers(&connection, &root), 0);
    assert_eq!(covers(&connection, &other_root), 1);
}
