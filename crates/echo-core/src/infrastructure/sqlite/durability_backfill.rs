//! Typed, root-scoped legacy rescue. Physical SQLite snapshots stay beside the
//! application database; only allowlisted domain records enter the library.

use super::{support::storage, SqliteDatabase};
use crate::application::ports::{ControlPlanePort, LibraryHistoryPort};
use crate::domain::ids::{LibraryRootId, Revision};
use crate::domain::library::{
    DeviceId, FavoriteRecord, HybridLogicalClock, OverrideRecord, PlayStatsRecord,
    PlaylistItemRecord, PlaylistRecord, PortableRecord, PortableRecordEnvelope,
};
use crate::error::Error;
use rusqlite::{params, Connection, OptionalExtension};
use std::sync::Arc;

impl SqliteDatabase {
    /// Attach the library authority before starting any user or sync writes.
    pub fn configure_library_control(&self, control: Arc<dyn ControlPlanePort>) {
        *self
            .library_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(control);
    }

    pub(crate) fn library_control(&self) -> Option<Arc<dyn ControlPlanePort>> {
        self.library_control
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

impl LibraryHistoryPort for SqliteDatabase {
    fn projection_status(&self, root: LibraryRootId) -> Result<crate::domain::library::durability::LibraryProjectionStatus, Error> {
        self.with_reader(move |connection| super::durability_projection::status(connection, root))
    }
    fn read_recovery_records(&self, root: LibraryRootId) -> Result<crate::domain::library::durability::LibraryRecoveryRecords, Error> {
        self.with_reader(move |connection| super::durability_projection::read(connection, root))
    }
    fn capture_library_history(
        &self,
        root: LibraryRootId,
    ) -> Result<Vec<PortableRecordEnvelope>, Error> {
        let backup = self.path.with_extension(format!("rescue-{root}.sqlite"));
        // Exclusive publication prevents a later open from overwriting the only
        // pre-rescue evidence. SQLite's backup API includes the WAL contents.
        if !backup.exists() {
            let temporary = self
                .path
                .with_extension(format!("rescue-{}.tmp", uuid::Uuid::new_v4()));
            self.backup_to(&temporary)?;
            match std::fs::hard_link(&temporary, &backup) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                Err(error) => {
                    return Err(Error::io("publish private rescue snapshot", error, &backup))
                }
            }
            std::fs::remove_file(&temporary)
                .map_err(|error| Error::io("remove rescue temporary", error, &temporary))?;
        }
        self.writer.run(move |connection| {
            let transaction = connection.transaction().map_err(storage)?;
            let records = export_library_records(&transaction, root)?;
            transaction.commit().map_err(storage)?;
            Ok(records)
        })
    }

    fn project_library_commits(
        &self,
        root: LibraryRootId,
        commits: &crate::domain::library::durability::LibraryReplay,
    ) -> Result<(), Error> {
        let commits = commits.clone();
        let control = self.library_control();
        self.writer.run(move |connection| {
            if let Some(image) = &commits.checkpoint {
                let current: Option<(u64, Option<String>)> = connection.query_row(
                    "SELECT commit_seq,commit_digest FROM library_projection_watermarks WHERE library_root_uuid=?1",
                    [root.to_string()], |row| Ok((row.get(0)?, row.get(1)?))).optional().map_err(storage)?;
                if let Some((seq, digest)) = current.filter(|(seq, _)| *seq < image.library_commit_seq) {
                    let operation: String = connection.query_row(
                        "SELECT operation_uuid FROM library_applied_operations WHERE library_root_uuid=?1 AND commit_seq=?2",
                        params![root.to_string(),seq], |row| row.get(0)).map_err(storage)?;
                    let durable = control.as_ref().and_then(|c| c.durability()).ok_or_else(|| Error::conflict("covered watermark requires directory operation proof"))?;
                    let original = durable.read_operation(root, uuid::Uuid::parse_str(&operation).map_err(storage)?)?
                        .ok_or_else(|| Error::conflict("covered database watermark has no operation receipt"))?;
                    if original.library_commit_seq != seq || Some(original.digest) != digest {
                        return Err(Error::conflict("covered database watermark differs from operation receipt"));
                    }
                }
            }
            super::durability_projection::project_commits(connection, root, &commits)
        })
    }
}

/// Export from one connection/transaction so caller-controlled writer
/// serialization establishes a consistent capture. No SQL/configuration is
/// copied and no confirmation metadata is inferred from an effective value.
pub(crate) fn export_library_records(
    connection: &Connection,
    root: LibraryRootId,
) -> Result<Vec<PortableRecordEnvelope>, Error> {
    let raw_device: String = connection
        .query_row(
            "SELECT value FROM device_state WHERE key='device_id'",
            [],
            |row| row.get(0),
        )
        .map_err(storage)?;
    let device = DeviceId::from_uuid(uuid::Uuid::parse_str(&raw_device).map_err(storage)?);
    let mut records = Vec::new();
    for song in super::all_songs_in_root(connection, root)? {
        let (wall, counter): (u64, u32) = connection
            .query_row(
                "SELECT hlc_wall_secs,hlc_counter FROM songs WHERE uuid=?1",
                [song.id().to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .map_err(storage)?;
        let hlc = HybridLogicalClock::new(wall, counter);
        push(
            &mut records,
            PortableRecord::Song(crate::application::portable_materialize::song_record(
                device,
                hlc,
                song.revision(),
                &song,
            )?),
        )?;
        push(
            &mut records,
            PortableRecord::Favorite(FavoriteRecord {
                song_uuid: song.id().as_uuid(),
                revision: song.revision(),
                updated_by_device_id: device,
                hlc,
                is_favorite: song.favorite(),
            }),
        )?;
        let by_device = super::read_device_play_counts(connection, song.id())?;
        push(
            &mut records,
            PortableRecord::PlayStats(PlayStatsRecord {
                song_uuid: song.id().as_uuid(),
                revision: song.revision(),
                updated_by_device_id: device,
                hlc,
                by_device,
            }),
        )?;
    }
    let mut statement=connection.prepare("SELECT uuid,display_name,created_at,revision,hlc_wall_secs,hlc_counter FROM playlists WHERE library_root_uuid=?1 ORDER BY uuid").map_err(storage)?;
    let rows = statement
        .query_map([root.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, u64>(2)?,
                row.get::<_, u64>(3)?,
                row.get::<_, u64>(4)?,
                row.get::<_, u32>(5)?,
            ))
        })
        .map_err(storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage)?;
    for (id, name, created_at, revision, wall, counter) in rows {
        push(
            &mut records,
            PortableRecord::Playlist(PlaylistRecord {
                playlist_uuid: uuid::Uuid::parse_str(&id).map_err(storage)?,
                display_name: name,
                created_at: Some(created_at),
                cover_asset: None,
                revision: Revision::from_u64(revision),
                updated_by_device_id: device,
                hlc: HybridLogicalClock::new(wall, counter),
            }),
        )?;
    }
    let mut statement=connection.prepare("SELECT ps.member_uuid,ps.playlist_uuid,ps.song_uuid,ps.position,ps.added_at,ps.member_revision FROM playlist_songs ps JOIN playlists p ON p.uuid=ps.playlist_uuid WHERE p.library_root_uuid=?1 ORDER BY ps.playlist_uuid,ps.position").map_err(storage)?;
    let rows = statement
        .query_map([root.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, u64>(3)?,
                row.get::<_, u64>(4)?,
                row.get::<_, u64>(5)?,
            ))
        })
        .map_err(storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage)?;
    for (id, playlist, song, position, added_at, revision) in rows {
        push(
            &mut records,
            PortableRecord::PlaylistItem(PlaylistItemRecord {
                item_uuid: uuid::Uuid::parse_str(&id).map_err(storage)?,
                playlist_uuid: uuid::Uuid::parse_str(&playlist).map_err(storage)?,
                song_uuid: uuid::Uuid::parse_str(&song).map_err(storage)?,
                position,
                added_at: Some(added_at),
                revision: Revision::from_u64(revision),
                updated_by_device_id: device,
                hlc: HybridLogicalClock::default(),
            }),
        )?;
    }
    export_overrides(connection, root, device, &mut records)?;
    Ok(records)
}

