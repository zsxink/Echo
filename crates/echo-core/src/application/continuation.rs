//! Object-record continuation + reconciliation (restore-user-data-from-library-records).
//!
//! Opening a library directory that already carries a portable control surface
//! must **continue** the local database from `echo/records/` *before* `media/`
//! is scanned — otherwise the scan sees "a file with no row" and mints a fresh
//! UUID, which is exactly how a wiped app-data directory loses the user's
//! playlists, favorites and play counts (issue #1).
//!
//! The pass is deliberately conservative (design D5):
//!
//! 1. a tombstone whose version is at or above the record's wins — the object
//!    does not come back;
//! 2. a record with no tombstone is adopted as the last known state (including
//!    `is_favorite: false`, which is a real state, not "absent");
//! 3. a record that cannot stand locally (its song has neither a record nor a
//!    row) is **kept on disk**, excluded from the effective view, and counted
//!    in the reconciliation report — never deleted.
//!
//! Nothing here deletes a record file, and every projection is idempotent:
//! replaying the same records yields the same objects, UUIDs, member order and
//! play counts.
//!
//! ## Why this module exists separately from [`crate::application::restore`]
//!
//! `RestoreLibrary` is the *new-device* path (records → projection → media
//! scan → hash relink) and keeps owning the relink step. This module owns the
//! shared projection itself so the root-switch path (open a local directory)
//! and the restore path cannot drift apart — the bug being fixed here was
//! precisely "the use case exists but nothing calls it".

use std::collections::{BTreeMap, HashSet};

use crate::application::portable::{ControlPlaneStatus, EnsureControlPlane};
use crate::application::ports::{ControlPlanePort, PlaylistRepository, SongRepository, UnitOfWork};
use crate::domain::entities::{PlaylistMember, Song, SongAvailability};
use crate::domain::ids::{
    LibraryRootId, PlaylistId, PlaylistItemId, RelativeMediaPath, Revision, SongId,
};
use crate::domain::library::{HybridLogicalClock, PortableRecord, RecordKind, TombstoneRecord};
use crate::error::Error;

/// What one projection adopted, suppressed and could not place.
///
/// The counts are the observable reconciliation report (design D7): a caller
/// can tell "the manifest was healed" from "40 records could not be placed"
/// without reading the control surface itself.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ProjectionReport {
    /// Song records adopted into the local store (by their record UUID).
    pub songs: usize,
    /// Favorite states adopted (`true` and `false` alike).
    pub favorites: usize,
    /// Playlists created or renamed to match their records.
    pub playlists: usize,
    /// Playlist memberships restored, with the record's member UUID.
    pub members: usize,
    /// Play counts restored from `play-stats` records.
    pub play_stats: usize,
    /// Override records parsed that refer to a song in the effective view.
    ///
    /// 0.1.0 has no override *mutation* entry point, so these are validated
    /// and reported but not projected into `song_overrides`.
    pub overrides_seen: usize,
    /// Objects suppressed because a tombstone outranks their record.
    pub tombstoned: usize,
    /// Records that cannot stand locally and were excluded (kept on disk).
    pub invalid: usize,
    /// Records that describe a media path a **different** local row already
    /// owns: an earlier identity for the same file, re-minted by a later scan.
    ///
    /// The local row is the newer fact, so the record is not adopted — putting
    /// it in would leave two rows on one file (and the catalog's
    /// `(root, path)` uniqueness forbids it). The record is kept on disk.
    pub superseded: usize,
}

impl ProjectionReport {
    /// Total records adopted into the effective view.
    #[must_use]
    pub const fn adopted(&self) -> usize {
        self.songs + self.favorites + self.playlists + self.members + self.play_stats
    }
}

/// The outcome of one continuation pass.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ContinuationReport {
    /// What it took to make the control surface usable (healed / created / …).
    pub control_plane: ControlPlaneStatus,
    pub projection: ProjectionReport,
}

/// A tombstone index: object UUID → the newest deletion record.
type Tombstones = BTreeMap<uuid::Uuid, TombstoneRecord>;

/// Continue a library's local state from its portable object records.
pub struct ContinueFromRecords<'a> {
    control: &'a dyn ControlPlanePort,
    songs: &'a dyn SongRepository,
    playlists: &'a dyn PlaylistRepository,
    uow: &'a dyn UnitOfWork,
}

