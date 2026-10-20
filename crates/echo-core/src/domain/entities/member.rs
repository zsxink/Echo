//! Playlist member entity.

use crate::domain::ids::{PlaylistId, PlaylistItemId, SongId};

use super::song::SongAvailability;

// ---------------------------------------------------------------------------
// PlaylistMember
// ---------------------------------------------------------------------------

/// A playlist membership row.
///
/// Carries an independent, stable member identity ([`PlaylistItemId`]) in
/// addition to the song identity and its position, so member-level logic
/// changes (add / remove / reorder) can be tracked and propagated across
/// devices without rewriting the whole playlist record (playlist-management
/// spec: "每个成员关系 MUST 拥有独立、稳定的成员 UUID").
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct PlaylistMember {
    id: PlaylistItemId,
    playlist: PlaylistId,
    song: SongId,
    position: u64,
    song_availability: SongAvailability,
}

impl PlaylistMember {
    /// Construct a member with an explicit, stable member identity.
    #[must_use]
    pub const fn with_id(
        id: PlaylistItemId,
        playlist: PlaylistId,
        song: SongId,
        position: u64,
        song_availability: SongAvailability,
    ) -> Self {
        Self {
            id,
            playlist,
            song,
            position,
            song_availability,
        }
    }

    /// Construct a member, minting a fresh stable identity.
    #[must_use]
    pub fn new(
        playlist: PlaylistId,
        song: SongId,
        position: u64,
        song_availability: SongAvailability,
    ) -> Self {
        Self::with_id(
            PlaylistItemId::new(),
            playlist,
            song,
            position,
            song_availability,
        )
    }

    /// The stable member identity.
    #[must_use]
    pub const fn id(&self) -> PlaylistItemId {
        self.id
    }
    #[must_use]
    pub const fn playlist(&self) -> PlaylistId {
        self.playlist
    }
    #[must_use]
    pub const fn song(&self) -> SongId {
        self.song
    }
    #[must_use]
    pub const fn position(&self) -> u64 {
        self.position
    }
    #[must_use]
    pub const fn song_availability(&self) -> SongAvailability {
        self.song_availability
    }

    /// Mirror the song's availability (stale by at most one reconcile tick).
    pub const fn mirror_song(&mut self, availability: SongAvailability) {
        self.song_availability = availability;
    }
}
