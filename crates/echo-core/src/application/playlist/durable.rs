//! Library-authoritative favorite and playlist commands. Planning uses stable
//! identities; the complete after-image is committed before any projection.

use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::application::library_commit::commit_then_project;
use crate::application::ports::{ControlPlanePort, PlaylistRepository, SongRepository, UnitOfWork};
use crate::domain::entities::{PlaylistMember, Song};
use crate::domain::ids::{LibraryRootId, OperationId, PlaylistId, Revision, SongId};
use crate::domain::library::durability::{ChangeOrigin, LibraryBatch};
use crate::domain::library::{
    next_hlc, DeviceId, HybridLogicalClock, PlaylistRecord, PortableRecord, PortableRecordEnvelope,
    RecordKind,
};
use crate::domain::text::{playlist_name_key, validate_playlist_name};
use crate::error::{Error, Subject};

use super::super::portable_materialize::{favorite_record, playlist_item_record, tombstone_record};

/// Dependencies shared by local logical edits, independent of desktop/SQLite.
pub struct DurableLibraryEdits<'a> {
    pub control: &'a dyn ControlPlanePort,
    pub songs: &'a dyn SongRepository,
    pub playlists: &'a dyn PlaylistRepository,
    pub unit: &'a dyn UnitOfWork,
    pub device: DeviceId,
}

