//! Domain rules for deciding whether persisted playback entries can return.
//!
//! Repository reads remain an application concern. Once the active root and
//! optional song record are known, these pure rules make every platform reach
//! the same restore and retry decision.

use crate::domain::entities::Song;
use crate::domain::ids::LibraryRootId;

/// The platform-neutral outcome for one persisted library queue entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlaybackRestoreDisposition {
    /// The song is still in the active library and can be loaded now.
    Restore,
    /// The identity remains valid but its media is temporarily unavailable.
    Blocked,
    /// The entry is absent, belongs to another root, or is pending deletion.
    Drop,
}

/// Classify a persisted song using the current active-root identity and its
/// repository result. A read failure is represented by `None` and therefore
/// safely drops the stale entry, matching startup's existing behavior.
#[must_use]
pub fn playback_restore_disposition(
    active_root: Option<LibraryRootId>,
    song: Option<&Song>,
) -> PlaybackRestoreDisposition {
    let Some(active_root) = active_root else {
        return PlaybackRestoreDisposition::Drop;
    };
    let Some(song) = song else {
        return PlaybackRestoreDisposition::Drop;
    };
    if song.root() != active_root {
        return PlaybackRestoreDisposition::Drop;
    }
    match song.availability() {
        crate::domain::entities::SongAvailability::Available => PlaybackRestoreDisposition::Restore,
        crate::domain::entities::SongAvailability::Missing => PlaybackRestoreDisposition::Blocked,
        crate::domain::entities::SongAvailability::PendingDelete => {
            PlaybackRestoreDisposition::Drop
        }
    }
}

/// A blocked queue entry may be retried only when it belongs to the active
/// root and has become playable again.
#[must_use]
pub fn is_playable_in_active_root(active_root: Option<LibraryRootId>, song: Option<&Song>) -> bool {
    matches!(
        (active_root, song),
        (Some(root), Some(song)) if song.root() == root && song.availability().is_playable()
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::entities::Song;
    use crate::domain::ids::{RelativeMediaPath, Revision, SongId};

    fn song(root: LibraryRootId) -> Song {
        Song::new(
            SongId::new(),
            root,
            RelativeMediaPath::new("music/song.flac").expect("safe test path"),
            Revision::INITIAL,
        )
    }

    #[test]
    fn restore_disposition_covers_available_missing_pending_and_absent_or_foreign() {
        let root = LibraryRootId::new();
        let other_root = LibraryRootId::new();
        let available = song(root);
        assert_eq!(
            playback_restore_disposition(Some(root), Some(&available)),
            PlaybackRestoreDisposition::Restore
        );

        let mut missing = song(root);
        missing.mark_missing();
        assert_eq!(
            playback_restore_disposition(Some(root), Some(&missing)),
            PlaybackRestoreDisposition::Blocked
        );

        let mut pending = song(root);
        pending.begin_pending_delete();
        assert_eq!(
            playback_restore_disposition(Some(root), Some(&pending)),
            PlaybackRestoreDisposition::Drop
        );
        assert_eq!(
            playback_restore_disposition(Some(root), Some(&song(other_root))),
            PlaybackRestoreDisposition::Drop
        );
        assert_eq!(
            playback_restore_disposition(Some(root), None),
            PlaybackRestoreDisposition::Drop
        );
    }

    #[test]
    fn retry_requires_an_active_root_and_available_song() {
        let root = LibraryRootId::new();
        let mut missing = song(root);
        missing.mark_missing();
        assert!(!is_playable_in_active_root(Some(root), Some(&missing)));
        missing.restore_available();
        assert!(is_playable_in_active_root(Some(root), Some(&missing)));
        assert!(!is_playable_in_active_root(None, Some(&missing)));
        assert!(!is_playable_in_active_root(
            Some(LibraryRootId::new()),
            Some(&missing)
        ));
    }
}
