use super::*;
use crate::application::ports::{
    ControlPlanePort, LibraryProjectionPort, LibraryRepository, SongRepository,
};
use crate::application::testkit::library_durability::Issue72Fixture;
use crate::domain::entities::LibraryRoot;
use crate::domain::library::durability::{ChangeOrigin, LibraryBatch, LibraryCommit};
use crate::infrastructure::sqlite::SqliteDatabase;

fn replay(library: LibraryId, records: Vec<PortableRecord>) -> LibraryReplay {
    chain(library, vec![records])
}

/// An ordered chain of real commits: each batch binds its predecessor digest,
/// so the replay can only be projected by replaying the whole sequence.
fn chain(library: LibraryId, batches: Vec<Vec<PortableRecord>>) -> LibraryReplay {
    let mut commits = Vec::new();
    let mut predecessor: Option<String> = None;
    for (index, records) in batches.into_iter().enumerate() {
        let batch = LibraryBatch::new(
            uuid::Uuid::new_v4(),
            library,
            ChangeOrigin::Recovery,
            records
                .into_iter()
                .map(|record| PortableRecordEnvelope::new(record, None).unwrap())
                .collect(),
        );
        let seq = u64::try_from(index).unwrap() + 1;
        let digest = LibraryCommit::calculate_digest(&batch, seq, predecessor.as_deref()).unwrap();
        commits.push(LibraryCommit {
            batch,
            library_commit_seq: seq,
            predecessor_digest: predecessor,
            digest,
        });
        predecessor = commits.last().map(|commit| commit.digest.clone());
    }
    LibraryReplay {
        checkpoint: None,
        commits,
    }
}

fn song_ids(database: &SqliteDatabase, root: LibraryRootId) -> Vec<uuid::Uuid> {
    SongRepository::all_in_root(database, root)
        .unwrap()
        .into_iter()
        .map(|song| song.id().as_uuid())
        .collect()
}

fn no_candidate_directories(parent: &std::path::Path) -> bool {
    std::fs::read_dir(parent).unwrap().all(|entry| {
        !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".projection-candidate-")
    })
}

mod rebuild;
mod recovery;
