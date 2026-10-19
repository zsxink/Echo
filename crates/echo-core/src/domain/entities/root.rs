//! Library root entity.

use crate::domain::ids::LibraryRootId;
use crate::error::Error;

use super::song::Song;

// ---------------------------------------------------------------------------
// LibraryRoot
// ---------------------------------------------------------------------------

/// A chosen library root directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LibraryRoot {
    id: LibraryRootId,
    /// Absolute path on this machine — kept in the entity as a raw field but
    /// **never** logged or serialized outward. Cross-team contracts use
    /// `RelativeMediaPath` only.
    absolute_path: std::path::PathBuf,
    is_active: bool,
    /// Capability observed from the filesystem / ownership marker. The
    /// effective capability also includes the safety isolation below.
    write_capable: bool,
    /// A durable safety isolation applied after an indeterminate destructive
    /// filesystem outcome. Re-probing permissions must never clear it.
    write_safety_locked: bool,
    availability: RootAvailability,
}

/// Whether the root directory is currently reachable & readable.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RootAvailability {
    #[default]
    Available,
    /// The root is missing, unmounted, or unreadable. Records are kept.
    Unavailable,
}

impl LibraryRoot {
    #[must_use]
    pub const fn new(
        id: LibraryRootId,
        absolute_path: std::path::PathBuf,
        is_active: bool,
        write_capable: bool,
    ) -> Self {
        Self {
            id,
            absolute_path,
            is_active,
            write_capable,
            write_safety_locked: false,
            availability: RootAvailability::Available,
        }
    }

    #[must_use]
    pub const fn id(&self) -> LibraryRootId {
        self.id
    }
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.is_active
    }
    #[must_use]
    pub const fn write_capable(&self) -> bool {
        self.write_capable && !self.write_safety_locked
    }
    /// The underlying filesystem capability, excluding a durable safety lock.
    #[must_use]
    pub const fn observed_write_capable(&self) -> bool {
        self.write_capable
    }
    /// Whether destructive operations have been durably isolated pending
    /// explicit operator recovery.
    #[must_use]
    pub const fn write_safety_locked(&self) -> bool {
        self.write_safety_locked
    }
    #[must_use]
    pub const fn availability(&self) -> RootAvailability {
        self.availability
    }
    /// Absolute path accessor — for the desktop boundary only; never into
    /// logs, IPC DTOs or sync payloads.
    #[must_use]
    pub fn absolute_path(&self) -> &std::path::Path {
        &self.absolute_path
    }

    /// Resolve one song's safe root-relative media path to its local absolute
    /// position. The root identity check prevents a caller from accidentally
    /// combining a song with a different library's directory.
    ///
    /// # Errors
    ///
    /// Returns `Conflict` when the song belongs to another library root.
    pub fn resolve_song_path(&self, song: &Song) -> Result<std::path::PathBuf, Error> {
        if song.root() != self.id {
            return Err(Error::conflict("song belongs to a different library root"));
        }
        Ok(self.absolute_path.join(song.path().normalized()))
    }

    /// Set write capability (derived from permissions + ownership marker).
    pub const fn set_write_capable(&mut self, capable: bool) {
        self.write_capable = capable;
    }

    /// Persist or clear the safety isolation independently from filesystem
    /// permission probing. Clearing this requires an explicit recovery path.
    pub const fn set_write_safety_locked(&mut self, locked: bool) {
        self.write_safety_locked = locked;
    }

    /// Mark the root available/unavailable, keeping records intact.
    pub const fn set_availability(&mut self, availability: RootAvailability) {
        self.availability = availability;
    }
}
