//! Plan on the writer connection, roll back, then project only saved facts.
mod artist_cover;
mod cover;
mod deletion;
mod members;
mod playback;
mod removal;
use super::{durability_backfill, durability_projection, statements, storage, SqliteDatabase};
use crate::application::library_commit::project_committed;
use crate::application::ports::{ControlPlanePort, LocalLibraryEditPort};
use crate::domain::ids::{LibraryRootId, OperationId, PlaylistId, Revision, SongId};
use crate::domain::library::durability::{ChangeOrigin, LibraryBatch, LibraryReplay, StateKind};
use crate::domain::library::{
    DeviceId, FavoriteRecord, HybridLogicalClock, LibraryId, PlaylistRecord, PortableRecord,
    PortableRecordEnvelope, RecordKind,
};
use crate::domain::sync::ObjectKey;
use crate::domain::text::playlist_name_key;
use crate::error::Error;
use rusqlite::{Connection, OptionalExtension};

enum PlaylistEdit {
    Create,
    Rename,
}

impl LocalLibraryEditPort for SqliteDatabase {
    fn remove_library_member(
        &self,
        control: &dyn ControlPlanePort,
        root: LibraryRootId,
        playlist: PlaylistId,
        song: SongId,
        device: DeviceId,
    ) -> Result<(), Error> {
        self.plan_library_edit(
            control,
            root,
            device,
            true,
            move |c, authority, library, sequence, baseline| {
                removal::Removal {
                    root,
                    playlist,
                    song,
                    device,
                    library,
                    sequence,
                }
                .plan(c, authority, baseline)
            },
        )
    }

    fn append_library_members(
        &self,
        control: &dyn ControlPlanePort,
        root: LibraryRootId,
        song: SongId,
        targets: &[PlaylistId],
        device: DeviceId,
    ) -> Result<(), Error> {
        let targets = targets.to_vec();
        self.plan_library_edit(
            control,
            root,
            device,
            true,
            move |c, authority, library, sequence, baseline| {
                members::Append {
                    root,
                    song,
                    targets,
                    device,
                    library,
                    sequence,
                }
                .plan(c, authority, baseline)
            },
        )
    }

    fn delete_library_playlist(
        &self,
        control: &dyn ControlPlanePort,
        root: LibraryRootId,
        id: PlaylistId,
        device: DeviceId,
    ) -> Result<(), Error> {
        self.plan_library_edit(
            control,
            root,
            device,
            false,
            move |c, _, library, sequence, _| {
                deletion::plan(c, root, library, id, device, sequence)
            },
        )
    }

    fn set_library_favorite(
        &self,
        control: &dyn ControlPlanePort,
        root: LibraryRootId,
        song: SongId,
        favorite: bool,
        device: DeviceId,
    ) -> Result<(), Error> {
        self.plan_library_edit(control, root, device, true, move |c, _, library, sequence, _| {
            let visible: bool = c.query_row(
                "SELECT EXISTS(SELECT 1 FROM songs WHERE uuid=?1 AND library_root_uuid=?2 AND availability<>'pending_delete')",
                rusqlite::params![song.to_string(), root.to_string()], |row| row.get(0),
            ).map_err(storage)?;
            if !visible {
                return Err(Error::unavailable("song", "not visible in the active library"));
            }
            statements::set_song_favorite(c, song, favorite)?;
            let (revision, wall, counter, favorited_at) = c.query_row(
                "SELECT h.local_revision,s.hlc_wall_secs,s.hlc_counter,s.favorited_at FROM songs s JOIN sync_local_highwater h ON h.object_type='favorite' AND h.object_uuid=s.uuid WHERE s.uuid=?1",
                [song.to_string()], |row| Ok((row.get::<_, u64>(0)?, row.get::<_, u64>(1)?, row.get::<_, u32>(2)?, row.get::<_, Option<u64>>(3)?)),
            ).map_err(storage)?;
            let record = PortableRecord::Favorite(FavoriteRecord {
                song_uuid: song.as_uuid(), is_favorite: favorite, favorited_at,
                revision: Revision::from_u64(revision), hlc: HybridLogicalClock::new(wall, counter),
                updated_by_device_id: device,
            });
            planned_batch(c, root, library, device, record, ObjectKey::Favorite(song), sequence)
        })
    }

