//! Inherent `SqliteDatabase` methods: open/maintenance/playback/scan-run/queries.

#[allow(clippy::wildcard_imports)]
use super::*;

impl SqliteDatabase {
    /// Open (or create) a database, configure safety pragmas, apply migrations
    /// and verify its quick integrity check.
    ///
    /// The database's parent directory is created when absent: SQLite's
    /// `Connection::open` refuses to create parent directories, so a cold start
    /// on a fresh machine — where the platform app-data directory does not yet
    /// exist — would otherwise fail before the first migration.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref().to_path_buf();
        // SQLite's `Connection::open` refuses to create parent directories, so
        // a cold start on a fresh machine — where the platform app-data
        // directory does not yet exist — must materialize it here (regression:
        // `open_creates_a_missing_parent_directory` in schema.rs).
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|source| {
                Error::io(
                    "create database parent directory",
                    source,
                    parent.to_path_buf(),
                )
            })?;
        }
        let had_database = path.exists() && file_is_non_empty(&path)?;
        let mut writer = open_writer(&path)?;
        if had_database {
            let backup = backup_path(&path);
            backup_connection(&writer, &backup)?;
        }
        apply_migrations(&mut writer)?;
        quick_check_connection(&writer)?;
        // Load/mint the device identity on the writer connection before any
        // reader is handed out, so every portable record sees it.
        let device_id = device_state::load_or_create_device_id(&writer)?;
        device_state::backfill_legacy_play_counts(&mut writer, device_id)?;

        let mut readers = Vec::with_capacity(reader_count());
        for _ in 0..reader_count() {
            readers.push(open_reader(&path)?);
        }
        Ok(Self {
            path,
            writer: SqliteWriteActor::spawn(writer),
            readers: ReaderPool::new(readers),
            device_id,
        })
    }

    /// Path of the local database; it is for runtime ownership only and must
    /// never be serialized to UI/sync payloads.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Subscribe to successful SQLite writes as a sync scheduler wake-up hint.
    /// Callers must coalesce notifications and re-read the durable queue; the
    /// event carries no data and never crosses the IPC boundary.
    pub fn subscribe_writes(&self) -> mpsc::Receiver<()> {
        self.writer.subscribe()
    }

    /// Register a channel for newly queued local mutations and newly published
    /// peer work. Sync's own bookkeeping does not wake another sync cycle. Stop
    /// signals may share the same channel so background workers wake promptly.
    pub fn register_sync_waker(&self, sender: mpsc::Sender<()>) {
        self.writer.register_sync_listener(sender);
    }

    /// Create a consistent SQLite backup using the SQLite backup API.
    pub fn backup_to(&self, destination: impl AsRef<Path>) -> Result<(), Error> {
        let destination = destination.as_ref().to_path_buf();
        self.with_reader(|connection| backup_connection(connection, &destination))
    }

    /// Execute SQLite's cheap integrity probe. Call this before accepting a
    /// database as healthy after startup/recovery.
    pub fn quick_check(&self) -> Result<(), Error> {
        self.with_reader(quick_check_connection)
    }

    /// Count outbox rows of one object kind. Diagnostics / 二期 read; 0.1.0
    /// never pushes them, so this is a shape probe, not a sync surface.
    pub fn outbox_kind_count(&self, kind: &str) -> Result<i64, Error> {
        let kind = kind.to_owned();
        self.with_reader(move |connection| sync::outbox_count(connection, &kind))
    }

    /// Read one `sync_state` key (connector config / cursor / schema marker).
    pub fn sync_state_load(&self, key: &str) -> Result<Option<String>, Error> {
        let key = key.to_owned();
        self.with_reader(move |connection| sync::load_sync_state(connection, &key))
    }

    /// A deterministic schema snapshot used by migration integration tests.
    pub fn schema_snapshot(&self) -> Result<Vec<(String, String)>, Error> {
        self.with_reader(|connection| {
            let mut statement = connection
                .prepare("SELECT name, sql FROM sqlite_master WHERE type IN ('table', 'index') AND name NOT LIKE 'sqlite_%' ORDER BY name")
                .map_err(storage)?;
            let snapshot = statement
                .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?.unwrap_or_default())))
                .map_err(storage)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage)?;
            Ok(snapshot)
        })
    }

    /// Create the durable envelope required before adding journal items. The
    /// application later advances item states through `OperationJournalRepository`.
    pub fn create_operation(
        &self,
        operation: OperationId,
        root: LibraryRootId,
        kind: &str,
        reserved_song: Option<SongId>,
    ) -> Result<(), Error> {
        let kind = kind.to_owned();
        self.writer.run(move |connection| {
            connection
                .execute(
                    "INSERT INTO operation_journal (operation_uuid, library_root_uuid, kind, state, reserved_song_uuid, created_at, updated_at) VALUES (?1, ?2, ?3, 'planned', ?4, ?5, ?5)",
                    params![operation.to_string(), root.to_string(), kind, reserved_song.map(|id| id.to_string()), now_ms()],
                )
                .map_err(map_constraint)?;
            Ok(())
        })
    }

    /// Idempotently record a qualified library playback session. The song
    /// count is incremented only when the session row is newly inserted.
    pub fn record_playback(&self, session: PlaybackSessionId, song: SongId) -> Result<bool, Error> {
        self.writer.run(move |connection| {
            let transaction = connection.transaction().map_err(storage)?;
            let inserted = transaction
                .execute(
                    "INSERT OR IGNORE INTO recorded_play_sessions (playback_session_uuid, song_uuid, recorded_at) VALUES (?1, ?2, ?3)",
                    params![session.to_string(), song.to_string(), now_ms()],
                )
                .map_err(storage)?;
            if inserted == 1 {
                transaction
                    .execute("UPDATE songs SET play_count = play_count + 1, updated_at = ?2 WHERE uuid = ?1", params![song.to_string(), now_ms()])
                    .map_err(storage)?;
                statements::touch_root_for_song(&transaction, song)?;
                let device_id: String = transaction
                    .query_row("SELECT value FROM device_state WHERE key = 'device_id'", [], |row| row.get(0))
                    .map_err(storage)?;
                transaction
                    .execute(
                        "INSERT INTO sync_device_play_counts (song_uuid, device_id, play_count) VALUES (?1, ?2, 1) ON CONFLICT(song_uuid, device_id) DO UPDATE SET play_count = play_count + 1",
                        params![song.to_string(), device_id],
                    )
                    .map_err(storage)?;
                let counts = read_device_play_counts(&transaction, song)?;
                // The host merge deserializes this payload as a complete
                // `PlayStatsRecord` (design §3 播放次数). A `{song_uuid,
                // by_device}` sketch is not decodable there and is rejected as
                // `WrongObject`, so the record shape is built once, here, from
                // the same helpers the portable record uses.
                let device_uuid = uuid::Uuid::parse_str(&device_id).map_err(|_| {
                    crate::error::Error::validation(
                        crate::error::Subject::Other,
                        "device id",
                        "device id is not a UUID",
                    )
                })?;
                let now_secs = u64::try_from(now_ms().max(0)).unwrap_or(0) / 1_000;
                let hlc = statements::advance_object_hlc(
                    &transaction,
                    sync::KIND_PLAY_STATS,
                    &song.to_string(),
                    now_secs,
                )?;
                // `enqueue_object_snapshot` assigns the next local revision, so
                // stamp the same ordinal the outbox row will carry.
                let next_revision = sync::current_outbox_revision(
                    &transaction,
                    sync::KIND_PLAY_STATS,
                    &song.to_string(),
                )?
                .saturating_add(1);
                let payload = serde_json::to_value(
                    crate::application::portable_materialize::play_stats_record(
                        crate::domain::library::DeviceId::from_uuid(device_uuid),
                        hlc,
                        crate::domain::ids::Revision::from_u64(
                            u64::try_from(next_revision).unwrap_or(0),
                        ),
                        song,
                        counts,
                    ),
                )
                .map_err(storage)?;
                statements::enqueue_object_snapshot(
                    &transaction,
                    sync::KIND_PLAY_STATS,
                    &song.to_string(),
                    &payload,
                    sync::OP_UPSERT,
                )?;
            }
            transaction.commit().map_err(storage)?;
            Ok(inserted == 1)
        })
    }

    /// Return durable per-device play buckets for portable projection and
    /// synchronization recovery.
    pub fn device_play_counts(&self, song: SongId) -> Result<BTreeMap<uuid::Uuid, u64>, Error> {
        self.with_reader(move |connection| read_device_play_counts(connection, song))
    }

    /// Search only the active root. Three or more Unicode scalar values use
    /// the trigram FTS table; shorter values use escaped normalized LIKE.
    pub fn search_active_songs(
        &self,
        query: &str,
        sort: SongSort,
        cursor: Option<&OpaqueCursor>,
        limit: usize,
    ) -> Result<Paged<Song>, Error> {
        self.query_active_songs_respecting_favorites(query, false, None, sort, cursor, limit)
    }

    /// Return a cursor page for the active root. Cursor boundaries are real
    /// keyset predicates over the selected sort columns, never `OFFSET`.
    pub fn query_active_songs(
        &self,
        query: &str,
        sort: SongSort,
        cursor: Option<&OpaqueCursor>,
        limit: usize,
    ) -> Result<Paged<Song>, Error> {
        self.query_active_songs_respecting_favorites(query, false, None, sort, cursor, limit)
    }

    /// The favorites-aware paginated query over the active root (task 6.2).
    /// `in_favorites` limits results to favorited songs so search can overlay
    /// on either the all-songs or the favorites view; `playlist` narrows to
    /// one playlist's members (mutually exclusive with `in_favorites`).
    #[allow(clippy::too_many_arguments)]
    pub fn query_active_songs_respecting_favorites(
        &self,
        query: &str,
        in_favorites: bool,
        playlist: Option<PlaylistId>,
        sort: SongSort,
        cursor: Option<&OpaqueCursor>,
        limit: usize,
    ) -> Result<Paged<Song>, Error> {
        if limit == 0 || limit > 500 {
            return Err(Error::validation(
                Subject::Query,
                "page limit",
                "must be 1 through 500",
            ));
        }
        let query = normalized_key(query);
        let cursor = cursor.cloned();
        self.with_reader(move |connection| {
            query_active(
                connection,
                &query,
                in_favorites,
                playlist,
                sort,
                cursor.as_ref(),
                limit,
            )
        })
    }

    /// Diagnostic snapshot of one scan run row (`scan_runs`), for tests and
    /// runtime status surfaces.
    pub fn scan_run_snapshot(
        &self,
        root: LibraryRootId,
        generation: u64,
    ) -> Result<Option<(ScanState, ScanProgress, bool)>, Error> {
        self.with_reader(move |connection| {
            connection
                .query_row(
                    "SELECT state, discovered_count, processed_count, created_count, updated_count, missing_count, skipped_count, failed_count, finished_at FROM scan_runs WHERE library_root_uuid = ?1 AND generation = ?2",
                    params![root.to_string(), i64::try_from(generation).unwrap_or(i64::MAX)],
                    |row| {
                        let state = scan_state_from_db(&row.get::<_, String>(0)?);
                        let progress = ScanProgress {
                            state,
                            discovered: u64::try_from(row.get::<_, i64>(1)?).unwrap_or_default(),
                            processed: u64::try_from(row.get::<_, i64>(2)?).unwrap_or_default(),
                            created: u64::try_from(row.get::<_, i64>(3)?).unwrap_or_default(),
                            updated: u64::try_from(row.get::<_, i64>(4)?).unwrap_or_default(),
                            missing: u64::try_from(row.get::<_, i64>(5)?).unwrap_or_default(),
                            skipped: u64::try_from(row.get::<_, i64>(6)?).unwrap_or_default(),
                            failed: u64::try_from(row.get::<_, i64>(7)?).unwrap_or_default(),
                        };
                        Ok((state, progress, row.get::<_, Option<i64>>(8)?.is_some()))
                    },
                )
                .optional()
                .map_err(storage)
        })
    }

    /// Map a stored issue code back to its stable `&'static str`.
    fn issue_code(code: &str) -> &'static str {
        match code {
            "unsupported_media" => "unsupported_media",
            "no_audio_track" => "no_audio_track",
            "corrupt_media" => "corrupt_media",
            "duplicate_content" => "duplicate_content",
            "tag_limit" => "tag_limit",
            _ => "scan_file_error",
        }
    }

    /// Diagnostic list of one run's issues (`scan_issues`).
    pub fn scan_issues(
        &self,
        root: LibraryRootId,
        generation: u64,
    ) -> Result<Vec<MediaDiagnostic>, Error> {
        self.with_reader(move |connection| {
            let run_key = format!("{root}:{generation}");
            let mut statement = connection
                .prepare(
                    "SELECT relative_path, code, detail FROM scan_issues WHERE scan_run_uuid = ?1 ORDER BY id",
                )
                .map_err(storage)?;
            let issues = statement
                .query_map(params![run_key], |row| {
                    let path = row.get::<_, String>(0)?;
                    let code = row.get::<_, String>(1)?;
                    let detail = row.get::<_, Option<String>>(2)?.unwrap_or_default();
                    Ok(MediaDiagnostic::new(
                        RelativeMediaPath::new(&path).map_err(to_sql_error)?,
                        Self::issue_code(&code),
                        detail,
                        false,
                    ))
                })
                .map_err(storage)?
                .collect::<rusqlite::Result<Vec<_>>>()
                .map_err(storage)?;
            Ok(issues)
        })
    }

    /// The active root's newest 100 available songs, using stable UUID ties.
    pub fn recent_songs(&self) -> Result<Vec<Song>, Error> {
        self.with_reader(|connection| {
            let root = active_root_id(connection)?.ok_or_else(|| Error::unavailable("library", "no active root"))?;
            let mut statement = connection
                .prepare(&format!("{} WHERE s.library_root_uuid = ?1 AND s.availability = 'available' ORDER BY s.added_at DESC, s.uuid DESC LIMIT {}", SONG_SELECT, RECENT_VIEW_LIMIT))
                .map_err(storage)?;
            let songs = statement
                .query_map(params![root.to_string()], song_from_row)
                .map_err(storage)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(storage)?;
            Ok(songs)
        })
    }

    pub(super) fn with_reader<T>(
        &self,
        read: impl FnOnce(&Connection) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let connection = self.readers.acquire()?;
        let result = read(&connection);
        match self.readers.release(connection) {
            Ok(()) => result,
            Err(release_error) => result.and(Err(release_error)),
        }
    }
}
