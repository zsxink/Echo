//! Disposable reconstruction validates directory evidence before adoption.

use super::super::{
    connection::{apply_migrations, open_writer, quick_check_connection},
    device_state, durability_projection,
    support::storage,
    sync_context,
};
use crate::application::ports::LibraryProjectionValidation;
use crate::domain::{
    ids::LibraryRootId,
    library::{
        durability::{DurableResource, DurableStateChange, LibraryRecoveryRecords, LibraryReplay},
        LibraryId, PortableRecord, PortableRecordEnvelope,
    },
};
use crate::error::Error;
use rusqlite::{params, Connection};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub(super) fn validate_with_legacy(
    parent: &Path,
    root: LibraryRootId,
    manifest_library: LibraryId,
    replay: &LibraryReplay,
    snapshots: &[PortableRecordEnvelope],
) -> Result<LibraryProjectionValidation, Error> {
    if manifest_library.as_uuid().is_nil()
        || replay
            .checkpoint
            .iter()
            .map(|image| image.library_id)
            .chain(replay.commits.iter().map(|commit| commit.batch.library_id))
            .any(|library| library != manifest_library)
    {
        return Err(Error::conflict(
            "projection replay differs from the manifest library",
        ));
    }
    let directory = CandidateDirectory::create(parent)?;
    let result = project_with_legacy(
        &directory.0.join("candidate.sqlite"),
        root,
        replay,
        snapshots,
    )
    .and_then(|connection| {
        candidate_corruption_probe(&connection, root)?;
        verify_with_legacy(&connection, root, manifest_library, replay, snapshots)
    });
    // project/verify owns no actor or reader pool; every SQLite handle closes
    // before the private directory is removed, including on Windows.
    let cleanup = directory.cleanup();
    result.and_then(|report| cleanup.map(|()| report))
}

/// Production builds never alter a candidate between projection and
/// verification. Tests install a one-shot fault here to prove that a candidate
/// whose bytes no longer match the directory is rejected before adoption.
#[cfg(not(test))]
#[allow(clippy::unnecessary_wraps)]
const fn candidate_corruption_probe(
    _connection: &Connection,
    _root: LibraryRootId,
) -> Result<(), Error> {
    Ok(())
}

#[cfg(test)]
type CandidateFault = Box<dyn FnOnce(&Connection) -> Result<(), Error>>;

#[cfg(test)]
thread_local! {
    static CANDIDATE_FAULT: std::cell::RefCell<Option<(LibraryRootId, CandidateFault)>> =
        const { std::cell::RefCell::new(None) };
}

/// Arm a single candidate corruption for one root on the calling thread.
/// `validate` runs on the caller's thread, so the fault can neither leak into a
/// parallel test nor fire for a different library.
#[cfg(test)]
pub(super) fn inject_candidate_corruption(
    root: LibraryRootId,
    fault: impl FnOnce(&Connection) -> Result<(), Error> + 'static,
) {
    CANDIDATE_FAULT.with(|slot| *slot.borrow_mut() = Some((root, Box::new(fault))));
}

#[cfg(test)]
fn candidate_corruption_probe(connection: &Connection, root: LibraryRootId) -> Result<(), Error> {
    CANDIDATE_FAULT.with(|slot| {
        let armed = slot
            .borrow()
            .as_ref()
            .is_some_and(|(armed_root, _)| *armed_root == root);
        if !armed {
            return Ok(());
        }
        match slot.borrow_mut().take() {
            Some((_, fault)) => fault(connection),
            None => Ok(()),
        }
    })
}

fn project_with_legacy(
    path: &Path,
    root: LibraryRootId,
    replay: &LibraryReplay,
    snapshots: &[PortableRecordEnvelope],
) -> Result<Connection, Error> {
    let mut connection = open_writer(path)?;
    apply_migrations(&mut connection)?;
    device_state::load_or_create_device_id(&connection)?;
    let location = path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_string_lossy();
    connection.execute(
        "INSERT INTO library_roots(uuid,absolute_path,normalized_path_key,is_active,write_capable,created_at,updated_at) VALUES(?1,?2,?3,0,0,0,0)",
        params![root.to_string(), location, format!("projection-candidate:{root}")],
    ).map_err(storage)?;
    durability_projection::project_commits(&mut connection, root, replay)?;
    if !snapshots.is_empty() {
        durability_projection::project_legacy_records(&mut connection, root, snapshots)?;
    }
    Ok(connection)
}

