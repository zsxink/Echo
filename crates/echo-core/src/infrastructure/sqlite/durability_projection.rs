//! Ordered, atomic projection of a verified library replay. Recovery evidence is
//! retained independently of whether a relationship's parents are available.
mod confirmation;
mod legacy;
mod records;
mod retained;
mod sync_state;
#[cfg(test)]
mod tests;
mod validation;

use super::support::storage;
use crate::domain::ids::LibraryRootId;
use crate::domain::library::durability::{
    LibraryProjectionStatus, LibraryRecoveryRecords, LibraryReplay,
};
use crate::error::Error;
use rusqlite::{Connection, OptionalExtension, Transaction, params};

pub fn project_legacy_records(
    connection: &mut Connection,
    root: LibraryRootId,
    records: &[crate::domain::library::PortableRecordEnvelope],
) -> Result<(), Error> {
    legacy::project(connection, root, records)
}

pub(super) fn project_legacy_records_in_transaction(
    connection: &Connection,
    root: LibraryRootId,
    records: &[crate::domain::library::PortableRecordEnvelope],
) -> Result<(), Error> {
    legacy::project_in_transaction(connection, root, records)
}

pub fn project_commits(
    connection: &mut Connection,
    root: LibraryRootId,
    replay: &LibraryReplay,
) -> Result<(), Error> {
    let tx = connection.transaction().map_err(storage)?;
    project_commits_in_transaction(&tx, root, replay)?;
    tx.commit().map_err(storage)
}

