//! Queue presentation-metadata resolver (task 2.2).
//!
//! Resolves and caches the title/artist/duration/cover of library queue
//! entries so the 10 Hz position stream never re-queries per row.

use std::sync::Arc;

use echo_core::application::ports::{CoverRepository, SongRepository};
use echo_core::domain::ids::SongId;

use super::{QueueEntryMeta, QueueItem};

/// Resolves and caches presentation metadata for library queue entries.
///
/// The resolver sits between the snapshot forwarder and the library: each
/// snapshot asks for the *current* set of song ids; the resolver answers from
/// its per-song cache and only queries the repository for ids it has not seen
/// (a queue change). Metadata for an id that no longer resolves (deleted /
/// foreign) is cached as `None` so a transient message cannot trigger a query
/// storm, and it stays that way until the process is restarted or the entry
/// is re-added to a fresh context.
pub struct QueueMetadataResolver {
    songs: Arc<dyn SongRepository>,
    covers: Arc<dyn CoverRepository>,
    // An unknown/deleted id is cached as `QueueEntryMeta::default()`, whose
    // fields serialize as explicit nulls. Keeping one value type avoids a
    // second "not found" representation and makes cache hits unambiguous.
    cache: std::sync::Mutex<std::collections::HashMap<SongId, QueueEntryMeta>>,
}

impl QueueMetadataResolver {
    /// A resolver over the same repository pair `AppServices` uses.
    #[must_use]
    pub fn new(songs: Arc<dyn SongRepository>, covers: Arc<dyn CoverRepository>) -> Self {
        Self {
            songs,
            covers,
            cache: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// Resolve the metadata of every library id in `queue`, batch-cached.
    ///
    /// Misses are resolved once and cached (as `QueueEntryMeta::default()` —
    /// explicit nulls — for ids that do not resolve); hits are answered from
    /// the cache, so repeated 10 Hz snapshots of an unchanged queue cost zero
    /// repository queries. Temporary items (no `song_id`) are never queried
    /// and never appear in the map (they already carry a display title).
    pub fn resolve(
        &self,
        queue: &[crate::player::queue::QueueEntry],
    ) -> std::collections::HashMap<SongId, QueueEntryMeta> {
        let mut result = std::collections::HashMap::new();
        let mut missing = std::collections::HashSet::new();
        {
            let cache = self.cache.lock().expect("metadata cache lock");
            for entry in queue {
                if let QueueItem::Library(id) = entry.item {
                    match cache.get(&id) {
                        Some(meta) => {
                            result.insert(id, meta.clone());
                        }
                        // Duplicate songs are distinct queue entries but share
                        // one library metadata record, so resolve each song id
                        // at most once per snapshot.
                        None => {
                            missing.insert(id);
                        }
                    }
                }
            }
        }

        // Resolve only the ids we have never seen (a queue change) — the
        // batched part of the contract. Deletion race: an entry dropped from
        // the library mid-queue resolves to explicit nulls, never an error.
        if !missing.is_empty() {
            let mut cache = self.cache.lock().expect("metadata cache lock");
            for id in &missing {
                let song = self.songs.by_id(*id).ok().flatten();
                let cover = song
                    .as_ref()
                    .and_then(|song| self.covers.cover_of(song.id()).ok().flatten());
                let meta = song
                    .map(|song| QueueEntryMeta {
                        title: song.title().map(ToOwned::to_owned),
                        artist: song.artist().map(ToOwned::to_owned),
                        duration_s: song.duration().map(|d| d.as_secs()),
                        cover_key: cover.map(|cover| cover.asset_key),
                    })
                    .unwrap_or_default();
                cache.insert(*id, meta.clone());
                result.insert(*id, meta);
            }
        }
        result
    }
}
