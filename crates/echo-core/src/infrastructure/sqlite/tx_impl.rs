//! `SqliteTx` plus the transactional writer port impls.

#[allow(clippy::wildcard_imports)]
use super::*;

pub(super) struct SqliteTx<'connection> {
    pub(super) transaction: &'connection Transaction<'connection>,
}

impl TxSongWriter for SqliteTx<'_> {
    fn upsert_song(&mut self, song: &Song) -> Result<(), Error> {
        upsert_song(self.transaction, song)
    }
    fn refresh_song_file_index(&mut self, song: &Song) -> Result<(), Error> {
        statements::refresh_song_file_index(self.transaction, song)
    }
    fn delete_song(&mut self, id: SongId) -> Result<(), Error> {
        delete_song(self.transaction, id)
    }
    fn set_song_availability(
        &mut self,
        id: SongId,
        availability: SongAvailability,
    ) -> Result<(), Error> {
        set_song_availability(self.transaction, id, availability)
    }
    fn set_song_favorite(&mut self, id: SongId, favorite: bool) -> Result<(), Error> {
        set_song_favorite(self.transaction, id, favorite)
    }
    fn increment_song_play_count(&mut self, id: SongId) -> Result<(), Error> {
        increment_play_count(self.transaction, id)
    }
}

impl TxRootWriter for SqliteTx<'_> {
    fn upsert_root(&mut self, root: &LibraryRoot) -> Result<(), Error> {
        upsert_root(self.transaction, root)
    }
    fn isolate_root_writes(&mut self, id: LibraryRootId, available: bool) -> Result<(), Error> {
        self.transaction
            .execute(
                "UPDATE library_roots SET write_safety_locked = 1, availability = ?2, updated_at = ?3 WHERE uuid = ?1",
                params![id.to_string(), if available { "available" } else { "unavailable" }, now_ms()],
            )
            .map_err(storage)?;
        Ok(())
    }
}

impl TxPlaylistWriter for SqliteTx<'_> {
    fn create_playlist(
        &mut self,
        id: PlaylistId,
        root: LibraryRootId,
        name: &str,
    ) -> Result<(), Error> {
        create_playlist(self.transaction, id, root, name)
    }
    fn insert_member(&mut self, member: &PlaylistMember) -> Result<(), Error> {
        add_member(
            self.transaction,
            member.playlist(),
            member.song(),
            member.position(),
            member.id(),
        )
    }
    fn remove_member(&mut self, playlist: PlaylistId, song: SongId) -> Result<(), Error> {
        self.transaction
            .execute(
                "DELETE FROM playlist_songs WHERE playlist_uuid = ?1 AND song_uuid = ?2",
                params![playlist.to_string(), song.to_string()],
            )
            .map_err(storage)?;
        Ok(())
    }
}

impl TxOperationWriter for SqliteTx<'_> {
    fn upsert_operation_item(
        &mut self,
        operation: OperationId,
        item: OperationItem,
    ) -> Result<(), Error> {
        upsert_operation_item(self.transaction, operation, item)
    }
    fn release_operation_claims(&mut self, operation: OperationId) -> Result<(), Error> {
        release_operation_claims(self.transaction, operation)
    }
    fn set_undo_deadline(
        &mut self,
        operation: OperationId,
        deadline_ms: i64,
        final_mutation_id: crate::domain::sync::MutationId,
    ) -> Result<(), Error> {
        self.transaction
            .execute(
                "UPDATE operation_journal SET undo_deadline = ?2, updated_at = ?3 WHERE operation_uuid = ?1",
                params![operation.to_string(), deadline_ms, now_ms()],
            )
            .map_err(storage)?;
        let song_uuid: Option<String> = self
            .transaction
            .query_row(
                "SELECT reserved_song_uuid FROM operation_journal WHERE operation_uuid = ?1",
                params![operation.to_string()],
                |row| row.get(0),
            )
            .optional()
            .map_err(storage)?
            .flatten();
        if let Some(song_uuid) = song_uuid {
            // The directory intent froze this identity before the hide, so a
            // concurrent finalization and the directory replay agree on the
            // one mutation that ends the object.
            self.transaction
                .execute(
                    "INSERT OR IGNORE INTO sync_local_delete_intents (intent_id, final_mutation_id, object_type, object_uuid, created_at, undo_deadline, state) VALUES (?1, ?2, 'song', ?3, ?4, ?5, 'open')",
                    params![
                        operation.to_string(),
                        final_mutation_id.as_uuid().to_string(),
                        song_uuid,
                        now_ms(),
                        deadline_ms,
                    ],
                )
                .map_err(storage)?;
        }
        Ok(())
    }
}