impl<'a> ContinueFromRecords<'a> {
    #[must_use]
    pub const fn new(
        control: &'a dyn ControlPlanePort,
        songs: &'a dyn SongRepository,
        playlists: &'a dyn PlaylistRepository,
        uow: &'a dyn UnitOfWork,
    ) -> Self {
        Self {
            control,
            songs,
            playlists,
            uow,
        }
    }

    /// Run the continuation for `root`.
    ///
    /// # Errors
    ///
    /// - [`Error::UnsupportedMedia`] when the manifest exists but this build
    ///   cannot read it — the caller must degrade to read-only rather than
    ///   project (the file is left untouched).
    /// - Propagates control-surface and store failures.
    pub fn run(&self, root: LibraryRootId) -> Result<ContinuationReport, Error> {
        let control_plane = EnsureControlPlane::new(self.control).run(root)?;
        // An unwritable control surface means read-only semantics: the local
        // database must not be rebuilt from records it could never materialize
        // back (spec 控制面不可写). Nothing is projected, nothing is deleted.
        if matches!(control_plane, ControlPlaneStatus::NotUsable) {
            return Ok(ContinuationReport {
                control_plane,
                projection: ProjectionReport::default(),
            });
        }
        let projection = self.project(root)?;
        Ok(ContinuationReport {
            control_plane,
            projection,
        })
    }

    /// Project every object kind, parents before children.
    fn project(&self, root: LibraryRootId) -> Result<ProjectionReport, Error> {
        let tombstones = self.tombstones(root)?;
        let mut report = ProjectionReport::default();

        // 1. Songs first: everything else references a song identity.
        self.project_songs(root, &tombstones, &mut report)?;
        // 2. Favorites, playlists, then members (members need both parents).
        self.project_favorites(root, &tombstones, &mut report)?;
        self.project_playlists(root, &tombstones, &mut report)?;
        self.project_playlist_items(root, &tombstones, &mut report)?;
        // 3. Play statistics and overrides hang off a song.
        self.project_play_stats(root, &tombstones, &mut report)?;
        self.project_overrides(root, &tombstones, &mut report)?;
        Ok(report)
    }

    /// Read every tombstone, keyed by the deleted object's UUID. When the same
    /// object was tombstoned more than once the newest wins.
    fn tombstones(&self, root: LibraryRootId) -> Result<Tombstones, Error> {
        let mut index = Tombstones::new();
        for raw in self.control.list_records(root, RecordKind::Tombstone)? {
            let Ok(PortableRecord::Tombstone(tombstone)) =
                serde_json::from_str::<PortableRecord>(&raw)
            else {
                continue;
            };
            index
                .entry(tombstone.object_uuid)
                .and_modify(|current| {
                    if version_of(tombstone.hlc, tombstone.revision)
                        > version_of(current.hlc, current.revision)
                    {
                        *current = tombstone.clone();
                    }
                })
                .or_insert(tombstone);
        }
        Ok(index)
    }

    fn project_songs(
        &self,
        root: LibraryRootId,
        tombstones: &Tombstones,
        report: &mut ProjectionReport,
    ) -> Result<(), Error> {
        for raw in self.control.list_records(root, RecordKind::Song)? {
            let Ok(PortableRecord::Song(song_record)) =
                serde_json::from_str::<PortableRecord>(&raw)
            else {
                report.invalid += 1;
                continue;
            };
            if suppressed(
                tombstones,
                &song_record.song_uuid,
                song_record.hlc,
                song_record.revision,
            ) {
                report.tombstoned += 1;
                continue;
            }
            let Ok(path) = RelativeMediaPath::new(song_record.media_path.as_str()) else {
                report.invalid += 1;
                continue;
            };
            let id = SongId::from_uuid(song_record.song_uuid);
            // An already-available local row keeps its media state: the scan
            // owns availability, continuation only owns identity + metadata.
            if let Some(current) = self.songs.by_id(id)? {
                if current.availability() == SongAvailability::Available {
                    report.songs += 1;
                    continue;
                }
            }
            // A *different* local row already owns this media path: the record
            // is an earlier identity for the same file, re-minted by a later
            // scan (the shape a real library took after its app-data was
            // wiped once in the past). The local identity is the newer fact, so
            // the record is not adopted here — adopting it would put two rows
            // on one file, which the catalog's `(root, path)` uniqueness
            // forbids outright. The record stays on disk, untouched.
            if let Some(owner) = self.songs.by_path(root, &path)? {
                if owner.id() != id {
                    report.superseded += 1;
                    continue;
                }
            }
            let mut song = Song::new(id, root, path, Revision::INITIAL);
            song.apply_metadata(
                song_record.title.clone(),
                song_record.artist.clone(),
                song_record.album.clone(),
                None,
            );
            song.mark_missing();
            let song = song.clone();
            self.uow.with_tx(Box::new(move |tx| {
                tx.upsert_song(&song)?;
                Ok(())
            }))?;
            report.songs += 1;
        }
        Ok(())
    }

