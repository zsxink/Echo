//! Verified fast-path application of directory evidence.
use crate::domain::ids::LibraryRootId;
use crate::domain::library::durability::{LibraryCommit, LibraryReplay};
use crate::error::Error;

/// Fast-path application of verified directory evidence without a full replay.
pub trait LibraryProjectionVerifiedPort: Send + Sync {
    /// Project a replay returned by the directory durability adapter, using
    /// the current SQLite watermark to apply only its authenticated suffix.
    ///
    /// This operation also retries retained dependencies after applying the
    /// suffix, since a newly imported song can satisfy an older playlist item.
    fn project_verified_library_suffix(
        &self,
        root: LibraryRootId,
        replay: &LibraryReplay,
    ) -> Result<(), Error>;
    /// Apply one directory-verified commit when it directly follows the
    /// current SQLite watermark. Returns `false` when a catch-up replay is
    /// needed first.
    fn project_verified_library_commit(
        &self,
        _root: LibraryRootId,
        _commit: &LibraryCommit,
    ) -> Result<bool, Error> {
        Ok(false)
    }
}
