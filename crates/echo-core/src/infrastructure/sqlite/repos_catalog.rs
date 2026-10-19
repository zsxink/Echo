//! SongRepository / CatalogQueryRepository / PlaylistRepository impls.

#[allow(clippy::wildcard_imports)]
use super::*;

impl SongRepository for SqliteDatabase {
    fn by_id(&self, id: SongId) -> Result<Option<Song>, Error> {
        self.with_reader(move |connection| {
            connection
                .query_row(
                    &format!("{} WHERE s.uuid = ?1", SONG_SELECT),
                    params![id.to_string()],
                    song_from_row,
                )
                .optional()
                .map_err(storage)
        })
    }

    fn by_path(
        &self,
        root: LibraryRootId,
        path: &RelativeMediaPath,
    ) -> Result<Option<Song>, Error> {
        // Lookup goes through the filesystem-semantic identity key so a
        // case/NFC-spelling variant resolves to the same song row.
        let path = path.identity_key().to_owned();
        self.with_reader(move |connection| {
            connection
                .query_row(
                    &format!(
                        "{} WHERE s.library_root_uuid = ?1 AND s.normalized_relative_path = ?2",
                        SONG_SELECT
                    ),
                    params![root.to_string(), path],
                    song_from_row,
                )
                .optional()
                .map_err(storage)
        })
    }

    fn all_in_root(&self, root: LibraryRootId) -> Result<Vec<Song>, Error> {
        self.with_reader(move |connection| all_songs_in_root(connection, root))
    }

    fn published_file_paths(
        &self,
        root: LibraryRootId,
    ) -> Result<Vec<(RelativeMediaPath, RelativeMediaPath)>, Error> {
        self.with_reader(|connection| sync_manifests::published_file_paths(connection, root))
    }

    fn published_logical_path(
        &self,
        root: LibraryRootId,
        local: &RelativeMediaPath,
    ) -> Result<Option<RelativeMediaPath>, Error> {
        self.with_reader(|connection| {
            sync_manifests::published_logical_path(connection, root, local)
        })
    }

    fn upsert(&self, song: &Song) -> Result<(), Error> {
        let song = song.clone();
        self.writer.run(move |connection| {
            let transaction = connection.transaction().map_err(storage)?;
            upsert_song(&transaction, &song)?;
            transaction.commit().map_err(storage)
        })
    }

    fn set_availability(&self, id: SongId, availability: SongAvailability) -> Result<(), Error> {
        self.writer.run(move |connection| {
            // The song mutation and the root revision bump are one transaction:
            // readers either see both (new data + new catalog revision, stale
            // cursors invalidated) or neither. A half commit can never leave
            // changed data under a still-valid cursor.
            let transaction = connection.transaction().map_err(storage)?;
            set_song_availability(&transaction, id, availability)?;
            transaction.commit().map_err(storage)
        })
    }

    fn set_favorite(&self, id: SongId, favorite: bool) -> Result<(), Error> {
        self.writer.run(move |connection| {
            let transaction = connection.transaction().map_err(storage)?;
            set_song_favorite(&transaction, id, favorite)?;
            transaction.commit().map_err(storage)
        })
    }

    fn increment_play_count(&self, id: SongId) -> Result<(), Error> {
        self.writer.run(move |connection| {
            let transaction = connection.transaction().map_err(storage)?;
            increment_play_count(&transaction, id)?;
            transaction.commit().map_err(storage)
        })
    }

    fn set_play_count(&self, id: SongId, count: u64) -> Result<(), Error> {
        self.writer.run(move |connection| {
            let transaction = connection.transaction().map_err(storage)?;
            set_play_count(&transaction, id, count)?;
            transaction.commit().map_err(storage)
        })
    }
}

impl CatalogQueryRepository for SqliteDatabase {
    fn all_songs(
        &self,
        sort: SongSort,
        cursor: Option<&OpaqueCursor>,
        limit: usize,
    ) -> Result<Paged<Song>, Error> {
        self.query_active_songs("", sort, cursor, limit)
    }

    fn favorites(
        &self,
        sort: SongSort,
        cursor: Option<&OpaqueCursor>,
        limit: usize,
    ) -> Result<Paged<Song>, Error> {
        self.query_active_songs_respecting_favorites("", true, None, sort, cursor, limit)
    }

    fn recent_100(&self) -> Result<Vec<Song>, Error> {
        self.recent_songs()
    }

    fn playlist_songs(&self, playlist: PlaylistId) -> Result<Vec<Song>, Error> {
        self.with_reader(move |connection| playlist_songs_query(connection, playlist))
    }

