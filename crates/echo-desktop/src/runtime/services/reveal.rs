//! Reveal a song's file in the OS file manager (task 7.5). The absolute
//! location is resolved and consumed entirely desktop-side; the UI receives only
//! the relative path plus whether the reveal succeeded.

use echo_core::domain::ids::SongId;
use echo_core::error::Error;

use crate::ipc::dto::RevealResultDto;
use crate::platform::dialogs::RevealOutcome;

impl super::AppServices {
    /// Reveal a song's file in the OS file manager by `SongId`. The absolute
    /// location is resolved and consumed entirely desktop-side; the UI receives
    /// only the relative path plus whether the reveal succeeded.
    ///
    /// # Errors
    ///
    /// `Unavailable` when the song or its root is unknown; storage errors
    /// propagate. A soft reveal failure is reported in the DTO, not as an
    /// error, so the UI can fall back to showing the relative path.
    pub fn reveal_song(&self, song: SongId) -> Result<RevealResultDto, Error> {
        let record = self
            .deps
            .songs
            .by_id(song)?
            .ok_or_else(|| Error::unavailable("song", "unknown song"))?;
        let root = self
            .deps
            .roots
            .by_id(record.root())?
            .ok_or_else(|| Error::unavailable("library", "song's root is unknown"))?;
        let absolute = root.resolve_song_path(&record)?;
        let revealed = match self.dialogs.reveal(&absolute)? {
            RevealOutcome::Revealed => true,
            RevealOutcome::Unavailable => false,
        };
        Ok(RevealResultDto {
            song_id: song.to_string(),
            relative_path: record.path().normalized().to_owned(),
            revealed,
        })
    }
}
