//! Desktop playlist commands; Core owns planning, directory commit and projection.

use echo_core::application::playlist::durable::DurableLibraryEdits;
use echo_core::domain::ids::{LibraryRootId, PlaylistId, SongId};
use echo_core::error::Error;

impl super::AppServices {
    pub(super) fn library_edits(&self) -> DurableLibraryEdits<'_> {
        DurableLibraryEdits {
            control: self.deps.control.as_ref(),
            songs: self.deps.songs.as_ref(),
            playlists: self.deps.playlists.as_ref(),
            unit: self.deps.uow.as_ref(),
            device: self.deps.device_id.current_device_id(),
        }
    }

    /// Create an empty playlist in the active library.
    /// # Errors
    /// Invalid input, active-root changes and persistence failures propagate.
    pub fn create_playlist(&self, root: LibraryRootId, name: &str) -> Result<String, Error> {
        let _activity = self.guard_writes()?;
        if self.active_root_id()? != root {
            return Err(Error::unavailable("library", "active root changed"));
        }
        self.library_edits()
            .create_playlist(root, name)
            .map(|id| id.to_string())
    }

    /// Rename while preserving members, creation time and cover.
    /// # Errors
    /// Validation, storage and saved-pending-projection errors propagate.
    pub fn rename_playlist(&self, id: PlaylistId, name: &str) -> Result<(), Error> {
        let _activity = self.guard_writes()?;
        self.library_edits()
            .rename_playlist(self.active_root_id()?, id, name)
    }

    /// Persist a user-selected image as the playlist cover. `None` clears the
    /// manual choice and returns the playlist to automatic newest-song artwork.
    ///
    /// # Errors
    ///
    /// `Unavailable` when writes are disabled or `id` is not a known playlist;
    /// `Validation` when the image is empty, exceeds 5 MiB, or its MIME type is
    /// not JPEG/PNG/WebP; cover-cache and storage failures propagate.
    pub fn set_playlist_cover(
        &self,
        id: PlaylistId,
        bytes: Option<&[u8]>,
        mime: Option<&str>,
    ) -> Result<(), Error> {
        let _activity = self.guard_writes()?;
        self.library_edits()
            .set_playlist_cover(self.active_root_id()?, id, bytes, mime)
    }

    /// Delete the playlist and membership facts as one durable set.
    /// # Errors
    /// Storage and saved-pending-projection errors propagate.
    pub fn delete_playlist(&self, id: PlaylistId) -> Result<(), Error> {
        let _activity = self.guard_writes()?;
        self.library_edits()
            .delete_playlist(self.active_root_id()?, id)
    }

    /// Append once to each target, preserving stable member identities.
    /// # Errors
    /// Invalid targets, storage and saved-pending-projection errors propagate.
    pub fn add_to_playlists(&self, song: SongId, targets: &[PlaylistId]) -> Result<(), Error> {
        let _activity = self.guard_writes()?;
        self.library_edits()
            .add_to_playlists(self.active_root_id()?, song, targets)
    }

    /// Remove a membership, retaining an explicit durable tombstone.
    /// # Errors
    /// Invalid targets, storage and saved-pending-projection errors propagate.
    pub fn remove_playlist_song(&self, playlist: PlaylistId, song: SongId) -> Result<(), Error> {
        let _activity = self.guard_writes()?;
        self.library_edits()
            .remove_playlist_song(self.active_root_id()?, playlist, song)
    }

    pub(super) fn active_root_id(&self) -> Result<LibraryRootId, Error> {
        self.deps
            .roots
            .active_root()?
            .map(|root| root.id())
            .ok_or_else(|| Error::unavailable("library", "no active root"))
    }
}