impl TxLyricsWriter for SqliteTx<'_> {
    fn set_lyrics_candidate(
        &mut self,
        song: SongId,
        candidate: &LyricsCandidate,
    ) -> Result<(), Error> {
        set_lyrics_candidate(self.transaction, song, candidate)
    }
    fn clear_lyrics_candidate(&mut self, song: SongId, source: LyricsSource) -> Result<(), Error> {
        clear_lyrics_candidate(self.transaction, song, source)
    }
}

impl TxStateWriter for SqliteTx<'_> {
    fn set_song_file_resources(
        &mut self,
        song: &Song,
        resources: &[crate::domain::sync::SharedResourceManifest],
    ) -> Result<(), Error> {
        sync_manifests::replace_song_files(self.transaction, song, resources)
    }
    fn attach_cover(&mut self, song: SongId, cover: &CoverAssetRef) -> Result<(), Error> {
        attach_cover(self.transaction, song, cover)
    }
    fn set_runtime_state(&mut self, key: &str, value: &str) -> Result<(), Error> {
        store_runtime_state(self.transaction, key, value)
    }
}

impl LyricsRepository for SqliteDatabase {
    fn candidates(&self, song: SongId) -> Result<Vec<LyricsCandidate>, Error> {
        self.with_reader(move |connection| lyrics_candidates(connection, song))
    }
}

impl CoverRepository for SqliteDatabase {
    fn cover_of(&self, song: SongId) -> Result<Option<CoverAssetRef>, Error> {
        self.with_reader(move |connection| cover_of_song(connection, song))
    }
    fn artist_cover_key(
        &self,
        root: LibraryRootId,
        artist_key: &str,
    ) -> Result<Option<String>, Error> {
        let artist_key = artist_key.to_owned();
        self.with_reader(move |connection| connection.query_row(
            "SELECT cover_asset_key FROM artist_covers WHERE library_root_uuid = ?1 AND artist_key = ?2",
            params![root.to_string(), artist_key],
            |row| row.get(0),
        ).optional().map_err(storage))
    }
    fn set_artist_cover_key(
        &self,
        root: LibraryRootId,
        artist_key: &str,
        key: Option<&str>,
    ) -> Result<(), Error> {
        let artist_key = artist_key.to_owned();
        let key = key.map(str::to_owned);
        self.writer.run(move |connection| {
            if let Some(key) = key {
                connection.execute(
                    "INSERT INTO artist_covers (library_root_uuid, artist_key, cover_asset_key, updated_at) VALUES (?1, ?2, ?3, ?4) ON CONFLICT(library_root_uuid, artist_key) DO UPDATE SET cover_asset_key = excluded.cover_asset_key, updated_at = excluded.updated_at",
                    params![root.to_string(), artist_key, key, now_ms()],
                ).map_err(storage)?;
            } else {
                connection.execute(
                    "DELETE FROM artist_covers WHERE library_root_uuid = ?1 AND artist_key = ?2",
                    params![root.to_string(), artist_key],
                ).map_err(storage)?;
            }
            Ok(())
        })
    }
    fn referenced_asset_keys(&self, root: LibraryRootId) -> Result<Vec<String>, Error> {
        self.with_reader(move |connection| referenced_asset_keys(connection, root))
    }

    fn all_referenced_asset_keys(&self) -> Result<Vec<String>, Error> {
        self.with_reader(all_referenced_asset_keys)
    }
}

impl ScanRunRepository for SqliteDatabase {
    fn begin_run(&self, root: LibraryRootId, generation: u64) -> Result<(), Error> {
        self.writer
            .run(move |connection| begin_scan_run(connection, root, generation))
    }
    fn update_progress(
        &self,
        root: LibraryRootId,
        generation: u64,
        progress: &ScanProgress,
    ) -> Result<(), Error> {
        let progress = *progress;
        self.writer
            .run(move |connection| update_scan_progress(connection, root, generation, &progress))
    }
    fn record_issue(
        &self,
        root: LibraryRootId,
        generation: u64,
        issue: &MediaDiagnostic,
    ) -> Result<(), Error> {
        let issue = issue.clone();
        self.writer
            .run(move |connection| record_scan_issue(connection, root, generation, &issue))
    }
    fn finish_run(
        &self,
        root: LibraryRootId,
        generation: u64,
        state: ScanState,
        progress: &ScanProgress,
    ) -> Result<(), Error> {
        let progress = *progress;
        self.writer
            .run(move |connection| finish_scan_run(connection, root, generation, state, &progress))
    }
    fn latest_generation(&self, root: LibraryRootId) -> Result<Option<u64>, Error> {
        self.with_reader(move |connection| latest_scan_generation(connection, root))
    }
    fn issues(&self, root: LibraryRootId, generation: u64) -> Result<Vec<MediaDiagnostic>, Error> {
        self.scan_issues(root, generation)
    }
}

impl RuntimeStateStore for SqliteDatabase {
    fn load(&self, key: &str) -> Result<Option<String>, Error> {
        let key = key.to_owned();
        self.with_reader(move |connection| load_runtime_state(connection, &key))
    }
}
