//! Playlist mutations (task 7.3). Every mutation obeys the write gate and
//! returns the committed outcome the UI renders.

use echo_core::application::playlist::{
    AddToPlaylists, CreatePlaylist, DeletePlaylist, RemoveFromPlaylist, RenamePlaylist,
};
use echo_core::application::ports::PlaylistRepository;
use echo_core::domain::ids::{LibraryRootId, PlaylistId, SongId};
use echo_core::error::Error;

impl super::AppServices {
    /// Create a playlist; returns its id.
    ///
    /// # Errors
    ///
    /// `Validation` for an invalid name; `Conflict` for a duplicate name;
    /// `Unavailable` when writes are disabled; storage errors propagate.
    pub fn create_playlist(&self, root: LibraryRootId, name: &str) -> Result<String, Error> {
        self.guard_writes()?;
        // The command's root comes from the shell status snapshot.  Recheck it
        // at the mutation boundary: a status event can race a root switch, and
        // creating in an old/forged root would make the new playlist invisible
        // to the active navigation.
        let active = self
            .deps
            .roots
            .active_root()?
            .map(|candidate| candidate.id());
        if active != Some(root) {
            return Err(Error::unavailable("library", "active root changed"));
        }
        Ok(CreatePlaylist::new(self.deps.playlists.as_ref())
            .execute(root, name)?
            .to_string())
    }

    /// Rename a playlist (members preserved).
    ///
    /// # Errors
    ///
    /// `Validation`/`Conflict` for an invalid or colliding name; `Unavailable`
    /// for an unknown playlist or disabled writes; storage errors propagate.
    pub fn rename_playlist(&self, id: PlaylistId, name: &str) -> Result<(), Error> {
        self.guard_writes()?;
        RenamePlaylist::new(self.deps.playlists.as_ref()).execute(id, name)
    }

    /// Persist a user-selected image as the playlist cover. `None` clears the
    /// manual choice and returns the playlist to automatic newest-song artwork.
    pub fn set_playlist_cover(
        &self,
        id: PlaylistId,
        bytes: Option<Vec<u8>>,
        mime: Option<&str>,
    ) -> Result<(), Error> {
        self.guard_writes()?;
        if PlaylistRepository::by_id(self.deps.playlists.as_ref(), id)?.is_none() {
            return Err(Error::unavailable("playlist", "not found"));
        }
        let key = match bytes {
            None => None,
            Some(bytes) => {
                const MAX_COVER_BYTES: usize = 5 * 1024 * 1024;
                let mime = mime.unwrap_or_default();
                if bytes.is_empty() || bytes.len() > MAX_COVER_BYTES {
                    return Err(Error::validation(
                        echo_core::error::Subject::Other,
                        "cover",
                        "image must be between 1 byte and 5 MiB",
                    ));
                }
                if !matches!(mime, "image/jpeg" | "image/png" | "image/webp") {
                    return Err(Error::validation(
                        echo_core::error::Subject::Other,
                        "cover",
                        "unsupported image type",
                    ));
                }
                // The cache owns raw image bytes and returns an opaque key;
                // neither a source path nor the bytes cross back to the UI.
                Some(self.deps.cover_cache.put(&bytes, mime)?)
            }
        };
        self.deps.playlists.set_cover_key(id, key.as_deref())
    }

    /// Delete a playlist (songs untouched).
    ///
    /// # Errors
    ///
    /// `Unavailable` when writes are disabled; storage errors propagate.
    pub fn delete_playlist(&self, id: PlaylistId) -> Result<(), Error> {
        self.guard_writes()?;
        DeletePlaylist::new(self.deps.playlists.as_ref()).execute(id)
    }

    /// Add one song to one or more playlists atomically.
    ///
    /// # Errors
    ///
    /// `Unavailable` when writes are disabled or a target playlist does not
    /// exist; storage errors propagate.
    pub fn add_to_playlists(&self, song: SongId, targets: &[PlaylistId]) -> Result<(), Error> {
        self.guard_writes()?;
        AddToPlaylists::new(
            self.deps.songs.as_ref(),
            self.deps.playlists.as_ref(),
            self.deps.uow.as_ref(),
        )
        .execute(song, targets, u64::MAX)
    }

    /// Remove `song` from `playlist`; removing a non-member is a no-op success.
    ///
    /// # Errors
    ///
    /// `Unavailable` when writes are disabled; storage errors propagate.
    pub fn remove_playlist_song(&self, playlist: PlaylistId, song: SongId) -> Result<(), Error> {
        self.guard_writes()?;
        RemoveFromPlaylist::new(self.deps.playlists.as_ref()).execute(playlist, song)
    }
}
