//! Pure validation of a proposed change against verified directory facts.
use super::{Error, LibraryBatch, LibraryCheckpoint, PortableSerialize};
use crate::domain::ids::{PlaylistId, SongId};
use crate::domain::library::{PortableRecord, PortableRecordEnvelope, RecordKind};
use crate::domain::sync::ObjectKey;
use std::collections::{HashMap, HashSet};

impl LibraryBatch {
    /// Validate preconditions and the resulting dependency graph against a
    /// verified directory image, never against SQLite or a live sync binding.
    /// Historical dangling records may still be read with `validate`; a new
    /// change must not introduce a dangling relationship or delete its parent.
    ///
    /// # Errors
    /// Rejects another local library, stale/ambiguous typed versions, duplicate
    /// targets, incomplete object dependencies and undeclared cover originals.
    pub fn validate_against(&self, baseline: &LibraryCheckpoint) -> Result<(), Error> {
        self.validate()?;
        if self.library_id != baseline.library_id {
            return Err(Error::conflict(
                "change set belongs to another local library",
            ));
        }
        let mut records = HashMap::new();
        for record in &baseline.records {
            record.validate()?;
            if records
                .insert(record.record().object_key()?, record)
                .is_some()
            {
                return Err(Error::conflict("ambiguous baseline typed target"));
            }
        }
        for expected in &self.expected {
            let target = expected_key(expected.kind, expected.object_uuid)?;
            let actual = records
                .get(&target)
                .map(PortableSerialize::content_hash)
                .transpose()?;
            if actual != expected.content_hash {
                return Err(Error::conflict("durable object precondition changed"));
            }
        }
        for record in &self.records {
            records.insert(record.record().object_key()?, record);
        }
        let available = |key| {
            records
                .get(&key)
                .is_some_and(|r| !r.record().is_tombstone())
        };
        let changed: HashSet<_> = self
            .records
            .iter()
            .map(|r| r.record().object_key())
            .collect::<Result<_, _>>()?;
        let deleting: HashSet<_> = self
            .records
            .iter()
            .filter(|r| r.record().is_tombstone())
            .map(|r| r.record().object_key())
            .collect::<Result<_, _>>()?;
        for record in records.values() {
            if record.record().is_tombstone() {
                continue;
            }
            let dependencies = dependencies(record);
            // Existing unresolved evidence is retained; changed records and any
            // relationship affected by a new deletion must be complete.
            if (changed.contains(&record.record().object_key()?)
                || dependencies.iter().any(|key| deleting.contains(key)))
                && dependencies.iter().any(|key| !available(*key))
            {
                return Err(Error::conflict("incomplete typed change set dependencies"));
            }
        }
        for record in &self.records {
            if let PortableRecord::Playlist(playlist) = record.record() {
                if let Some(asset) = &playlist.cover_asset {
                    let path = format!("echo/assets/{}", asset.content_hash);
                    if !self
                        .resources
                        .iter()
                        .chain(&baseline.resources)
                        .any(|resource| {
                            resource.relative_path == path
                                && resource.content_hash == asset.content_hash
                                && resource.byte_length == asset.byte_len
                        })
                    {
                        return Err(Error::conflict("missing playlist original declaration"));
                    }
                }
            }
        }
        Ok(())
    }
}
fn dependencies(record: &PortableRecordEnvelope) -> Vec<ObjectKey> {
    match record.record() {
        PortableRecord::Favorite(r) => vec![ObjectKey::Song(SongId::from_uuid(r.song_uuid))],
        PortableRecord::Override(r) => vec![ObjectKey::Song(SongId::from_uuid(r.song_uuid))],
        PortableRecord::PlayStats(r) => vec![ObjectKey::Song(SongId::from_uuid(r.song_uuid))],
        PortableRecord::PlaylistItem(r) => vec![
            ObjectKey::Song(SongId::from_uuid(r.song_uuid)),
            ObjectKey::Playlist(PlaylistId::from_uuid(r.playlist_uuid)),
        ],
        _ => vec![],
    }
}
fn expected_key(kind: RecordKind, id: uuid::Uuid) -> Result<ObjectKey, Error> {
    Ok(match kind {
        RecordKind::Song => ObjectKey::Song(SongId::from_uuid(id)),
        RecordKind::Favorite => ObjectKey::Favorite(SongId::from_uuid(id)),
        RecordKind::Playlist => ObjectKey::Playlist(PlaylistId::from_uuid(id)),
        RecordKind::PlaylistItem => {
            ObjectKey::PlaylistItem(crate::domain::ids::PlaylistItemId::from_uuid(id))
        }
        RecordKind::Override => ObjectKey::Override(SongId::from_uuid(id)),
        RecordKind::PlayStats => ObjectKey::PlayStats(SongId::from_uuid(id)),
        RecordKind::Tombstone => return Err(Error::conflict("untyped tombstone precondition")),
    })
}