impl DurableLibraryEdits<'_> {
    fn save<T>(
        &self,
        root: LibraryRootId,
        records: Vec<PortableRecord>,
        project: impl FnOnce() -> Result<T, Error>,
    ) -> Result<T, Error> {
        let manifest = self
            .control
            .read_manifest(root)?
            .ok_or_else(|| Error::unavailable("library", "library manifest missing"))?;
        let operation = uuid::Uuid::new_v4();
        let envelopes = records
            .into_iter()
            .map(|record| PortableRecordEnvelope::new(record, None))
            .collect::<Result<Vec<_>, _>>()?;
        let batch = LibraryBatch::new(
            operation,
            manifest.library_id,
            ChangeOrigin::LocalEdit,
            envelopes,
        );
        let (outcome, value) = commit_then_project(self.control, root, &batch, |_| project())?;
        value.ok_or_else(|| {
            let reason = Error::unavailable(
                "library",
                outcome
                    .reason
                    .unwrap_or_else(|| "saved operation awaits projection".to_owned()),
            );
            if outcome.status
                == crate::domain::library::durability::CommitStatus::SavedPendingRecovery
            {
                Error::saved_pending_recovery(OperationId::from_uuid(operation), reason)
            } else {
                Error::saved_pending_projection(OperationId::from_uuid(operation), reason)
            }
        })
    }

    fn stamp(
        &self,
        root: LibraryRootId,
        kind: RecordKind,
        id: uuid::Uuid,
    ) -> Result<(Revision, HybridLogicalClock), Error> {
        let existing = self.control.read_record(root, kind, &id.to_string())?;
        Ok((
            existing.as_ref().map_or(Revision::INITIAL, |record| {
                Revision::from_u64(record.revision().as_u64().saturating_add(1))
            }),
            next_hlc(
                existing
                    .as_ref()
                    .map_or(HybridLogicalClock::default(), PortableRecord::hlc),
                now_ms() / 1000,
            ),
        ))
    }

    fn ensure_playlist(&self, root: LibraryRootId, id: PlaylistId) -> Result<(), Error> {
        if !self.playlists.list(root)?.contains(&id) {
            return Err(Error::unavailable("playlist", "not in the active library"));
        }
        Ok(())
    }

    fn validate_name(
        &self,
        root: LibraryRootId,
        id: Option<PlaylistId>,
        name: &str,
    ) -> Result<(), Error> {
        validate_playlist_name(name)
            .map_err(|reason| Error::validation(Subject::Name, "name", reason))?;
        if self
            .playlists
            .by_name(root, &playlist_name_key(name))?
            .is_some_and(|found| Some(found) != id)
        {
            return Err(Error::conflict("playlist name already exists"));
        }
        Ok(())
    }

    /// Save both favorite values, including explicit cancellation.
    /// # Errors
    /// Invalid dependencies, durable storage and pending projection are typed.
    pub fn set_favorite(
        &self,
        root: LibraryRootId,
        song: SongId,
        favorite: bool,
    ) -> Result<Song, Error> {
        let current = self
            .songs
            .by_id(song)?
            .ok_or_else(|| Error::unavailable("song", "not in the library"))?;
        if current.root() != root || !current.availability().is_visible() {
            return Err(Error::unavailable(
                "song",
                "not visible in the active library",
            ));
        }
        let (revision, hlc) = self.stamp(root, RecordKind::Favorite, song.as_uuid())?;
        self.save(
            root,
            vec![PortableRecord::Favorite(favorite_record(
                self.device,
                hlc,
                revision,
                song,
                favorite,
            ))],
            || {
                self.songs.set_favorite(song, favorite)?;
                self.songs
                    .by_id(song)?
                    .ok_or_else(|| Error::unavailable("song", "projection disappeared"))
            },
        )
    }

    /// Persist an empty playlist definition with its original creation time.
    /// # Errors
    /// Invalid/colliding names, storage and pending projection propagate.
    pub fn create_playlist(&self, root: LibraryRootId, name: &str) -> Result<PlaylistId, Error> {
        let name = name.trim();
        self.validate_name(root, None, name)?;
        let id = PlaylistId::new();
        let (revision, hlc) = self.stamp(root, RecordKind::Playlist, id.as_uuid())?;
        let record = PortableRecord::Playlist(PlaylistRecord {
            playlist_uuid: id.as_uuid(),
            revision,
            hlc,
            updated_by_device_id: self.device,
            display_name: name.to_owned(),
            created_at: Some(now_ms()),
            cover_asset: None,
        });
        self.save(root, vec![record], || {
            self.playlists.create(id, root, name)?;
            Ok(id)
        })
    }

    fn playlist_record(
        &self,
        root: LibraryRootId,
        id: PlaylistId,
    ) -> Result<PlaylistRecord, Error> {
        self.ensure_playlist(root, id)?;
        let (revision, hlc) = self.stamp(root, RecordKind::Playlist, id.as_uuid())?;
        let mut record =
            match self
                .control
                .read_record(root, RecordKind::Playlist, &id.to_string())?
            {
                Some(PortableRecord::Playlist(record)) => record,
                _ => PlaylistRecord {
                    playlist_uuid: id.as_uuid(),
                    revision,
                    hlc,
                    updated_by_device_id: self.device,
                    display_name: self
                        .playlists
                        .name(id)?
                        .ok_or_else(|| Error::unavailable("playlist", "name missing"))?,
                    created_at: None,
                    cover_asset: None,
                },
            };
        record.revision = revision;
        record.hlc = hlc;
        record.updated_by_device_id = self.device;
        Ok(record)
    }

    /// Rename preserves original time, custom cover and member identities.
    /// # Errors
    /// Invalid targets, names, storage and pending projection propagate.
    pub fn rename_playlist(
        &self,
        root: LibraryRootId,
        id: PlaylistId,
        name: &str,
    ) -> Result<(), Error> {
        let name = name.trim();
        self.validate_name(root, Some(id), name)?;
        let mut record = self.playlist_record(root, id)?;
        record.display_name = name.to_owned();
        self.save(root, vec![PortableRecord::Playlist(record)], || {
            self.playlists.rename(id, name)
        })
    }

    /// Commit the playlist and every membership tombstone as one set.
    /// # Errors
    /// Unknown targets, storage and pending projection propagate.
    pub fn delete_playlist(&self, root: LibraryRootId, id: PlaylistId) -> Result<(), Error> {
        self.ensure_playlist(root, id)?;
        let mut records = Vec::new();
        for member in self.playlists.members(id)? {
            records.push(self.tombstone(root, RecordKind::PlaylistItem, member.id().as_uuid())?);
        }
        records.push(self.tombstone(root, RecordKind::Playlist, id.as_uuid())?);
        self.save(root, records, || self.playlists.delete(id))
    }

    fn tombstone(
        &self,
        root: LibraryRootId,
        kind: RecordKind,
        id: uuid::Uuid,
    ) -> Result<PortableRecord, Error> {
        let (revision, hlc) = self.stamp(root, kind, id)?;
        Ok(PortableRecord::Tombstone(tombstone_record(
            self.device,
            hlc,
            revision,
            id,
            kind,
        )))
    }

    /// Multi-target append mints each member identity before directory commit.
    /// # Errors
    /// Invalid root/dependency, order overflow, storage and pending projection.
    pub fn add_to_playlists(
        &self,
        root: LibraryRootId,
        song: SongId,
        targets: &[PlaylistId],
    ) -> Result<(), Error> {
        let current = self
            .songs
            .by_id(song)?
            .ok_or_else(|| Error::unavailable("song", "not in library"))?;
        if current.root() != root || !current.availability().is_playable() {
            return Err(Error::unavailable("song", "not playable in active library"));
        }
        let targets: BTreeSet<_> = targets.iter().copied().collect();
        let mut records = Vec::new();
        let mut members = Vec::new();
        for playlist in targets {
            self.ensure_playlist(root, playlist)?;
            let existing = self.playlists.members(playlist)?;
            if existing.iter().any(|member| member.song() == song) {
                continue;
            }
            let position = match existing.iter().map(PlaylistMember::position).max() {
                None => 0,
                Some(position) => position
                    .checked_add(1)
                    .ok_or_else(|| Error::conflict("playlist position exhausted"))?,
            };
            let member = PlaylistMember::new(playlist, song, position, current.availability());
            let (revision, hlc) =
                self.stamp(root, RecordKind::PlaylistItem, member.id().as_uuid())?;
            let mut record = playlist_item_record(
                self.device,
                hlc,
                revision,
                member.id(),
                playlist,
                song,
                position,
            );
            record.added_at = Some(now_ms());
            records.push(PortableRecord::PlaylistItem(record));
            members.push(member);
        }
        if records.is_empty() {
            return Ok(());
        }
        self.save(root, records, || {
            self.unit.with_tx(Box::new(move |tx| {
                for member in members {
                    tx.insert_member(&member)?;
                }
                Ok(())
            }))
        })
    }

    /// Removing an absent membership remains a no-op.
    /// # Errors
    /// Invalid root/dependency, storage and pending projection propagate.
    pub fn remove_playlist_song(
        &self,
        root: LibraryRootId,
        playlist: PlaylistId,
        song: SongId,
    ) -> Result<(), Error> {
        self.ensure_playlist(root, playlist)?;
        let Some(member) = self
            .playlists
            .members(playlist)?
            .into_iter()
            .find(|member| member.song() == song)
        else {
            return Ok(());
        };
        let record = self.tombstone(root, RecordKind::PlaylistItem, member.id().as_uuid())?;
        self.save(root, vec![record], || {
            self.playlists.remove_member(playlist, song)
        })
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |value| {
            u64::try_from(value.as_millis()).unwrap_or(u64::MAX)
        })
}
