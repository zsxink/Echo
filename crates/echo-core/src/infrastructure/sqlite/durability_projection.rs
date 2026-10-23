//! Ordered, atomic projection of a verified library replay. Recovery evidence is
//! retained independently of whether a relationship's parents are available.
mod records;
mod retained;
#[cfg(test)]
mod tests;
mod validation;

use super::support::storage;
use crate::domain::ids::LibraryRootId;
use crate::domain::library::durability::{
    LibraryProjectionStatus, LibraryRecoveryRecords, LibraryReplay,
};
use crate::error::Error;
use rusqlite::{params, Connection};

pub fn project_commits(
    connection: &mut Connection,
    root: LibraryRootId,
    replay: &LibraryReplay,
) -> Result<(), Error> {
    let current = validation::validate(connection, root, replay)?;
    let tx = connection.transaction().map_err(storage)?;
    let mut seq = current.map_or(0, |(_, seq, _)| seq);
    if let Some(image) = replay
        .checkpoint
        .as_ref()
        .filter(|image| seq < image.library_commit_seq)
    {
        tx.execute(
            "DELETE FROM library_recovery_records WHERE library_root_uuid=?1",
            [root.to_string()],
        )
        .map_err(storage)?;
        retained::save(&tx, root, &image.records, &image.state, &image.resources)?;
        // Final tombstones are retried first, freeing deleted unique names.
        retained::retry(&tx, root)?;
        watermark(
            &tx,
            root,
            image.library_id.to_string(),
            image.library_commit_seq,
            &image.head_digest,
        )?;
        seq = image.library_commit_seq;
    }
    for commit in &replay.commits {
        if commit.library_commit_seq <= seq {
            continue;
        }
        retained::save(
            &tx,
            root,
            &commit.batch.records,
            &commit.batch.state,
            &commit.batch.resources,
        )?;
        retained::retry(&tx, root)?;
        tx.execute("INSERT INTO library_applied_operations(library_root_uuid,operation_uuid,commit_seq,commit_digest) VALUES(?1,?2,?3,?4)",params![root.to_string(),commit.batch.operation_id.to_string(),commit.library_commit_seq,commit.digest]).map_err(storage)?;
        watermark(
            &tx,
            root,
            commit.batch.library_id.to_string(),
            commit.library_commit_seq,
            &commit.digest,
        )?;
    }
    // Reopening the same head also retries retained unresolved dependencies.
    retained::retry(&tx, root)?;
    tx.commit().map_err(storage)
}

fn watermark(
    c: &Connection,
    root: LibraryRootId,
    library: String,
    seq: u64,
    digest: &str,
) -> Result<(), Error> {
    c.execute("INSERT INTO library_projection_watermarks(library_root_uuid,library_id,commit_seq,commit_digest) VALUES(?1,?2,?3,?4) ON CONFLICT(library_root_uuid) DO UPDATE SET commit_seq=excluded.commit_seq,commit_digest=excluded.commit_digest",params![root.to_string(),library,seq,digest]).map_err(storage)?;
    Ok(())
}

pub fn status(c: &Connection, root: LibraryRootId) -> Result<LibraryProjectionStatus, Error> {
    retained::status(c, root)
}
pub fn read(c: &Connection, root: LibraryRootId) -> Result<LibraryRecoveryRecords, Error> {
    retained::read(c, root)
}
