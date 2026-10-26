//! OperationJournalRepository, DeviceIdProvider, SyncStateReader and UnitOfWork impls.

#[allow(clippy::wildcard_imports)]
use super::*;

use super::tx_impl::SqliteTx;

impl OperationJournalRepository for SqliteDatabase {
    fn ensure_operation(
        &self,
        operation: OperationId,
        root: LibraryRootId,
        kind: &str,
        reserved_song: Option<SongId>,
    ) -> Result<(), Error> {
        let kind = kind.to_owned();
        self.writer.run(move |connection| {
            ensure_operation_journal(connection, operation, root, &kind, reserved_song)
        })
    }

    fn item_state(
        &self,
        operation: OperationId,
        item: &str,
    ) -> Result<Option<OperationItem>, Error> {
        let item = item.to_owned();
        self.with_reader(move |connection| operation_item(connection, operation, &item))
    }

    fn upsert_item(&self, operation: OperationId, item: OperationItem) -> Result<(), Error> {
        self.writer.run(move |connection| {
            let transaction = connection.transaction().map_err(storage)?;
            upsert_operation_item(&transaction, operation, item)?;
            transaction.commit().map_err(storage)
        })
    }

    fn items(&self, operation: OperationId) -> Result<Vec<OperationItem>, Error> {
        self.with_reader(move |connection| {
            let mut statement = connection.prepare("SELECT i.kind, i.state, COALESCE(i.song_uuid, j.reserved_song_uuid), i.target_relative_path, i.expected_hash, i.item_key, i.normalized_target_path, i.source_locator, i.staging_relative_path FROM operation_items i JOIN operation_journal j ON j.operation_uuid = i.operation_uuid WHERE i.operation_uuid = ?1 ORDER BY i.item_key").map_err(storage)?;
            let items = statement.query_map(params![operation.to_string()], operation_item_from_row).map_err(storage)?.collect::<Result<Vec<_>, _>>().map_err(storage)?;
            Ok(items)
        })
    }

    fn release_claims(&self, operation: OperationId) -> Result<(), Error> {
        self.writer
            .run(move |connection| release_operation_claims(connection, operation))
    }

    fn set_undo_deadline(&self, operation: OperationId, deadline_ms: i64) -> Result<(), Error> {
        self.writer.run(move |connection| {
            connection
                .execute(
                    "UPDATE operation_journal SET undo_deadline = ?2, updated_at = ?3 WHERE operation_uuid = ?1",
                    params![operation.to_string(), deadline_ms, now_ms()],
                )
                .map_err(storage)?;
            Ok(())
        })
    }

    fn undo_deadline(&self, operation: OperationId) -> Result<Option<i64>, Error> {
        self.with_reader(move |connection| {
            let row: Option<Option<i64>> = connection
                .query_row(
                    "SELECT undo_deadline FROM operation_journal WHERE operation_uuid = ?1",
                    params![operation.to_string()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(storage)?;
            Ok(row.flatten())
        })
    }

    fn incomplete_items(
        &self,
        root: LibraryRootId,
    ) -> Result<Vec<(OperationId, String, OperationItem)>, Error> {
        self.with_reader(move |connection| incomplete_operation_items(connection, root))
    }
}

impl crate::application::ports::DeviceIdProvider for SqliteDatabase {
    fn current_device_id(&self) -> crate::domain::library::DeviceId {
        self.device_id
    }
}

impl crate::application::ports::SyncStateReader for SqliteDatabase {
    fn outbox_revision(
        &self,
        object_type: &str,
        object_uuid: &str,
    ) -> Result<crate::domain::ids::Revision, Error> {
        let object_type = object_type.to_owned();
        let object_uuid = object_uuid.to_owned();
        self.with_reader(move |connection| {
            sync::current_outbox_revision(connection, &object_type, &object_uuid)
        })
        .map(|revision| {
            crate::domain::ids::Revision::from_u64(u64::try_from(revision.max(0)).unwrap_or(0))
        })
    }

    fn object_hlc(
        &self,
        object_type: &str,
        object_uuid: &str,
    ) -> Result<Option<crate::domain::library::HybridLogicalClock>, Error> {
        let object_type = object_type.to_owned();
        let object_uuid = object_uuid.to_owned();
        self.with_reader(move |connection| {
            let sql = match object_type.as_str() {
                sync::KIND_SONG | sync::KIND_FAVORITE => {
                    "SELECT hlc_wall_secs, hlc_counter FROM songs WHERE uuid = ?1"
                }
                sync::KIND_OVERRIDE => {
                    "SELECT hlc_wall_secs, hlc_counter FROM song_overrides WHERE song_uuid = ?1"
                }
                sync::KIND_PLAYLIST => {
                    "SELECT hlc_wall_secs, hlc_counter FROM playlists WHERE uuid = ?1"
                }
                // Kinds with no canonical HLC column (a playlist membership or
                // a play-stats record lives in a join/detail table) answer
                // `None` — "not stamped yet" — rather than failing the caller.
                // A materialized record then falls back to the revision's own
                // ordering, which is the authoritative signal on one device.
                _ => return Ok(None),
            };
            let row: Option<(i64, i64)> = connection
                .query_row(sql, rusqlite::params![object_uuid], |row| {
                    Ok((row.get(0)?, row.get(1)?))
                })
                .optional()
                .map_err(storage)?;
            let (wall, counter) = row.unwrap_or((0, 0));
            let wall = u64::try_from(wall).unwrap_or(0);
            let counter = u32::try_from(counter).unwrap_or(0);
            // `(0, 0)` is the untouched default; treat it as "never stamped".
            if wall == 0 && counter == 0 {
                Ok(None)
            } else {
                Ok(Some(crate::domain::library::HybridLogicalClock::new(
                    wall, counter,
                )))
            }
        })
    }
}

impl UnitOfWork for SqliteDatabase {
    fn artist_cover_edits(&self) -> Option<&dyn crate::application::ports::ArtistCoverEditPort> {
        Some(self)
    }

    fn playback_edits(&self) -> Option<&dyn crate::application::ports::PlaybackEditPort> {
        Some(self)
    }

    fn playlist_cover_edits(
        &self,
    ) -> Option<&dyn crate::application::ports::PlaylistCoverEditPort> {
        Some(self)
    }

    fn local_library_edits(&self) -> Option<&dyn crate::application::ports::LocalLibraryEditPort> {
        Some(self)
    }

    fn with_tx(&self, f: TxWork) -> Result<(), Error> {
        self.writer.run(move |connection| {
            let transaction = connection.transaction().map_err(storage)?;
            {
                let mut access = SqliteTx {
                    transaction: &transaction,
                };
                f(&mut access)?;
            }
            transaction.commit().map_err(storage)
        })
    }
}