    fn project_favorites(
        &self,
        root: LibraryRootId,
        tombstones: &Tombstones,
        report: &mut ProjectionReport,
    ) -> Result<(), Error> {
        let known = self.known_songs(root)?;
        for raw in self.control.list_records(root, RecordKind::Favorite)? {
            let Ok(PortableRecord::Favorite(favorite)) =
                serde_json::from_str::<PortableRecord>(&raw)
            else {
                report.invalid += 1;
                continue;
            };
            let id = SongId::from_uuid(favorite.song_uuid);
            if suppressed(
                tombstones,
                &favorite.song_uuid,
                favorite.hlc,
                favorite.revision,
            ) {
                // A tombstoned favorite must not come back: the last known
                // state is "not a favorite".
                report.tombstoned += 1;
                if known.contains(&id) {
                    self.songs.set_favorite(id, false)?;
                }
                continue;
            }
            if !known.contains(&id) {
                // Dangling: keep the file, count it, never resurrect the song.
                report.invalid += 1;
                continue;
            }
            self.songs.set_favorite(id, favorite.is_favorite)?;
            report.favorites += 1;
        }
        Ok(())
    }

    fn project_playlists(
        &self,
        root: LibraryRootId,
        tombstones: &Tombstones,
        report: &mut ProjectionReport,
    ) -> Result<(), Error> {
        for raw in self.control.list_records(root, RecordKind::Playlist)? {
            let Ok(PortableRecord::Playlist(playlist)) =
                serde_json::from_str::<PortableRecord>(&raw)
            else {
                report.invalid += 1;
                continue;
            };
            let id = PlaylistId::from_uuid(playlist.playlist_uuid);
            if suppressed(
                tombstones,
                &playlist.playlist_uuid,
                playlist.hlc,
                playlist.revision,
            ) {
                report.tombstoned += 1;
                // A deleted playlist does not come back — remove the local
                // projection so the delete propagates.
                if self.playlists.by_id(id)?.is_some() {
                    self.playlists.delete(id)?;
                }
                continue;
            }
            if self.playlists.by_id(id)?.is_none() {
                if self
                    .playlists
                    .create(id, root, &playlist.display_name)
                    .is_err()
                {
                    // A name collision with a local playlist is not a reason
                    // to fail the whole open; report and move on.
                    report.invalid += 1;
                    continue;
                }
            } else {
                let current = self.playlists.name(id)?;
                if current.as_deref() != Some(playlist.display_name.as_str())
                    && self.playlists.rename(id, &playlist.display_name).is_err()
                {
                    report.invalid += 1;
                    continue;
                }
            }
            report.playlists += 1;
        }
        Ok(())
    }