fn export_overrides(
    connection: &Connection,
    root: LibraryRootId,
    device: DeviceId,
    records: &mut Vec<PortableRecordEnvelope>,
) -> Result<(), Error> {
    let mut statement=connection.prepare("SELECT o.song_uuid,o.title,o.artist,o.album,o.lyrics_text,o.revision,o.hlc_wall_secs,o.hlc_counter FROM song_overrides o JOIN songs s ON s.uuid=o.song_uuid WHERE s.library_root_uuid=?1 ORDER BY o.song_uuid").map_err(storage)?;
    let rows = statement
        .query_map(params![root.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, u64>(5)?,
                row.get::<_, u64>(6)?,
                row.get::<_, u32>(7)?,
            ))
        })
        .map_err(storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage)?;
    for (id, title, artist, album, lyrics_text, revision, wall, counter) in rows {
        push(
            records,
            PortableRecord::Override(OverrideRecord {
                song_uuid: uuid::Uuid::parse_str(&id).map_err(storage)?,
                title,
                artist,
                album,
                lyrics_text,
                revision: Revision::from_u64(revision),
                updated_by_device_id: device,
                hlc: HybridLogicalClock::new(wall, counter),
            }),
        )?;
    }
    Ok(())
}
fn push(records: &mut Vec<PortableRecordEnvelope>, record: PortableRecord) -> Result<(), Error> {
    records.push(PortableRecordEnvelope::new(record, None)?);
    Ok(())
}

/// Local playback event identities are recovery state, never shared statistics.
pub(crate) fn export_playback_events(
    connection: &Connection,
    root: LibraryRootId,
) -> Result<Vec<crate::domain::library::durability::DurableStateChange>, Error> {
    use crate::domain::library::durability::{DurableStateChange, PlaybackEvent, StateKind};
    let mut statement = connection.prepare("SELECT r.playback_session_uuid,r.song_uuid,r.recorded_at FROM recorded_play_sessions r JOIN songs s ON s.uuid=r.song_uuid WHERE s.library_root_uuid=?1 ORDER BY r.playback_session_uuid").map_err(storage)?;
    let rows = statement
        .query_map([root.to_string()], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, u64>(2)?,
            ))
        })
        .map_err(storage)?;
    rows.map(|row| {
        let (session, song, recorded_at) = row.map_err(storage)?;
        let event = PlaybackEvent {
            song_uuid: uuid::Uuid::parse_str(&song).map_err(storage)?,
            recorded_at,
        };
        Ok(DurableStateChange::new(
            StateKind::PlaybackEvent,
            uuid::Uuid::parse_str(&session).map_err(storage)?,
            serde_json::to_value(event).map_err(storage)?,
        ))
    })
    .collect()
}
