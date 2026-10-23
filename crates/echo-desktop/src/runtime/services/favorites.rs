//! Favorite commands delegate the durable boundary to Core.

use crate::ipc::dto::SongView;
use echo_core::domain::ids::SongId;
use echo_core::error::Error;

impl super::AppServices {
    /// Save a favorite value before updating the visible projection.
    /// # Errors
    /// Write-gate, storage and saved-pending-projection errors propagate.
    pub fn set_favorite(&self, song: SongId, favorite: bool) -> Result<SongView, Error> {
        let _activity = self.guard_writes()?;
        let root = self.active_root_id()?;
        let result = self.library_edits().set_favorite(root, song, favorite)?;
        Ok(SongView::from(&result))
    }
}
