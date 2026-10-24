use super::*;
use crate::application::ports::{
    LibraryProjectionPort, LibraryRepository, PlaylistRepository, SongRepository,
};
use crate::application::testkit::library_durability::{CanonicalLibraryState, Issue72Fixture};
use crate::domain::entities::LibraryRoot;
use crate::domain::ids::{PlaylistId, SongId};
use crate::domain::library::durability::{ChangeOrigin, LibraryBatch, LibraryCommit};
use crate::domain::library::{LibraryId, PortableRecord, PortableRecordEnvelope};
use crate::infrastructure::sqlite::SqliteDatabase;

fn append(replay: &mut LibraryReplay, library: LibraryId, records: Vec<PortableRecord>) {
    let seq = replay.commits.len() as u64 + 1;
    let predecessor_digest = replay.commits.last().map(|commit| commit.digest.clone());
    let batch = LibraryBatch::new(
        uuid::Uuid::new_v4(),
        library,
        ChangeOrigin::Recovery,
        records
            .into_iter()
            .map(|record| PortableRecordEnvelope::new(record, None).unwrap())
            .collect(),
    );
    let digest =
        LibraryCommit::calculate_digest(&batch, seq, predecessor_digest.as_deref()).unwrap();
    replay.commits.push(LibraryCommit {
        batch,
        library_commit_seq: seq,
        predecessor_digest,
        digest,
    });
}

fn database() -> (tempfile::TempDir, SqliteDatabase, LibraryRootId) {
    let directory = tempfile::tempdir().unwrap();
    let db = SqliteDatabase::open(directory.path().join("projection.sqlite")).unwrap();
    let root = LibraryRootId::new();
    LibraryRepository::upsert(
        &db,
        &LibraryRoot::new(root, directory.path().join("library"), true, true),
    )
    .unwrap();
    (directory, db, root)
}

#[test]
fn library_durability_projection_retains_orphans_and_links_when_evidence_arrives_without_echo() {
    let (_directory, db, root) = database();
    let library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let fixture = Issue72Fixture::with_database_evidence();
    let records = fixture.database;
    let expected_records: Vec<_> = records
        .iter()
        .map(|record| PortableRecordEnvelope::new(record.clone(), None).unwrap())
        .collect();
    let songs: Vec<_> = records
        .iter()
        .filter(|record| matches!(record, PortableRecord::Song(_)))
        .cloned()
        .collect();
    let relations: Vec<_> = records
        .iter()
        .filter(|record| !matches!(record, PortableRecord::Song(_)))
        .cloned()
        .collect();
    let mut replay = LibraryReplay::default();
    append(&mut replay, library, relations);
    db.project_library_commits(root, &replay).unwrap();
    assert!(!db.projection_status(root).unwrap().unresolved.is_empty());
    assert!(db.all_in_root(root).unwrap().is_empty());
    append(&mut replay, library, songs);
    db.project_library_commits(root, &replay).unwrap();
    assert!(db.projection_status(root).unwrap().unresolved.is_empty());
    assert_eq!(db.all_in_root(root).unwrap().len(), 15);
    let song = SongRepository::by_id(&db, SongId::from_uuid(uuid::Uuid::from_u128(100)))
        .unwrap()
        .unwrap();
    assert!(song.favorite());
    assert_eq!(song.added_at(), 1_700_000_000_000);
    let members =
        PlaylistRepository::members(&db, PlaylistId::from_uuid(uuid::Uuid::from_u128(500)))
            .unwrap();
    assert_eq!(
        members
            .iter()
            .map(|member| (member.id().as_uuid(), member.position()))
            .collect::<Vec<_>>(),
        vec![
            (uuid::Uuid::from_u128(700), 0),
            (uuid::Uuid::from_u128(701), 10),
            (uuid::Uuid::from_u128(702), 20)
        ]
    );
    let before = db.read_recovery_records(root).unwrap();
    CanonicalLibraryState::from_parts(&expected_records, &[], &[]).assert_matches(
        &CanonicalLibraryState::from_parts(&before.records, &before.state, &before.resources),
    );
    db.project_library_commits(root, &replay).unwrap();
    assert_eq!(db.read_recovery_records(root).unwrap(), before);
    db.with_reader(|connection| {
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM sync_mutations", [], |row| row.get(0))
            .map_err(storage)?;
        assert_eq!(count, 0, "projection never makes upload candidates");
        Ok(())
    })
    .unwrap();
}

#[test]
fn library_durability_projection_retries_only_new_or_unresolved_records() {
    let (_directory, db, root) = database();
    let library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let fixture = Issue72Fixture::with_database_evidence();
    let song = fixture
        .database
        .iter()
        .find(|record| matches!(record, PortableRecord::Song(_)))
        .unwrap()
        .clone();
    let id = song.object_uuid().to_string();
    let mut replay = LibraryReplay::default();
    append(&mut replay, library, vec![song.clone()]);
    db.project_library_commits(root, &replay).unwrap();
    db.writer.run(move |c| {
        c.execute("UPDATE songs SET title='newer live value',blake3_hash='newer live hash',revision=revision+1 WHERE uuid=?1", [id]).map_err(storage)?;
        c.execute_batch("CREATE TRIGGER reject_redundant_song_projection BEFORE UPDATE ON songs BEGIN SELECT RAISE(ABORT,'resolved record was projected again'); END;")
            .map_err(storage)
    }).unwrap();
    db.project_library_commits(root, &replay).unwrap();
    append(&mut replay, library, vec![]);
    db.project_library_commits(root, &replay).unwrap();
    let projected = SongRepository::by_id(&db, SongId::from_uuid(song.object_uuid()))
        .unwrap()
        .unwrap();
    assert_eq!(projected.title(), Some("newer live value"));
    assert_eq!(projected.blake3_hash(), Some("newer live hash"));
    db.writer
        .run(|c| {
            c.execute_batch("DROP TRIGGER reject_redundant_song_projection")
                .map_err(storage)
        })
        .unwrap();
    // A new directory commit is authoritative even if its object revision is
    // lower than the live row; SQL revisions cannot replace directory order.
    let mut next = song;
    if let PortableRecord::Song(record) = &mut next {
        record.title = Some("next directory value".into());
    }
    append(&mut replay, library, vec![next.clone()]);
    db.project_library_commits(root, &replay).unwrap();
    assert_eq!(
        SongRepository::by_id(&db, SongId::from_uuid(next.object_uuid()))
            .unwrap()
            .unwrap()
            .title(),
        Some("next directory value")
    );
}

#[test]
fn library_durability_projection_rejects_bad_suffix_before_any_database_write() {
    let (_directory, db, root) = database();
    let library = LibraryId::from_uuid(uuid::Uuid::new_v4());
    let fixture = Issue72Fixture::with_database_evidence();
    let mut replay = LibraryReplay::default();
    append(&mut replay, library, vec![fixture.database[0].clone()]);
    append(&mut replay, library, vec![]);
    let bad = &mut replay.commits[1];
    bad.predecessor_digest = None;
    bad.digest = LibraryCommit::calculate_digest(&bad.batch, 2, None).unwrap();
    assert!(db.project_library_commits(root, &replay).is_err());
    assert!(db.all_in_root(root).unwrap().is_empty());
    assert_eq!(
        db.projection_status(root)
            .unwrap()
            .applied_library_commit_seq,
        0
    );
    assert!(db.read_recovery_records(root).unwrap().records.is_empty());
}

mod identity;
mod retained_evidence;