    fn create_library_playlist(
        &self,
        control: &dyn ControlPlanePort,
        root: LibraryRootId,
        id: PlaylistId,
        name: &str,
        device: DeviceId,
    ) -> Result<PlaylistId, Error> {
        self.edit_library_playlist(control, root, id, name, device, PlaylistEdit::Create)
    }

    fn rename_library_playlist(
        &self,
        control: &dyn ControlPlanePort,
        root: LibraryRootId,
        id: PlaylistId,
        name: &str,
        device: DeviceId,
    ) -> Result<(), Error> {
        self.edit_library_playlist(control, root, id, name, device, PlaylistEdit::Rename)
            .map(|_| ())
    }
}

impl SqliteDatabase {
    fn edit_library_playlist(
        &self,
        control: &dyn ControlPlanePort,
        root: LibraryRootId,
        id: PlaylistId,
        name: &str,
        device: DeviceId,
        edit: PlaylistEdit,
    ) -> Result<PlaylistId, Error> {
        let name = name.to_owned();
        let async_projection = matches!(&edit, PlaylistEdit::Create);
        self.plan_library_change(
            control,
            root,
            device,
            None,
            async_projection,
            move |c, authority, library, sequence, baseline| {
                let saved = if matches!(edit, PlaylistEdit::Rename) {
                    authority.read_record(root, RecordKind::Playlist, &id.to_string())?
                } else {
                    None
                };
                match edit {
                    PlaylistEdit::Create => {
                        // A creation already saved in the directory under the
                        // same unique name is that same playlist: complete it
                        // instead of minting a second identity (LD-08).
                        if let Some(live) = baseline.playlist_by_name(&playlist_name_key(&name)) {
                            let saved = PlaylistId::from_uuid(live.playlist_uuid);
                            return Ok((
                                LibraryBatch::new(
                                    uuid::Uuid::new_v4(),
                                    library,
                                    ChangeOrigin::LocalEdit,
                                    vec![],
                                ),
                                saved,
                            ));
                        }
                        statements::create_playlist(c, id, root, &name)?;
                    }
                    PlaylistEdit::Rename => {
                        let belongs: bool = c.query_row(
                            "SELECT EXISTS(SELECT 1 FROM playlists WHERE uuid=?1 AND library_root_uuid=?2)",
                            rusqlite::params![id.to_string(), root.to_string()], |row| row.get(0),
                        ).map_err(storage)?;
                        if !belongs {
                            return Err(Error::unavailable("playlist", "not in the active library"));
                        }
                        statements::rename_playlist(c, id, &name)?;
                    }
                }
                let record = planned_playlist(c, id, device, saved.as_ref())?;
                planned_batch(c, root, library, device, record, ObjectKey::Playlist(id), sequence)
                    .map(|batch| (batch, id))
            },
        )
    }

    fn plan_library_edit(
        &self,
        control: &dyn ControlPlanePort,
        root: LibraryRootId,
        device: DeviceId,
        async_projection: bool,
        plan: impl FnOnce(
                &Connection,
                &dyn ControlPlanePort,
                LibraryId,
                u64,
                &LibraryReplay,
            ) -> Result<LibraryBatch, Error>
            + Send
            + 'static,
    ) -> Result<(), Error> {
        self.plan_library_change(
            control,
            root,
            device,
            None,
            async_projection,
            move |c, authority, library, sequence, baseline| {
                plan(c, authority, library, sequence, baseline).map(|batch| (batch, ()))
            },
        )
    }