    fn counts(&self) -> Result<CatalogCounts, Error> {
        self.with_reader(catalog_counts)
    }

    fn search(
        &self,
        query: &str,
        in_favorites: bool,
        playlist: Option<PlaylistId>,
        sort: SongSort,
        cursor: Option<&OpaqueCursor>,
        limit: usize,
    ) -> Result<Paged<Song>, Error> {
        self.query_active_songs_respecting_favorites(
            query,
            in_favorites,
            playlist,
            sort,
            cursor,
            limit,
        )
    }
}

impl PlaylistRepository for SqliteDatabase {
    fn by_id(&self, id: PlaylistId) -> Result<Option<PlaylistId>, Error> {
        self.with_reader(move |connection| {
            connection
                .query_row(
                    "SELECT uuid FROM playlists WHERE uuid = ?1",
                    params![id.to_string()],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(storage)?
                .map(|value| parse_id(&value, "PlaylistId"))
                .transpose()
        })
    }

    fn name(&self, id: PlaylistId) -> Result<Option<String>, Error> {
        self.with_reader(move |connection| {
            connection
                .query_row(
                    "SELECT display_name FROM playlists WHERE uuid = ?1",
                    params![id.to_string()],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(storage)
        })
    }

    fn cover_key(&self, id: PlaylistId) -> Result<Option<String>, Error> {
        self.with_reader(move |connection| {
            connection
                .query_row(
                    "SELECT cover_asset_key FROM playlists WHERE uuid = ?1",
                    params![id.to_string()],
                    |row| row.get::<_, Option<String>>(0),
                )
                .optional()
                .map_err(storage)
                .map(std::option::Option::flatten)
        })
    }

    fn by_name(
        &self,
        root: LibraryRootId,
        normalized_name: &str,
    ) -> Result<Option<PlaylistId>, Error> {
        let key = playlist_name_key(normalized_name);
        self.with_reader(move |connection| connection.query_row("SELECT uuid FROM playlists WHERE library_root_uuid = ?1 AND normalized_name_key = ?2", params![root.to_string(), key], |row| row.get::<_, String>(0)).optional().map_err(storage)?.map(|value| parse_id(&value, "PlaylistId")).transpose())
    }

    fn list(&self, root: LibraryRootId) -> Result<Vec<PlaylistId>, Error> {
        self.with_reader(move |connection| {
            let mut statement = connection.prepare("SELECT uuid FROM playlists WHERE library_root_uuid = ?1 ORDER BY normalized_name_key, uuid").map_err(storage)?;
            let ids = statement.query_map(params![root.to_string()], |row| row.get::<_, String>(0)).map_err(storage)?.map(|value| value.map_err(storage).and_then(|value| parse_id(&value, "PlaylistId"))).collect();
            ids
        })
    }

    fn create(&self, id: PlaylistId, root: LibraryRootId, name: &str) -> Result<(), Error> {
        let name = name.to_owned();
        self.writer
            .run(move |connection| create_playlist(connection, id, root, &name))
    }

    fn rename(&self, id: PlaylistId, to_normalized_name: &str) -> Result<(), Error> {
        let name = to_normalized_name.to_owned();
        self.writer.run(move |connection| {
            let key = playlist_name_key(&name);
            connection.execute("UPDATE playlists SET display_name = ?2, normalized_name_key = ?3, updated_at = ?4 WHERE uuid = ?1", params![id.to_string(), name, key, now_ms()]).map_err(map_constraint)?;
            // Rename is a playlist logic change → outbox snapshot (task 3.11).
            let payload = serde_json::json!({ "display_name": name });
            statements::enqueue_object_snapshot(
                connection,
                sync::KIND_PLAYLIST,
                &id.to_string(),
                &payload,
                sync::OP_UPSERT,
            )?;
            let now_secs = u64::try_from(now_ms().max(0)).unwrap_or(0) / 1_000;
            statements::advance_object_hlc(
                connection,
                sync::KIND_PLAYLIST,
                &id.to_string(),
                now_secs,
            )?;
            Ok(())
        })
    }

    fn set_cover_key(&self, id: PlaylistId, key: Option<&str>) -> Result<(), Error> {
        let key = key.map(str::to_owned);
        self.writer.run(move |connection| {
            connection
                .execute(
                    "UPDATE playlists SET cover_asset_key = ?2, updated_at = ?3 WHERE uuid = ?1",
                    params![id.to_string(), key, now_ms()],
                )
                .map_err(storage)?;
            Ok(())
        })
    }

    fn delete(&self, id: PlaylistId) -> Result<(), Error> {
        self.writer.run(move |connection| {
            // A user-initiated playlist delete is a durable, syncable tombstone:
            // the deletion itself must propagate cross-device, so it has to
            // enter the mutation stream and not just the local tombstone table.
            // The row is still present here, which is what lets the enqueue
            // resolve the object's owning library root.
            let payload = serde_json::json!({ "tombstone": true });
            let exists: bool = connection
                .query_row(
                    "SELECT EXISTS(SELECT 1 FROM playlists WHERE uuid = ?1)",
                    params![id.to_string()],
                    |row| row.get(0),
                )
                .map_err(storage)?;
            let revision = if exists {
                sync::enqueue_sync(
                    connection,
                    sync::KIND_PLAYLIST,
                    &id.to_string(),
                    &serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_owned()),
                    sync::OP_DELETE,
                )?
            } else {
                sync::current_outbox_revision(connection, sync::KIND_PLAYLIST, &id.to_string())?
                    .saturating_add(1)
            };
            // Replayed deletes of an already-tombstoned playlist stay
            // idempotent: no second mutation, only the durable local tombstone.
            sync::write_tombstone(connection, sync::KIND_PLAYLIST, &id.to_string(), revision)?;
            connection
                .execute(
                    "DELETE FROM playlists WHERE uuid = ?1",
                    params![id.to_string()],
                )
                .map_err(storage)?;
            Ok(())
        })
    }

    fn members(&self, id: PlaylistId) -> Result<Vec<PlaylistMember>, Error> {
        self.with_reader(move |connection| {
            let mut statement = connection.prepare("SELECT ps.member_uuid, ps.playlist_uuid, ps.song_uuid, ps.position, s.availability FROM playlist_songs ps JOIN songs s ON s.uuid = ps.song_uuid WHERE ps.playlist_uuid = ?1 ORDER BY ps.position, ps.song_uuid").map_err(storage)?;
            let members = statement.query_map(params![id.to_string()], |row| {
                let member_uuid = row.get::<_, Option<String>>(0)?.unwrap_or_default();
                Ok(PlaylistMember::with_id(
                    parse_id(&member_uuid, "PlaylistItemId").map_err(to_sql_error)?,
                    parse_id(&row.get::<_, String>(1)?, "PlaylistId").map_err(to_sql_error)?,
                    parse_id(&row.get::<_, String>(2)?, "SongId").map_err(to_sql_error)?,
                    row.get::<_, u64>(3)?,
                    availability_from_db(&row.get::<_, String>(4)?).map_err(to_sql_error)?,
                ))
            }).map_err(storage)?.collect::<Result<Vec<_>, _>>().map_err(storage)?;
            Ok(members)
        })
    }

    fn playlists_for_song(&self, song: SongId) -> Result<Vec<PlaylistId>, Error> {
        self.with_reader(move |connection| {
            let mut statement = connection
                .prepare(
                    "SELECT ps.playlist_uuid FROM playlist_songs ps JOIN playlists p ON p.uuid = ps.playlist_uuid JOIN songs s ON s.uuid = ps.song_uuid AND s.library_root_uuid = p.library_root_uuid WHERE ps.song_uuid = ?1 ORDER BY ps.playlist_uuid",
                )
                .map_err(storage)?;
            let playlists = statement
                .query_map(params![song.to_string()], |row| row.get::<_, String>(0))
                .map_err(storage)?
                .map(|value| {
                    value
                        .map_err(storage)
                        .and_then(|value| parse_id(&value, "PlaylistId"))
                })
                .collect();
            playlists
        })
    }

    fn add_member(&self, playlist: PlaylistId, song: SongId, position: u64) -> Result<(), Error> {
        self.writer.run(move |connection| {
            // Membership lookup + insert are one transaction so a concurrent
            // mutation cannot slip a duplicate or position clash between them.
            let transaction = connection.transaction().map_err(storage)?;
            let member_uuid = crate::domain::ids::PlaylistItemId::new();
            add_member(&transaction, playlist, song, position, member_uuid)?;
            transaction.commit().map_err(storage)
        })
    }

    fn upsert_member(&self, member: &PlaylistMember) -> Result<(), Error> {
        let member = member.clone();
        self.writer.run(move |connection| {
            let transaction = connection.transaction().map_err(storage)?;
            upsert_member(&transaction, &member)?;
            transaction.commit().map_err(storage)
        })
    }

    fn remove_member(&self, playlist: PlaylistId, song: SongId) -> Result<(), Error> {
        self.writer.run(move |connection| {
            // Capture the stable membership identity before erasing its local
            // projection. The tombstone is what prevents a recovery on another
            // device from resurrecting this member.
            let member_uuid: Option<String> = connection
                .query_row(
                    "SELECT member_uuid FROM playlist_songs WHERE playlist_uuid = ?1 AND song_uuid = ?2",
                    params![playlist.to_string(), song.to_string()],
                    |row| row.get(0),
                )
                .optional()
                .map_err(storage)?;
            if let Some(member_uuid) = member_uuid {
                // The membership identity dies here, so this is a typed
                // playlist-item tombstone rather than a plain playlist edit.
                // The host only drops a removed member from a concurrent
                // playlist merge when the member itself is tombstoned
                // (`sync_ledger::exclude_final_playlist_members`), so the
                // delete has to reach the ledger. Enqueue before the row is
                // erased: the object resolves its owning library root through
                // `playlist_songs`.
                let payload = serde_json::json!({ "tombstone": true });
                let revision = sync::enqueue_sync(
                    connection,
                    sync::KIND_PLAYLIST_ITEM,
                    &member_uuid,
                    &serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_owned()),
                    sync::OP_DELETE,
                )?;
                sync::write_tombstone(
                    connection,
                    sync::KIND_PLAYLIST_ITEM,
                    &member_uuid,
                    revision,
                )?;
            }
            connection
                .execute(
                    "DELETE FROM playlist_songs WHERE playlist_uuid = ?1 AND song_uuid = ?2",
                    params![playlist.to_string(), song.to_string()],
                )
                .map_err(storage)?;
            // Membership removal is a playlist logic change.
            let members_json = statements::playlist_members_excerpt(connection, playlist)?;
            let payload = serde_json::json!({ "members": members_json });
            statements::enqueue_object_snapshot(
                connection,
                sync::KIND_PLAYLIST,
                &playlist.to_string(),
                &payload,
                sync::OP_UPSERT,
            )?;
            let now_secs = u64::try_from(now_ms().max(0)).unwrap_or(0) / 1_000;
            statements::advance_object_hlc(
                connection,
                sync::KIND_PLAYLIST,
                &playlist.to_string(),
                now_secs,
            )?;
            Ok(())
        })
    }
}

