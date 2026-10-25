//! Disposable reconstruction validates directory evidence before adoption.

use super::super::{
    connection::{apply_migrations, open_writer, quick_check_connection},
    device_state, durability_projection,
    support::storage,
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

pub(super) fn validate(
    parent: &Path,
    root: LibraryRootId,
    manifest_library: LibraryId,
    replay: &LibraryReplay,
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
    let result = project(&directory.0.join("candidate.sqlite"), root, replay)
        .and_then(|connection| verify(&connection, root, manifest_library, replay));
    // project/verify owns no actor or reader pool; every SQLite handle closes
    // before the private directory is removed, including on Windows.
    let cleanup = directory.cleanup();
    result.and_then(|report| cleanup.map(|()| report))
}

fn project(path: &Path, root: LibraryRootId, replay: &LibraryReplay) -> Result<Connection, Error> {
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
    Ok(connection)
}

fn verify(
    connection: &Connection,
    root: LibraryRootId,
    library: LibraryId,
    replay: &LibraryReplay,
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
    let expected = Evidence::from_replay(replay)?;
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

pub(super) fn replace_active(
    connection: &mut Connection,
    root: LibraryRootId,
    replay: &LibraryReplay,
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
    if has_sync_context {
        return Err(Error::unavailable(
            "library projection rebuild",
            "root-scoped sync state requires its own archive-aware rebuild",
        ));
    }
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
    tx.execute("DELETE FROM songs WHERE library_root_uuid=?1", [&root_id])
        .map_err(storage)?;
    tx.execute(
        "DELETE FROM playlists WHERE library_root_uuid=?1",
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
    verify_active(&tx, root, replay, &report)?;
    if queue_counts(&tx)? != queued_before {
        return Err(Error::conflict(
            "projection rebuild changed synchronization upload candidates",
        ));
    }
    tx.commit().map_err(storage)?;
    Ok(report)
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
    expected_report: &LibraryProjectionValidation,
) -> Result<(), Error> {
    integrity_check(connection)?;
    let projection = durability_projection::status(connection, root)?;
    if projection != expected_report.projection {
        return Err(Error::conflict(
            "active projection differs from validated candidate status",
        ));
    }
    let expected = Evidence::from_replay(replay)?;
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
        std::fs::create_dir(&path)
            .map_err(|error| Error::io("create projection candidate", error, &path))?;
        let directory = Self(path);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&directory.0, std::fs::Permissions::from_mode(0o700))
                .map_err(|error| Error::io("protect projection candidate", error, &directory.0))?;
        }
        Ok(directory)
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
#[path = "projection_rebuild_tests.rs"]
mod tests;