    #[allow(clippy::too_many_lines)]
    fn plan_library_change<T: Send + 'static>(
        &self,
        control: &dyn ControlPlanePort,
        root: LibraryRootId,
        device: DeviceId,
        retry_operation: Option<uuid::Uuid>,
        requested_async_projection: bool,
        plan: impl FnOnce(
                &Connection,
                &dyn ControlPlanePort,
                LibraryId,
                u64,
                &LibraryReplay,
            ) -> Result<(LibraryBatch, T), Error>
            + Send
            + 'static,
    ) -> Result<T, Error> {
        // Keep legacy unit fixtures deterministic; the desktop integration
        // benchmark exercises the production asynchronous path without this
        // crate-level unit-test configuration.
        let async_projection = !cfg!(test) && requested_async_projection;

        let authority = self.library_control().ok_or_else(|| {
            Error::unavailable("library edits", "directory authority is not configured")
        })?;
        let requested = control.read_manifest(root)?;
        let configured = authority.read_manifest(root)?;
        if device != self.device_id
            || requested.is_none()
            || requested.as_ref().map(|manifest| manifest.library_id)
                != configured.as_ref().map(|manifest| manifest.library_id)
        {
            return Err(Error::conflict("local edit authority or device differs"));
        }
        let projection_worker = async_projection
            .then(|| self.library_projection_worker(root))
            .transpose()?;
        let writer_wait_started = std::time::Instant::now();
        self.writer.run(move |connection| {
            trace_local_edit_stage(root, "writer_wait_and_setup", writer_wait_started);
            let stage_started = std::time::Instant::now();
            let durable = authority.durability().ok_or_else(|| {
                Error::unavailable("library edits", "directory commit adapter is not configured")
            })?;
            // Keep the directory writer guard through planning, so the same
            // verified recovery can authorize publication without a second
            // whole-library read or a gap in which another writer can commit.
            let mut session = durable.begin_write(root)?;
            trace_local_edit_stage(root, "begin_write", stage_started);
            let stage_started = std::time::Instant::now();
            let baseline = match &session {
                Some(session) => session.replay().clone(),
                None => durable.recover(root)?,
            };
            if !durability_projection::is_current(connection, root, &baseline)? {
                durability_backfill::verify_covered_watermark(
                    connection,
                    root,
                    &baseline,
                    Some(authority.as_ref()),
                )?;
                let projection = if async_projection {
                    durability_projection::project_verified_suffix(connection, root, &baseline)
                } else {
                    durability_projection::project_commits(connection, root, &baseline)
                };
                projection.map_err(|error| {
                    saved_pending_catch_up(connection, root, &baseline, retry_operation, error)
                })?;
            }
            trace_local_edit_stage(root, "sqlite_catch_up", stage_started);
            let manifest = authority.read_manifest(root)?
                .ok_or_else(|| Error::unavailable("library edits", "library manifest missing"))?;
            let stage_started = std::time::Instant::now();
            let tx = connection.transaction().map_err(storage)?;
            let sequence = tx.query_row(
                "SELECT origin_seq FROM sync_local_origin_cursors WHERE local_root_uuid=?1 AND origin_device_id=?2",
                rusqlite::params![root.to_string(), device.to_string()], |row| row.get::<_, u64>(0),
            ).optional().map_err(storage)?.unwrap_or(0);
            let (batch, result) = plan(&tx, authority.as_ref(), manifest.library_id, sequence, &baseline)?;
            tx.rollback().map_err(storage)?;
            trace_local_edit_stage(root, "plan", stage_started);
            if batch.records.is_empty() && batch.state.is_empty() && batch.resources.is_empty() {
                return Ok(result);
            }
            batch.validate()?;
            let stage_started = std::time::Instant::now();
            let outcome = match &mut session {
                Some(session) => session.commit(&batch)?,
                None => durable.commit(root, &batch)?,
            };
            trace_local_edit_stage(root, "directory_commit", stage_started);
            drop(session);
            if let Some(projection_worker) = projection_worker {
                // The folder journal has acknowledged the favorite/playlist
                // edit. SQL is a rebuildable read model and must not extend this
                // interaction's critical path. Failure to signal is recovered
                // from the directory at startup or before a later ordered edit.
                let _ = projection_worker.wake();
            } else {
                let stage_started = std::time::Instant::now();
                project_committed(authority.as_ref(), root, &batch, outcome, |committed| {
                    let extends_baseline = baseline.head().map_or_else(
                        || {
                            committed.library_commit_seq == 1
                                && committed.predecessor_digest.is_none()
                        },
                        |(sequence, digest)| {
                            sequence.checked_add(1) == Some(committed.library_commit_seq)
                                && committed.predecessor_digest.as_deref() == Some(digest)
                        },
                    );
                    let replay = if extends_baseline {
                        let mut replay = baseline;
                        replay.commits.push(committed.clone());
                        replay
                    } else {
                        durable.recover(root)?
                    };
                    durability_backfill::verify_covered_watermark(
                        connection,
                        root,
                        &replay,
                        Some(authority.as_ref()),
                    )?;
                    durability_projection::project_commits(connection, root, &replay)
                })?;
                trace_local_edit_stage(root, "sqlite_project_commit", stage_started);
            }
            Ok(result)
        })
    }
}

fn trace_local_edit_stage(root: LibraryRootId, stage: &str, started: std::time::Instant) {
    if std::env::var_os("ECHO_EDIT_TRACE").is_some() {
        eprintln!(
            "ECHO_EDIT_TRACE root={root} stage={stage} elapsed_ms={:.3}",
            started.elapsed().as_secs_f64() * 1_000.0
        );
    }
}