    fn project_playlist_items(
        &self,
        root: LibraryRootId,
        tombstones: &Tombstones,
        report: &mut ProjectionReport,
    ) -> Result<(), Error> {
        let known = self.known_songs(root)?;
        // Deterministic order: position first, then the member UUID — the
        // record order on disk must never decide the playlist's order.
        let mut items = Vec::new();
        for raw in self.control.list_records(root, RecordKind::PlaylistItem)? {
            match serde_json::from_str::<PortableRecord>(&raw) {
                Ok(PortableRecord::PlaylistItem(item)) => items.push(item),
                _ => report.invalid += 1,
            }
        }
        items.sort_by(|a, b| {
            a.position
                .cmp(&b.position)
                .then(a.item_uuid.cmp(&b.item_uuid))
        });

        for item in items {
            let playlist = PlaylistId::from_uuid(item.playlist_uuid);
            let song = SongId::from_uuid(item.song_uuid);
            if suppressed(tombstones, &item.item_uuid, item.hlc, item.revision) {
                report.tombstoned += 1;
                if self.playlists.by_id(playlist)?.is_some() && known.contains(&song) {
                    self.playlists.remove_member(playlist, song)?;
                }
                continue;
            }
            // A member whose playlist or song is not in the effective view is
            // dangling: kept on disk, counted, never invented (design D5 #3).
            if self.playlists.by_id(playlist)?.is_none() || !known.contains(&song) {
                report.invalid += 1;
                continue;
            }
            let member = PlaylistMember::with_id(
                PlaylistItemId::from_uuid(item.item_uuid),
                playlist,
                song,
                item.position,
                SongAvailability::Available,
            );
            if self.playlists.upsert_member(&member).is_err() {
                // A position clash inside a malformed record set must not fail
                // the open; the member is reported as unplaced.
                report.invalid += 1;
                continue;
            }
            report.members += 1;
        }
        Ok(())
    }

    fn project_play_stats(
        &self,
        root: LibraryRootId,
        tombstones: &Tombstones,
        report: &mut ProjectionReport,
    ) -> Result<(), Error> {
        let known = self.known_songs(root)?;
        for raw in self.control.list_records(root, RecordKind::PlayStats)? {
            let Ok(PortableRecord::PlayStats(stats)) = serde_json::from_str::<PortableRecord>(&raw)
            else {
                report.invalid += 1;
                continue;
            };
            let id = SongId::from_uuid(stats.song_uuid);
            if suppressed(tombstones, &stats.song_uuid, stats.hlc, stats.revision) {
                report.tombstoned += 1;
                continue;
            }
            if !known.contains(&id) {
                report.invalid += 1;
                continue;
            }
            // Σ by_device: two devices each playing once merge to 2, and a
            // replay of the same record never doubles the count.
            self.songs.set_play_count(id, stats.total())?;
            report.play_stats += 1;
        }
        Ok(())
    }

    fn project_overrides(
        &self,
        root: LibraryRootId,
        tombstones: &Tombstones,
        report: &mut ProjectionReport,
    ) -> Result<(), Error> {
        let known = self.known_songs(root)?;
        for raw in self.control.list_records(root, RecordKind::Override)? {
            let Ok(PortableRecord::Override(override_record)) =
                serde_json::from_str::<PortableRecord>(&raw)
            else {
                report.invalid += 1;
                continue;
            };
            if suppressed(
                tombstones,
                &override_record.song_uuid,
                override_record.hlc,
                override_record.revision,
            ) {
                report.tombstoned += 1;
                continue;
            }
            if known.contains(&SongId::from_uuid(override_record.song_uuid)) {
                report.overrides_seen += 1;
            } else {
                report.invalid += 1;
            }
        }
        Ok(())
    }

    /// The songs currently in the effective view of `root`.
    fn known_songs(&self, root: LibraryRootId) -> Result<HashSet<SongId>, Error> {
        Ok(self
            .songs
            .all_in_root(root)?
            .into_iter()
            .map(|song| song.id())
            .collect())
    }
}

/// Whether a tombstone outranks the record it deletes (design D5 rule 1).
///
/// A tombstone at exactly the record's version also wins: the delete was the
/// last fact written for that object.
fn suppressed(
    tombstones: &Tombstones,
    object_uuid: &uuid::Uuid,
    hlc: HybridLogicalClock,
    revision: Revision,
) -> bool {
    tombstones.get(object_uuid).is_some_and(|tombstone| {
        version_of(hlc, revision) <= version_of(tombstone.hlc, tombstone.revision)
    })
}

/// Strict ordering of two versions: HLC first, revision as the tie-break.
const fn version_of(hlc: HybridLogicalClock, revision: Revision) -> (HybridLogicalClock, u64) {
    (hlc, revision.as_u64())
}

#[cfg(test)]
mod tests;
