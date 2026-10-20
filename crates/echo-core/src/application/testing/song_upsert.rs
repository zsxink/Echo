//! The one contract every in-memory song store must mirror: a **metadata**
//! upsert carries parsed metadata + scan facts, never user state.
//!
//! The real store says this in SQL — `statements::upsert_song`'s
//! `ON CONFLICT(uuid) DO UPDATE` deliberately omits `availability`,
//! `is_favorite` and `play_count`, because a caller may hold a stale snapshot
//! and must never roll user state backward. Those three move only through
//! their dedicated mutations (`set_song_availability`, `set_song_favorite`,
//! `set_play_count`).
//!
//! A double that overwrote the whole row would be structurally unable to fail
//! on any bug of that family — it would keep reporting green while the real
//! SQLite stack does the opposite. Every in-memory store funnels its song
//! writes through [`metadata_upsert`] so the divergence cannot come back.

use crate::domain::entities::{Song, SongAvailability};
use crate::error::Error;

/// Mirror the guarded local-only file-index write without generating a shared
/// mutation or carrying a stale user-state snapshot into the stored song.
pub fn refresh_file_index(stored: &mut Song, incoming: &Song) -> Result<(), Error> {
    if stored.root() != incoming.root()
        || stored.path().identity_key() != incoming.path().identity_key()
        || stored.blake3_hash() != incoming.blake3_hash()
        || stored.file_size() != incoming.file_size()
        || stored.availability() != SongAvailability::Available
    {
        return Err(Error::conflict(
            "published song association changed during metadata indexing",
        ));
    }
    let (Some(hash), Some(size), Some(mtime), Some(format)) = (
        incoming.blake3_hash(),
        incoming.file_size(),
        incoming.file_mtime_ns(),
        incoming.format(),
    ) else {
        return Err(Error::conflict(
            "published song index has incomplete file facts",
        ));
    };
    stored.apply_scan_facts_with_params(
        hash.to_owned(),
        size,
        mtime,
        format,
        incoming.audio_parameters(),
    );
    Ok(())
}

/// Fold `incoming` onto the stored row (if any) exactly the way the SQL
/// upsert does: new rows take the incoming state as written, existing rows
/// keep their availability, favorite flag and play count.
#[must_use]
pub fn metadata_upsert(existing: Option<&Song>, incoming: &Song) -> Song {
    let Some(stored) = existing else {
        return incoming.clone();
    };
    let availability = stored.availability();
    let favorite = stored.favorite();
    let plays = stored.play_count();
    let mut merged = incoming.clone();
    match availability {
        SongAvailability::Available => merged.restore_available(),
        SongAvailability::Missing => merged.mark_missing(),
        SongAvailability::PendingDelete => merged.begin_pending_delete(),
    }
    merged.set_favorite(favorite);
    merged.restore_play_count(plays);
    merged
}