/// Ignore a projection pass whose journal snapshot has already been surpassed
/// by a later serialized pass or foreground catch-up on the same root.
pub(super) fn project_commits_unless_superseded(
    connection: &mut Connection,
    root: LibraryRootId,
    replay: &LibraryReplay,
) -> Result<(), Error> {
    let projected: Option<u64> = connection
        .query_row(
            "SELECT commit_seq FROM library_projection_watermarks WHERE library_root_uuid=?1",
            [root.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage)?;
    if projected.is_some_and(|sequence| {
        sequence > replay.head().map_or(0, |(sequence, _)| sequence)
    }) {
        return Ok(());
    }
    project_commits(connection, root, replay)
}

/// Cheaply determine whether a verified journal replay is already represented
/// by SQLite. Interactive planners use this to avoid revalidating an unchanged
/// read model on every click.
pub(super) fn is_current(
    connection: &Connection,
    root: LibraryRootId,
    replay: &LibraryReplay,
) -> Result<bool, Error> {
    let projected: Option<(u64, String)> = connection
        .query_row(
            "SELECT commit_seq,commit_digest FROM library_projection_watermarks WHERE library_root_uuid=?1",
            [root.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(storage)?;
    Ok(match (replay.head(), projected) {
        (Some((sequence, digest)), Some((projected_sequence, projected_digest))) => {
            sequence == projected_sequence && digest == projected_digest
        }
        (None, None) => true,
        _ => false,
    })
}

/// Project a complete verified replay inside a caller-owned transaction.
/// Root replacement uses this to make clearing stale rows and installing the
/// replacement head one atomic SQLite commit.
pub fn project_commits_in_transaction(
    tx: &Transaction<'_>,
    root: LibraryRootId,
    replay: &LibraryReplay,
) -> Result<(), Error> {
    let current = validation::validate(tx, root, replay)?;
    let library = replay
        .commits
        .last()
        .map(|commit| commit.batch.library_id)
        .or_else(|| replay.checkpoint.as_ref().map(|image| image.library_id));
    let mut seq = current.map_or(0, |(_, seq, _)| seq);
    let mut retried = false;
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
        retained::save(tx, root, &image.records, &image.state, &image.resources)?;
        // Final tombstones are retried first, freeing deleted unique names.
        retained::retry(tx, root, library)?;
        retried = true;
        watermark(
            tx,
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
            tx,
            root,
            &commit.batch.records,
            &commit.batch.state,
            &commit.batch.resources,
        )?;
        retained::retry(tx, root, library)?;
        retried = true;
        tx.execute("INSERT INTO library_applied_operations(library_root_uuid,operation_uuid,commit_seq,commit_digest) VALUES(?1,?2,?3,?4)",params![root.to_string(),commit.batch.operation_id.to_string(),commit.library_commit_seq,commit.digest]).map_err(storage)?;
        watermark(
            tx,
            root,
            commit.batch.library_id.to_string(),
            commit.library_commit_seq,
            &commit.digest,
        )?;
    }
    // Reopening the same head also retries retained unresolved dependencies.
    if !retried {
        retained::retry(tx, root, library)?;
    }
    Ok(())
}

/// Project only the authenticated suffix after SQLite's existing watermark.
/// The caller must obtain `replay` from the directory durability port's
/// `begin_write`/`recover` boundary; cold or checkpoint-behind projections keep
/// using the full validation path above.
pub(super) fn project_verified_suffix(
    connection: &mut Connection,
    root: LibraryRootId,
    replay: &LibraryReplay,
) -> Result<(), Error> {
    let tx = connection.transaction().map_err(storage)?;
    let projected_watermark: Option<(String, u64, Option<String>)> = tx
        .query_row(
            "SELECT library_id,commit_seq,commit_digest FROM library_projection_watermarks WHERE library_root_uuid=?1",
            [root.to_string()],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(storage)?;
    let library = replay
        .commits
        .last()
        .map(|commit| commit.batch.library_id)
        .or_else(|| replay.checkpoint.as_ref().map(|image| image.library_id))
        .ok_or_else(|| Error::conflict("cannot project an empty directory replay"))?;
    let checkpoint_seq = replay
        .checkpoint
        .as_ref()
        .map_or(0, |image| image.library_commit_seq);
    let mut current_seq = 0;
    let mut predecessor = None;
    if let Some((current_library, seq, digest)) = projected_watermark {
        if current_library != library.to_string() {
            return Err(Error::conflict("database projection belongs to another library"));
        }
        if seq < checkpoint_seq {
            // SQLite is older than the compacted base. Only the full snapshot
            // can bridge that gap safely.
            drop(tx);
            return project_commits(connection, root, replay);
        }
        let expected_digest = replay
            .checkpoint
            .as_ref()
            .filter(|image| image.library_commit_seq == seq)
            .map(|image| image.head_digest.as_str())
            .or_else(|| {
                replay
                    .commits
                    .iter()
                    .find(|commit| commit.library_commit_seq == seq)
                    .map(|commit| commit.digest.as_str())
            });
        if expected_digest != digest.as_deref() {
            return Err(Error::conflict(
                "database watermark differs from verified directory chain",
            ));
        }
        current_seq = seq;
        predecessor = digest;
    } else if checkpoint_seq > 0 {
        drop(tx);
        return project_commits(connection, root, replay);
    }
    let after_watermark = current_seq;
    for commit in replay
        .commits
        .iter()
        .filter(|commit| commit.library_commit_seq > after_watermark)
    {
        if current_seq.checked_add(1) != Some(commit.library_commit_seq)
            || commit.predecessor_digest != predecessor
            || commit.batch.library_id != library
        {
            return Err(Error::conflict("verified directory suffix is incomplete"));
        }
        commit.validate()?;
        retained::save(
            &tx,
            root,
            &commit.batch.records,
            &commit.batch.state,
            &commit.batch.resources,
        )?;
        tx.execute(
            "INSERT INTO library_applied_operations(library_root_uuid,operation_uuid,commit_seq,commit_digest) VALUES(?1,?2,?3,?4)",
            params![root.to_string(), commit.batch.operation_id.to_string(), commit.library_commit_seq, commit.digest],
        )
        .map_err(storage)?;
        watermark(
            &tx,
            root,
            library.to_string(),
            commit.library_commit_seq,
            &commit.digest,
        )?;
        current_seq = commit.library_commit_seq;
        predecessor = Some(commit.digest.clone());
    }
    // The interactive edit lanes currently append standalone favorite or
    // empty-playlist facts, neither of which resolves an older retained
    // dependency. Full recovery/projection performs the global retry pass.
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

pub(super) fn artist_cover_original(
    c: &Connection,
    root: LibraryRootId,
    artist_key: &str,
) -> Result<Option<crate::domain::library::PortableAssetReference>, Error> {
    retained::artist_cover_original(c, root, artist_key)
}