/// Attribute a failed ordered catch-up to the stable operation identity of the
/// first commit the projection has not recorded, so every retry keeps reporting
/// the saved operation instead of a fresh unsaved failure (LD-08).
fn saved_pending_catch_up(
    connection: &Connection,
    root: LibraryRootId,
    baseline: &LibraryReplay,
    retry_operation: Option<uuid::Uuid>,
    error: Error,
) -> Error {
    if let Some(operation) = retry_operation.filter(|operation| {
        baseline
            .states()
            .any(|state| state.kind == StateKind::PlaybackEvent && state.id == *operation)
    }) {
        return Error::saved_pending_projection(OperationId::from_uuid(operation), error);
    }
    let projected: Option<u64> = connection
        .query_row(
            "SELECT commit_seq FROM library_projection_watermarks WHERE library_root_uuid=?1",
            [root.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage)
        .unwrap_or(None);
    match baseline
        .commits
        .iter()
        .find(|commit| commit.library_commit_seq > projected.unwrap_or(0))
    {
        Some(commit) => Error::saved_pending_projection(
            OperationId::from_uuid(commit.batch.operation_id),
            error,
        ),
        None => error,
    }
}

fn planned_batch(
    c: &Connection,
    root: LibraryRootId,
    library: LibraryId,
    device: DeviceId,
    record: PortableRecord,
    object: ObjectKey,
    sequence: u64,
) -> Result<LibraryBatch, Error> {
    let mut batch = LibraryBatch::new(
        uuid::Uuid::new_v4(),
        library,
        ChangeOrigin::LocalEdit,
        vec![PortableRecordEnvelope::new(record, None)?],
    );
    batch.state = super::sync::durability::capture_local_object_mutation(
        c,
        root,
        object,
        library,
        crate::domain::sync::DeviceId::from_uuid(device.as_uuid()),
        sequence,
    )?;
    Ok(batch)
}

fn planned_playlist(
    c: &Connection,
    id: PlaylistId,
    device: DeviceId,
    saved: Option<&PortableRecord>,
) -> Result<PortableRecord, Error> {
    let (name, created_at, revision, wall, counter) = c.query_row(
        "SELECT display_name,created_at,revision,hlc_wall_secs,hlc_counter FROM playlists WHERE uuid=?1",
        [id.to_string()], |row| Ok((row.get::<_, String>(0)?,row.get::<_, u64>(1)?,row.get::<_, u64>(2)?,row.get::<_, u64>(3)?,row.get::<_, u32>(4)?)),
    ).map_err(storage)?;
    let (created_at, cover_asset) = match saved {
        Some(PortableRecord::Playlist(record)) => (record.created_at, record.cover_asset.clone()),
        _ => (Some(created_at), None),
    };
    Ok(PortableRecord::Playlist(PlaylistRecord {
        playlist_uuid: id.as_uuid(),
        display_name: name,
        created_at,
        cover_asset,
        revision: Revision::from_u64(revision),
        hlc: HybridLogicalClock::new(wall, counter),
        updated_by_device_id: device,
    }))
}

fn planned_record(
    authority: &dyn ControlPlanePort,
    root: LibraryRootId,
    kind: RecordKind,
    id: uuid::Uuid,
    replay: &LibraryReplay,
) -> Result<Option<PortableRecord>, Error> {
    let mut latest = None;
    let mut observe = |record: &PortableRecord| {
        let record_id = match record {
            PortableRecord::Playlist(record) if kind == RecordKind::Playlist => {
                Some(record.playlist_uuid)
            }
            PortableRecord::PlaylistItem(record) if kind == RecordKind::PlaylistItem => {
                Some(record.item_uuid)
            }
            PortableRecord::Tombstone(record) if record.deleted_kind == kind => {
                if record.object_uuid == id {
                    latest = Some(None);
                }
                return;
            }
            _ => None,
        };
        if record_id == Some(id) {
            latest = Some(Some(record.clone()));
        }
    };
    if let Some(checkpoint) = &replay.checkpoint {
        for envelope in &checkpoint.records {
            observe(envelope.record());
        }
    }
    for commit in &replay.commits {
        for envelope in &commit.batch.records {
            observe(envelope.record());
        }
    }
    match latest {
        Some(record) => Ok(record),
        None => authority.read_record(root, kind, &id.to_string()),
    }
}
