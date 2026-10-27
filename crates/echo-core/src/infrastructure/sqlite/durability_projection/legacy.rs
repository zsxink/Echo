//! Snapshot compatibility does not manufacture ordered directory history.

use super::{retained, storage};
use crate::domain::ids::LibraryRootId;
use crate::domain::library::{PortableRecord, PortableRecordEnvelope};
use crate::error::Error;
use rusqlite::{Connection, params};

/// The caller has already selected the effective legacy snapshot under its
/// historical version/tombstone rules and excluded targets covered by a real
/// directory replay. Keep unresolved references without minting sync events.
pub(super) fn project(
    connection: &mut Connection,
    root: LibraryRootId,
    records: &[PortableRecordEnvelope],
) -> Result<(), Error> {
    let tx = connection.transaction().map_err(storage)?;
    project_in_transaction(&tx, root, records)?;
    tx.commit().map_err(storage)
}

pub(super) fn project_in_transaction(
    tx: &Connection,
    root: LibraryRootId,
    records: &[PortableRecordEnvelope],
) -> Result<(), Error> {
    for record in records {
        record.validate()?;
    }
    for envelope in records {
        let key = serde_json::to_string(&envelope.record().object_key()?).map_err(storage)?;
        // Legacy snapshots can replace an older temporary tombstone. The
        // permanent tombstone rule belongs to verified directory commits.
        tx.execute(
            "DELETE FROM library_recovery_records WHERE library_root_uuid=?1 AND object_key=?2 AND row_kind='record'",
            params![root.to_string(), key],
        ).map_err(storage)?;
        if let PortableRecord::Song(song) = envelope.record() {
            tx.execute(
                "UPDATE songs SET availability='missing' WHERE uuid=?1 AND library_root_uuid=?2 AND availability='pending_delete'
                 AND NOT EXISTS(SELECT 1 FROM sync_object_versions v WHERE v.object_type='song' AND v.object_uuid=songs.uuid AND v.is_tombstone=1)
                 AND NOT EXISTS(SELECT 1 FROM sync_local_delete_intents d WHERE d.object_type='song' AND d.object_uuid=songs.uuid AND d.state='open')",
                params![song.song_uuid.to_string(), root.to_string()],
            ).map_err(storage)?;
        }
    }
    retained::save(tx, root, records, &[], &[])?;
    retained::retry(tx, root, None)
}
