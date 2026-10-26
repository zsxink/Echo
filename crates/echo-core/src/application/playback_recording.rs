//! Qualified playback events are committed to the music library before their
//! absolute per-device buckets are projected. Repeating a session cannot count twice.

use crate::application::ports::{ControlPlanePort, SongRepository, UnitOfWork};
use crate::domain::ids::{PlaybackSessionId, SongId};
use crate::domain::library::DeviceId;
use crate::error::Error;

/// Record through the adapter's serialized directory-first planning boundary.
/// # Errors
/// Missing adapters, directory failures and saved-pending projection propagate.
pub fn record_qualified_playback(
    control: &dyn ControlPlanePort,
    songs: &dyn SongRepository,
    unit: &dyn UnitOfWork,
    device: DeviceId,
    session: PlaybackSessionId,
    song: SongId,
    qualified_at: u64,
) -> Result<bool, Error> {
    let Some(current) = songs.by_id(song)? else {
        return Ok(false);
    };
    if !current.availability().is_visible() {
        return Ok(false);
    }
    unit.playback_edits()
        .ok_or_else(|| Error::unavailable("playback", "ordered adapter missing"))?
        .record_library_playback(control, current.root(), session, song, device, qualified_at)
}
