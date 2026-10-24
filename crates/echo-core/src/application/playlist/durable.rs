//! Library-authoritative favorite and playlist commands. Planning uses stable
//! identities; the complete after-image is committed before any projection.

use crate::application::ports::{ControlPlanePort, PlaylistRepository, SongRepository, UnitOfWork};
use crate::domain::entities::Song;
use crate::domain::ids::{LibraryRootId, PlaylistId, SongId};
use crate::domain::library::DeviceId;
use crate::domain::text::{playlist_name_key, validate_playlist_name};
use crate::error::{Error, Subject};

/// Dependencies shared by local logical edits, independent of desktop/SQLite.
pub struct DurableLibraryEdits<'a> {
    pub control: &'a dyn ControlPlanePort,
    pub songs: &'a dyn SongRepository,
    pub playlists: &'a dyn PlaylistRepository,
    pub unit: &'a dyn UnitOfWork,
    pub device: DeviceId,
}

impl DurableLibraryEdits<'_> {
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
        let edits = self.unit.local_library_edits().ok_or_else(|| {
            Error::unavailable("library edits", "ordered edit adapter is not configured")
        })?;
        edits.set_library_favorite(self.control, root, song, favorite, self.device)?;
        self.songs
            .by_id(song)?
            .ok_or_else(|| Error::unavailable("song", "projection disappeared"))
    }

    /// Persist an empty playlist definition with its original creation time.
    /// # Errors
    /// Invalid/colliding names, storage and pending projection propagate.
    pub fn create_playlist(&self, root: LibraryRootId, name: &str) -> Result<PlaylistId, Error> {
        let name = name.trim();
        self.validate_name(root, None, name)?;
        let id = PlaylistId::new();
        let edits = self.unit.local_library_edits().ok_or_else(|| {
            Error::unavailable("library edits", "ordered edit adapter is not configured")
        })?;
        edits.create_library_playlist(self.control, root, id, name, self.device)?;
        Ok(id)
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
        let edits = self.unit.local_library_edits().ok_or_else(|| {
            Error::unavailable("library edits", "ordered edit adapter is not configured")
        })?;
        edits.rename_library_playlist(self.control, root, id, name, self.device)
    }

    /// Commit the playlist and every membership tombstone as one set.
    /// # Errors
    /// Unknown targets, storage and pending projection propagate.
    pub fn delete_playlist(&self, root: LibraryRootId, id: PlaylistId) -> Result<(), Error> {
        let edits = self.unit.local_library_edits().ok_or_else(|| {
            Error::unavailable("library edits", "ordered edit adapter is not configured")
        })?;
        edits.delete_library_playlist(self.control, root, id, self.device)
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
        let edits = self.unit.local_library_edits().ok_or_else(|| {
            Error::unavailable("library edits", "ordered edit adapter is not configured")
        })?;
        edits.append_library_members(self.control, root, song, targets, self.device)
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
        let edits = self.unit.local_library_edits().ok_or_else(|| {
            Error::unavailable("library edits", "ordered edit adapter is not configured")
        })?;
        edits.remove_library_member(self.control, root, playlist, song, self.device)
    }
}
