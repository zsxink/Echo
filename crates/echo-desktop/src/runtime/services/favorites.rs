//! Favorite mutation (task 7.3). Always obeys the write gate.

use echo_core::application::favorite::SetFavorite;
use echo_core::application::portable_materialize::{
    committed_version, ensure_control_plane_writable, favorite_record, FAVORITE_OBJECT_TYPE,
};
use echo_core::domain::ids::SongId;
use echo_core::domain::library::PortableRecord;
use echo_core::error::Error;

use crate::ipc::dto::SongView;

impl super::AppServices {
    /// Toggle favorite; returns the authoritative committed song view.
    ///
    /// # Errors
    ///
    /// `Unavailable` when writes are disabled or the song is not toggleable;
    /// storage errors propagate.
    pub fn set_favorite(&self, song: SongId, favorite: bool) -> Result<SongView, Error> {
        self.guard_writes()?;
        let root = self
            .deps
            .roots
            .active_root()?
            .ok_or_else(|| Error::unavailable("library", "no active root"))?;
        // Check before the SQLite transaction: a control-plane-disabled library
        // must reject logical mutations rather than commit a fact it cannot
        // materialize for recovery.
        ensure_control_plane_writable(self.deps.control.as_ref(), root.id())?;
        let result = SetFavorite::new(self.deps.songs.as_ref()).execute(song, favorite)?;
        let device = self.deps.device_id.current_device_id();
        let (revision, hlc) = committed_version(
            self.deps.sync.as_ref(),
            FAVORITE_OBJECT_TYPE,
            &song.to_string(),
            device,
        )?;
        let record = PortableRecord::Favorite(favorite_record(
            device,
            hlc,
            revision,
            song,
            result.favorite,
        ));
        self.deps.control.write_record(root.id(), &record)?;
        Ok(SongView::from(&result.song))
    }
}
