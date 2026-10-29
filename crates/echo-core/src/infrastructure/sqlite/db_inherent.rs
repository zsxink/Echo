//! Inherent `SqliteDatabase` methods: open/maintenance/playback/scan-run/queries.

#[allow(clippy::wildcard_imports)]
use super::*;

use std::sync::Arc;

impl SqliteDatabase {
    /// Open (or create) a database, configure safety pragmas, apply migrations
    /// and verify its quick integrity check.
    ///
    /// The database's parent directory is created when absent: SQLite's
    /// `Connection::open` refuses to create parent directories, so a cold start
    /// on a fresh machine — where the platform app-data directory does not yet
    /// exist — would otherwise fail before the first migration.
    ///
    /// An existing file that cannot be opened as a SQLite database — unreadable
    /// header, failed `quick_check`, damaged pages — is quarantined beside the
    /// original and replaced by a fresh database instead of aborting startup.
    /// Quarantine is a byte-for-byte move: the damaged file is never read for
    /// business data, never repaired in place and never deleted, so diagnosis
    /// stays possible (design: 「SQLite 损坏时先隔离再重建」). Every other
    /// failure (permission denied, disk full, a lock held elsewhere, a migration
    /// checksum mismatch) is *not* damage and still fails the open, because
    /// renaming a healthy database aside would discard a usable projection.
    ///
    /// The rebuilt projection is empty: the directory is the source of truth,
    /// so re-selecting the library folder re-derives the same root identity and
    /// replays the committed facts (see `rebuild_library_projection`). The
    /// quarantined file is available through [`Self::quarantined_database`].
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
        match Self::open_healthy(&path, had_database) {
            Ok(database) => Ok(database),
            Err(failure) if had_database && is_database_damage(&failure) => {
                let quarantined = quarantine_database(&path, &failure)?;
                tracing::warn!(
                    quarantined = %crate::logging::redact_path(&quarantined),
                    failure = %failure.to_log(crate::logging::DiagnosticMode::default()),
                    "damaged SQLite database quarantined; starting from a fresh projection"
                );
                let mut database = Self::open_healthy(&path, false)?;
                database.quarantined = Some(quarantined);
                Ok(database)
            }
            Err(failure) => Err(failure),
        }
    }

    /// The healthy-open body: configure, back up, migrate, verify, then start
    /// the writer actor and the reader pool. Kept separate from [`Self::open`]
    /// so the quarantine decision is made on the failure, not inside it.
    fn open_healthy(path: &Path, had_database: bool) -> Result<Self, Error> {
        let mut writer = open_writer(path)?;
        if had_database {
            let backup = backup_path(path);
            backup_connection(&writer, &backup)?;
        }
        apply_migrations(&mut writer)?;
        quick_check_connection(&writer)?;
        // Load/mint the device identity on the writer connection before any
        // reader is handed out, so every portable record sees it.
        let device_id = device_state::load_or_create_device_id(&writer)?;

        let mut readers = Vec::with_capacity(reader_count());
        for _ in 0..reader_count() {
            readers.push(open_reader(path)?);
        }
        Ok(Self {
            library_control: Mutex::new(None),
            library_projection_workers: Mutex::new(BTreeMap::new()),
            path: path.to_path_buf(),
            writer: SqliteWriteActor::spawn(writer),
            readers: ReaderPool::new(readers),
            device_id,
            quarantined: None,
            projection_notify: Mutex::new(None),
        })
    }

    /// The damaged database this open quarantined, if any. It is diagnostic
    /// material only: nothing in the runtime reads it, and it is never removed
    /// automatically.
    #[must_use]
    pub fn quarantined_database(&self) -> Option<&Path> {
        self.quarantined.as_deref()
    }

    /// Queue an asynchronous, directory-authoritative projection pass.
    ///
    /// The durable journal remains the source of truth. The worker owns a
    /// separate SQLite writer connection and coalesces repeated wakes for the
    /// same root, so local acknowledgements never wait for read-model replay.
    pub(crate) fn library_projection_worker(
        &self,
        root: LibraryRootId,
    ) -> Result<Arc<crate::application::library_projection_worker::LibraryProjectionWorker>, Error>
    {
        use crate::application::library_projection_worker::LibraryProjectionWorker;

        let control = self.library_control().ok_or_else(|| {
            Error::unavailable(
                "library projection",
                "directory authority is not configured",
            )
        })?;
        let worker = {
            let mut workers =
                self.library_projection_workers
                    .lock()
                    .map_err(|_| Error::InvariantViolation {
                        why: "library projection worker registry poisoned".to_owned(),
                    })?;
            if let Some(worker) = workers.get(&root) {
                Arc::clone(worker)
            } else {
                let writer = self.writer.clone();
                let notify = self.projection_notify.lock().unwrap().clone();
                let worker = Arc::new(LibraryProjectionWorker::spawn(root, move |root| {
                    let durable = control.durability().ok_or_else(|| {
                        Error::unavailable(
                            "library projection",
                            "directory commit adapter is not configured",
                        )
                    })?;
                    // Projection reads the authenticated journal without
                    // materializing files or contending on the writer lock.
                    let replay = durable.projection_replay(root)?;
                    let projected = writer.run(move |connection| {
                        durability_projection::project_commits_unless_superseded(
                            connection, root, &replay,
                        )
                    });
                    if projected.is_ok() {
                        if let Some(notify) = &notify {
                            notify(root);
                        }
                    }
                    projected
                })?);
                workers.insert(root, Arc::clone(&worker));
                worker
            }
        };
        Ok(worker)
    }

    /// Register a callback invoked after a root's projection worker completes a
    /// catch-up pass. The desktop runtime wires this to a Tauri event so the UI
    /// re-pulls the catalog read model (playlists / songs / counts) that changed
    /// out of band from the originating command — e.g. a playlist created from
    /// the shell before the projector had caught up. Tests leave it unset.
    pub fn set_projection_notifier(
        &self,
        notifier: Arc<dyn Fn(LibraryRootId) + Send + Sync>,
    ) {
        *self.projection_notify.lock().unwrap() = Some(notifier);
    }

    /// Wait until all projection wakes already accepted for this root finish.
    /// Intended for orderly shutdown, diagnostics, and deterministic recovery
    /// boundaries; interactive writes do not call it.
    pub fn flush_library_projection(
        &self,
        root: LibraryRootId,
        timeout: std::time::Duration,
    ) -> Result<(), Error> {
        let worker = self
            .library_projection_workers
            .lock()
            .map_err(|_| Error::InvariantViolation {
                why: "library projection worker registry poisoned".to_owned(),
            })?
            .get(&root)
            .cloned();
        if let Some(worker) = worker {
            worker.flush(timeout)?;
        }
        Ok(())
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