fn verify_with_legacy(
    connection: &Connection,
    root: LibraryRootId,
    library: LibraryId,
    replay: &LibraryReplay,
    snapshots: &[PortableRecordEnvelope],
) -> Result<LibraryProjectionValidation, Error> {
    quick_check_connection(connection)?;
    let mut foreign_keys = connection
        .prepare("PRAGMA foreign_key_check")
        .map_err(storage)?;
    if foreign_keys
        .query([])
        .map_err(storage)?
        .next()
        .map_err(storage)?
        .is_some()
    {
        return Err(Error::conflict(
            "temporary projection has invalid foreign keys",
        ));
    }
    let projection = durability_projection::status(connection, root)?;
    let (seq, digest) = replay
        .head()
        .map_or((0, None), |(seq, digest)| (seq, Some(digest)));
    if projection.applied_library_commit_seq != seq
        || projection.applied_head_digest.as_deref() != digest
    {
        return Err(Error::conflict(
            "temporary projection has a different directory head",
        ));
    }
    let retained = durability_projection::read(connection, root)?;
    let expected = Evidence::from_replay_and_legacy(replay, snapshots)?;
    if expected != Evidence::from_retained(&retained)? {
        return Err(Error::conflict(
            "temporary projection lost or changed typed recovery evidence",
        ));
    }
    let queued: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_mutations) OR EXISTS(SELECT 1 FROM sync_outbox)",
            [],
            |row| row.get(0),
        )
        .map_err(storage)?;
    if queued {
        return Err(Error::conflict(
            "temporary projection created upload candidates",
        ));
    }
    Ok(LibraryProjectionValidation {
        library_id: library,
        projection,
        retained_records: retained.records.len(),
        retained_state: retained.state.len(),
        retained_resources: retained.resources.len(),
    })
}

pub(super) fn replace_active_with_legacy(
    connection: &mut Connection,
    root: LibraryRootId,
    replay: &LibraryReplay,
    snapshots: &[PortableRecordEnvelope],
    report: LibraryProjectionValidation,
) -> Result<LibraryProjectionValidation, Error> {
    let tx = connection.transaction().map_err(storage)?;
    let root_id = root.to_string();
    let exists: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM library_roots WHERE uuid=?1)",
            [&root_id],
            |row| row.get(0),
        )
        .map_err(storage)?;
    if !exists {
        return Err(Error::conflict("projection root is not registered"));
    }
    let has_sync_context: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_root_contexts WHERE local_root_uuid=?1)",
            [&root_id],
            |row| row.get(0),
        )
        .map_err(storage)?;
    // A root that owns a sync context keeps its protocol history in the live
    // tables (when it is the selected context) or in `archived_<table>` (when it
    // is not). That history is directory evidence, not a projection of the read
    // model: no protocol table has a foreign key to `songs` or `playlists`, and
    // only `retry_library_sync_state` may re-project it, under a validated live
    // session. Rebuilding the catalog must therefore leave every protocol table
    // and the selected context exactly as they were — LD-07 keeps unconfirmed
    // intent and conflicts across a rebuild rather than dropping them.
    let protocol = has_sync_context
        .then(|| ProtocolContext::read(&tx))
        .transpose()?;
    let incomplete: bool = tx
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM operation_items i JOIN operation_journal j ON j.operation_uuid=i.operation_uuid WHERE i.library_root_uuid=?1 AND (i.state NOT IN ('completed','rolled_back','database_finalized','restored') OR (i.state='restored' AND j.kind='delete' AND EXISTS(SELECT 1 FROM songs s WHERE s.uuid=COALESCE(i.song_uuid,j.reserved_song_uuid) AND s.availability='pending_delete'))))",
            [&root_id],
            |row| row.get(0),
        )
        .map_err(storage)?;
    if incomplete {
        return Err(Error::conflict(
            "cannot rebuild a library projection while file operations are incomplete",
        ));
    }

    let queued_before = queue_counts(&tx)?;
    // Remove only root-owned read-model rows. Foreign keys cascade song/playlist
    // children; the root registration, runtime epoch, device identity, operation
    // journal, and other roots remain in the shared application database.
    tx.execute(
        "DELETE FROM sync_device_play_counts WHERE song_uuid IN (SELECT uuid FROM songs WHERE library_root_uuid=?1)",
        [&root_id],
    )
    .map_err(storage)?;
    tx.execute(
        "DELETE FROM scan_runs WHERE library_root_uuid=?1",
        [&root_id],
    )
    .map_err(storage)?;
    // Search rows are a plain fts5 mirror with no trigger and no foreign key, so
    // deleting songs would strand hits for objects this rebuild no longer has.
    tx.execute(
        "DELETE FROM song_search WHERE song_uuid IN (SELECT uuid FROM songs WHERE library_root_uuid=?1)",
        [&root_id],
    )
    .map_err(storage)?;
    tx.execute("DELETE FROM songs WHERE library_root_uuid=?1", [&root_id])
        .map_err(storage)?;
    tx.execute(
        "DELETE FROM playlists WHERE library_root_uuid=?1",
        [&root_id],
    )
    .map_err(storage)?;
    // Artist covers are root-owned but only rewritten when retained state still
    // carries the cover; a rebuilt replay that lacks one must not keep the old
    // assignment (a NULL row is the absence of a user cover).
    tx.execute(
        "DELETE FROM artist_covers WHERE library_root_uuid=?1",
        [&root_id],
    )
    .map_err(storage)?;
    tx.execute(
        "DELETE FROM library_recovery_records WHERE library_root_uuid=?1",
        [&root_id],
    )
    .map_err(storage)?;
    tx.execute(
        "DELETE FROM library_applied_operations WHERE library_root_uuid=?1",
        [&root_id],
    )
    .map_err(storage)?;
    tx.execute(
        "DELETE FROM library_projection_watermarks WHERE library_root_uuid=?1",
        [&root_id],
    )
    .map_err(storage)?;

    durability_projection::project_commits_in_transaction(&tx, root, replay)?;
    if !snapshots.is_empty() {
        durability_projection::project_legacy_records_in_transaction(&tx, root, snapshots)?;
    }
    verify_active(&tx, root, replay, snapshots, &report)?;
    if queue_counts(&tx)? != queued_before {
        return Err(Error::conflict(
            "projection rebuild changed synchronization upload candidates",
        ));
    }
    if let Some(protocol) = &protocol {
        if ProtocolContext::read(&tx)? != *protocol {
            return Err(Error::conflict(
                "projection rebuild changed synchronization protocol history",
            ));
        }
    }
    tx.commit().map_err(storage)?;
    Ok(report)
}

