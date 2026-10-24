//! Serialized local edits freeze their planned facts before directory commit.
use super::ControlPlanePort;
use crate::domain::ids::{LibraryRootId, PlaylistId, SongId};
use crate::domain::library::DeviceId;
use crate::error::Error;

pub trait LocalLibraryEditPort: Send + Sync {
    /// Remove a stable member and freeze the remaining playlist in one operation.
    fn remove_library_member(
        &self,
        control: &dyn ControlPlanePort,
        root: LibraryRootId,
        playlist: PlaylistId,
        song: SongId,
        device: DeviceId,
    ) -> Result<(), Error>;

    /// Append once per target, freezing member identities and complete snapshots.
    fn append_library_members(
        &self,
        control: &dyn ControlPlanePort,
        root: LibraryRootId,
        song: SongId,
        targets: &[PlaylistId],
        device: DeviceId,
    ) -> Result<(), Error>;

    /// Save the playlist and all known membership tombstones as one operation.
    fn delete_library_playlist(
        &self,
        control: &dyn ControlPlanePort,
        root: LibraryRootId,
        id: PlaylistId,
        device: DeviceId,
    ) -> Result<(), Error>;

    /// Freeze an explicit favorite value before updating its read model.
    fn set_library_favorite(
        &self,
        control: &dyn ControlPlanePort,
        root: LibraryRootId,
        song: SongId,
        favorite: bool,
        device: DeviceId,
    ) -> Result<(), Error>;

    /// Plan without committing SQL, save the immutable operation, then catch up
    /// the complete directory chain. Never report success for SQL alone.
    fn create_library_playlist(
        &self,
        control: &dyn ControlPlanePort,
        root: LibraryRootId,
        id: PlaylistId,
        name: &str,
        device: DeviceId,
    ) -> Result<(), Error>;

    /// Preserve the original playlist facts while freezing a renamed snapshot.
    fn rename_library_playlist(
        &self,
        control: &dyn ControlPlanePort,
        root: LibraryRootId,
        id: PlaylistId,
        name: &str,
        device: DeviceId,
    ) -> Result<(), Error>;
}
