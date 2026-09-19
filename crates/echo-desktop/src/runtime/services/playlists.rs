//! Playlist mutations (task 7.3). Every mutation obeys the write gate and
//! returns the committed outcome the UI renders.
//!
//! Every committed playlist/membership change is **materialized** into
//! `echo/records/` after the `SQLite` transaction commits (design D6): the
//! local database is a rebuildable projection, so a change that never reached
//! the portable records is a change the next "open this directory" would lose.
//! A failed materialization fails the mutation — the same rule favorites and
//! imports already follow.

use echo_core::application::playlist::{
    AddToPlaylists, CreatePlaylist, DeletePlaylist, RemoveFromPlaylist, RenamePlaylist,
};
use echo_core::application::portable_materialize::{
    committed_version, ensure_control_plane_writable, playlist_item_record, playlist_record,
    tombstone_record, write_record_guarded, PLAYLIST_ITEM_OBJECT_TYPE, PLAYLIST_OBJECT_TYPE,
};
use echo_core::application::ports::PlaylistRepository;
use echo_core::domain::ids::{LibraryRootId, PlaylistId, SongId};
use echo_core::domain::library::{PortableRecord, RecordKind};
use echo_core::error::Error;

impl super::AppServices {
    /// Create a playlist; returns its id.
    ///
    /// # Errors
    ///
    /// `Validation` for an invalid name; `Conflict` for a duplicate name;
    /// `Unavailable` when writes are disabled or the control surface is not
    /// writable; storage errors propagate.
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
        ensure_control_plane_writable(self.deps.control.as_ref(), root)?;
        let id = CreatePlaylist::new(self.deps.playlists.as_ref()).execute(root, name)?;
        self.materialize_playlist(root, id, name)?;
        Ok(id.to_string())
    }

    /// Rename a playlist (members preserved).
    ///
    /// # Errors
    ///
    /// `Validation`/`Conflict` for an invalid or colliding name; `Unavailable`
    /// for an unknown playlist, disabled writes, or an unwritable control
    /// surface; storage errors propagate.
    pub fn rename_playlist(&self, id: PlaylistId, name: &str) -> Result<(), Error> {
        self.guard_writes()?;
        let root = self.active_root_id()?;
        ensure_control_plane_writable(self.deps.control.as_ref(), root)?;
        let trimmed = name.trim();
        RenamePlaylist::new(self.deps.playlists.as_ref()).execute(id, name)?;
        self.materialize_playlist(root, id, trimmed)?;
        Ok(())
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

    /// Delete a playlist (songs untouched). The deletion is materialized as a
    /// **tombstone**, so reopening the library does not resurrect it.
    ///
    /// # Errors
    ///
    /// `Unavailable` when writes are disabled; storage errors propagate.
    pub fn delete_playlist(&self, id: PlaylistId) -> Result<(), Error> {
        self.guard_writes()?;
        let root = self.active_root_id()?;
        ensure_control_plane_writable(self.deps.control.as_ref(), root)?;
        let device = self.deps.device_id.current_device_id();
        let (revision, hlc) = committed_version(
            self.deps.sync.as_ref(),
            PLAYLIST_OBJECT_TYPE,
            &id.to_string(),
            device,
        )?;
        DeletePlaylist::new(self.deps.playlists.as_ref()).execute(id)?;
        // The row is gone, so the tombstone is written from the revision read
        // before the delete (one past it: the delete is the newer fact).
        let record = PortableRecord::Tombstone(tombstone_record(
            device,
            hlc,
            echo_core::domain::ids::Revision::from_u64(revision.as_u64().saturating_add(1)),
            id.as_uuid(),
            RecordKind::Playlist,
        ));
        write_record_guarded(self.deps.control.as_ref(), root, &record)?;
        Ok(())
    }

    /// Add one song to one or more playlists atomically. Each committed
    /// membership is materialized as a `playlist-items` record carrying the
    /// member's stable UUID and position, so member order survives a rebuild.
    ///
    /// # Errors
    ///
    /// `Unavailable` when writes are disabled or a target playlist does not
    /// exist; storage errors propagate.
    pub fn add_to_playlists(&self, song: SongId, targets: &[PlaylistId]) -> Result<(), Error> {
        self.guard_writes()?;
        let root = self.active_root_id()?;
        ensure_control_plane_writable(self.deps.control.as_ref(), root)?;
        AddToPlaylists::new(
            self.deps.songs.as_ref(),
            self.deps.playlists.as_ref(),
            self.deps.uow.as_ref(),
        )
        .execute(song, targets, u64::MAX)?;
        self.materialize_members(root, song, targets)?;
        Ok(())
    }

    /// Remove `song` from `playlist`; removing a non-member is a no-op success.
    /// The removal is materialized as a tombstone of the membership.
    ///
    /// # Errors
    ///
    /// `Unavailable` when writes are disabled; storage errors propagate.
    pub fn remove_playlist_song(&self, playlist: PlaylistId, song: SongId) -> Result<(), Error> {
        self.guard_writes()?;
        let root = self.active_root_id()?;
        ensure_control_plane_writable(self.deps.control.as_ref(), root)?;
        let device = self.deps.device_id.current_device_id();
        // Read the membership identity *before* the delete erases it — the
        // tombstone must name the member UUID, not the (playlist, song) pair.
        let member = PlaylistRepository::members(self.deps.playlists.as_ref(), playlist)?
            .into_iter()
            .find(|member| member.song() == song);
        let (revision, hlc) = match member.as_ref() {
            Some(member) => committed_version(
                self.deps.sync.as_ref(),
                PLAYLIST_ITEM_OBJECT_TYPE,
                &member.id().to_string(),
                device,
            )?,
            None => (
                echo_core::domain::ids::Revision::INITIAL,
                echo_core::domain::library::HybridLogicalClock::default(),
            ),
        };
        RemoveFromPlaylist::new(self.deps.playlists.as_ref()).execute(playlist, song)?;
        if let Some(member) = member {
            let record = PortableRecord::Tombstone(tombstone_record(
                device,
                hlc,
                echo_core::domain::ids::Revision::from_u64(revision.as_u64().saturating_add(1)),
                member.id().as_uuid(),
                RecordKind::PlaylistItem,
            ));
            write_record_guarded(self.deps.control.as_ref(), root, &record)?;
        }
        Ok(())
    }

    /// Write the playlist record for a committed create/rename.
    fn materialize_playlist(
        &self,
        root: LibraryRootId,
        id: PlaylistId,
        name: &str,
    ) -> Result<(), Error> {
        let device = self.deps.device_id.current_device_id();
        let (revision, hlc) = committed_version(
            self.deps.sync.as_ref(),
            PLAYLIST_OBJECT_TYPE,
            &id.to_string(),
            device,
        )?;
        let record = PortableRecord::Playlist(playlist_record(device, hlc, revision, id, name));
        self.deps.control.write_record(root, &record)?;
        Ok(())
    }

    /// Write a `playlist-items` record for each committed membership.
    fn materialize_members(
        &self,
        root: LibraryRootId,
        song: SongId,
        targets: &[PlaylistId],
    ) -> Result<(), Error> {
        let device = self.deps.device_id.current_device_id();
        for playlist in targets {
            // A membership that is not visible after commit is not ours to
            // record (for example an idempotent re-add that kept the row).
            let Some(member) =
                PlaylistRepository::members(self.deps.playlists.as_ref(), *playlist)?
                    .into_iter()
                    .find(|member| member.song() == song)
            else {
                continue;
            };
            let (revision, hlc) = committed_version(
                self.deps.sync.as_ref(),
                PLAYLIST_ITEM_OBJECT_TYPE,
                &member.id().to_string(),
                device,
            )?;
            let record = PortableRecord::PlaylistItem(playlist_item_record(
                device,
                hlc,
                revision,
                member.id(),
                *playlist,
                song,
                member.position(),
            ));
            self.deps.control.write_record(root, &record)?;
        }
        Ok(())
    }

    /// The active root id, or `Unavailable` when no library is configured.
    fn active_root_id(&self) -> Result<LibraryRootId, Error> {
        self.deps
            .roots
            .active_root()?
            .map(|root| root.id())
            .ok_or_else(|| Error::unavailable("library", "no active root"))
    }
}