/// Row counts of every protocol table plus the selected context, captured
/// before a rebuild so the rebuild can prove it left that history untouched.
///
/// Protocol rows are root-scoped directory evidence rather than a read-model
/// projection, and no protocol table references `songs` or `playlists`, so a
/// catalog replacement must not add or remove any of them. Counts detect the
/// failure that matters here — a cascade or a stray delete silently dropping
/// another root's ledger — without hashing large payload bodies on a path that
/// only runs during recovery.
#[derive(Clone, Debug, Eq, PartialEq)]
struct ProtocolContext {
    active_context: String,
    live: Vec<(&'static str, u64)>,
    archived: Vec<(&'static str, u64)>,
}

impl ProtocolContext {
    fn read(connection: &Connection) -> Result<Self, Error> {
        let active_context: String = connection
            .query_row(
                "SELECT local_root_uuid FROM sync_active_context WHERE singleton=1",
                [],
                |row| row.get(0),
            )
            .map_err(storage)?;
        let mut live = Vec::with_capacity(sync_context::TABLES.len());
        let mut archived = Vec::with_capacity(sync_context::TABLES.len());
        for table in sync_context::TABLES {
            live.push((
                *table,
                count_rows(connection, &format!("SELECT COUNT(*) FROM {table}"))?,
            ));
            archived.push((
                *table,
                count_rows(
                    connection,
                    &format!("SELECT COUNT(*) FROM archived_{table}"),
                )?,
            ));
        }
        Ok(Self {
            active_context,
            live,
            archived,
        })
    }
}

fn count_rows(connection: &Connection, sql: &str) -> Result<u64, Error> {
    connection
        .query_row(sql, [], |row| row.get::<_, i64>(0))
        .map(|count| u64::try_from(count).unwrap_or(0))
        .map_err(storage)
}

fn queue_counts(connection: &Connection) -> Result<(u64, u64), Error> {
    connection
        .query_row(
            "SELECT (SELECT COUNT(*) FROM sync_mutations),(SELECT COUNT(*) FROM sync_outbox)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(storage)
}

fn verify_active(
    connection: &Connection,
    root: LibraryRootId,
    replay: &LibraryReplay,
    snapshots: &[PortableRecordEnvelope],
    expected_report: &LibraryProjectionValidation,
) -> Result<(), Error> {
    integrity_check(connection)?;
    let projection = durability_projection::status(connection, root)?;
    if projection != expected_report.projection {
        return Err(Error::conflict(
            "active projection differs from validated candidate status",
        ));
    }
    let expected = Evidence::from_replay_and_legacy(replay, snapshots)?;
    let actual = Evidence::from_retained(&durability_projection::read(connection, root)?)?;
    if expected != actual {
        return Err(Error::conflict(
            "active projection differs from validated typed recovery evidence",
        ));
    }
    Ok(())
}

fn integrity_check(connection: &Connection) -> Result<(), Error> {
    quick_check_connection(connection)?;
    let mut foreign_keys = connection
        .prepare("PRAGMA foreign_key_check")
        .map_err(storage)?;
    if foreign_keys
        .query([])
        .map_err(storage)?
        .next()
        .map_err(storage)?
        .is_some()
    {
        return Err(Error::conflict("projection has invalid foreign keys"));
    }
    Ok(())
}

#[derive(Default, Eq, PartialEq)]
struct Evidence {
    records: BTreeMap<String, PortableRecordEnvelope>,
    state: BTreeMap<String, DurableStateChange>,
    resources: BTreeMap<String, DurableResource>,
}

impl Evidence {
    fn from_replay_and_legacy(
        replay: &LibraryReplay,
        snapshots: &[PortableRecordEnvelope],
    ) -> Result<Self, Error> {
        let mut evidence = Self::from_replay(replay)?;
        for envelope in snapshots {
            if evidence
                .records
                .insert(record_key(envelope)?, envelope.clone())
                .is_some()
            {
                return Err(Error::conflict("legacy snapshot overlaps journal evidence"));
            }
        }
        Ok(evidence)
    }
    fn from_replay(replay: &LibraryReplay) -> Result<Self, Error> {
        let mut result = Self::default();
        for envelope in replay
            .checkpoint
            .iter()
            .flat_map(|image| &image.records)
            .chain(
                replay
                    .commits
                    .iter()
                    .flat_map(|commit| &commit.batch.records),
            )
        {
            let key = record_key(envelope)?;
            let mut current = envelope.clone();
            if let Some(previous) = result.records.get(&key) {
                if previous.record().is_tombstone() && !current.record().is_tombstone() {
                    continue;
                }
                if let (PortableRecord::PlayStats(before), PortableRecord::PlayStats(after)) =
                    (previous.record(), current.record())
                {
                    let mut merged = after.clone();
                    for (device, count) in &before.by_device {
                        let bucket = merged.by_device.entry(*device).or_default();
                        *bucket = (*bucket).max(*count);
                    }
                    current = PortableRecordEnvelope::new(
                        PortableRecord::PlayStats(merged),
                        current.sync_metadata().cloned(),
                    )?;
                }
            }
            result.records.insert(key, current);
        }
        for state in replay.states() {
            result.state.insert(state.storage_key()?, state.clone());
        }
        for resource in replay
            .checkpoint
            .iter()
            .flat_map(|image| &image.resources)
            .chain(
                replay
                    .commits
                    .iter()
                    .flat_map(|commit| &commit.batch.resources),
            )
        {
            result
                .resources
                .insert(resource.relative_path.clone(), resource.clone());
        }
        Ok(result)
    }

    fn from_retained(retained: &LibraryRecoveryRecords) -> Result<Self, Error> {
        let mut result = Self::default();
        for envelope in &retained.records {
            if result
                .records
                .insert(record_key(envelope)?, envelope.clone())
                .is_some()
            {
                return Err(Error::conflict("duplicate retained object identity"));
            }
        }
        for state in &retained.state {
            if result
                .state
                .insert(state.storage_key()?, state.clone())
                .is_some()
            {
                return Err(Error::conflict("duplicate retained state identity"));
            }
        }
        for resource in &retained.resources {
            if result
                .resources
                .insert(resource.relative_path.clone(), resource.clone())
                .is_some()
            {
                return Err(Error::conflict("duplicate retained resource identity"));
            }
        }
        Ok(result)
    }
}

fn record_key(envelope: &PortableRecordEnvelope) -> Result<String, Error> {
    serde_json::to_string(&envelope.record().object_key()?).map_err(storage)
}

struct CandidateDirectory(PathBuf);

impl CandidateDirectory {
    fn create(parent: &Path) -> Result<Self, Error> {
        let path = parent.join(format!(".projection-candidate-{}", uuid::Uuid::new_v4()));
        // The candidate holds a full copy of library data: create it
        // owner-restricted from its first visible moment (the platform
        // primitive lives in the filesystem layer, below this adapter).
        crate::infrastructure::filesystem::private_dir::create_owner_only_dir(&path)?;
        Ok(Self(path))
    }

    fn cleanup(&self) -> Result<(), Error> {
        std::fs::remove_dir_all(&self.0)
            .map_err(|error| Error::io("remove projection candidate", error, &self.0))
    }
}

impl Drop for CandidateDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
#[path = "projection_rebuild_tests/mod.rs"]
mod tests;

#[cfg(test)]
fn project(path: &Path, root: LibraryRootId, replay: &LibraryReplay) -> Result<Connection, Error> {
    project_with_legacy(path, root, replay, &[])
}

#[cfg(test)]
fn verify(
    connection: &Connection,
    root: LibraryRootId,
    library: LibraryId,
    replay: &LibraryReplay,
) -> Result<LibraryProjectionValidation, Error> {
    verify_with_legacy(connection, root, library, replay, &[])
}