impl LibraryRepository for SqliteDatabase {
    fn replica_enrollment_pending(&self, root: LibraryRootId) -> Result<bool, Error> {
        Self::replica_enrollment_pending(self, root)
    }
    fn active_root(&self) -> Result<Option<LibraryRoot>, Error> {
        self.with_reader(|connection| {
            connection
                .query_row("SELECT uuid, absolute_path, is_active, write_capable, availability, write_safety_locked FROM library_roots WHERE is_active = 1", [], root_from_row)
                .optional()
                .map_err(storage)
        })
    }

    fn by_id(&self, id: LibraryRootId) -> Result<Option<LibraryRoot>, Error> {
        self.with_reader(move |connection| {
            connection
                .query_row("SELECT uuid, absolute_path, is_active, write_capable, availability, write_safety_locked FROM library_roots WHERE uuid = ?1", params![id.to_string()], root_from_row)
                .optional()
                .map_err(storage)
        })
    }

    fn upsert(&self, root: &LibraryRoot) -> Result<(), Error> {
        let root = root.clone();
        self.writer.run(move |connection| {
            let transaction = connection.transaction().map_err(storage)?;
            upsert_root(&transaction, &root)?;
            transaction.commit().map_err(storage)
        })
    }

    fn deactivate(&self, id: LibraryRootId) -> Result<(), Error> {
        self.writer.run(move |connection| {
            connection
                .execute(
                    "UPDATE library_roots SET is_active = 0, updated_at = ?2 WHERE uuid = ?1",
                    params![id.to_string(), now_ms()],
                )
                .map_err(storage)?;
            Ok(())
        })
    }
}

impl RootWriteRepository for SqliteDatabase {
    fn set_write_and_availability(
        &self,
        id: LibraryRootId,
        write_capable: bool,
        available: bool,
    ) -> Result<(), Error> {
        self.writer.run(move |connection| {
            connection.execute("UPDATE library_roots SET write_capable = ?2, availability = ?3, updated_at = ?4 WHERE uuid = ?1", params![id.to_string(), i64::from(write_capable), if available { "available" } else { "unavailable" }, now_ms()]).map_err(storage)?;
            Ok(())
        })
    }

    fn set_write_safety_locked(&self, id: LibraryRootId, locked: bool) -> Result<(), Error> {
        self.writer.run(move |connection| {
            connection.execute("UPDATE library_roots SET write_safety_locked = ?2, updated_at = ?3 WHERE uuid = ?1", params![id.to_string(), i64::from(locked), now_ms()]).map_err(storage)?;
            Ok(())
        })
    }
}
