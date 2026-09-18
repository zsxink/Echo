//! One shared in-memory backend implementing every repository port *and* the
//! unit of work coherently — the fake counterpart of one SQLite database.
//!
//! The per-port `Memory*` doubles stay available for isolated unit tests, but
//! use-case tests need the same guarantee the real stack has: a transaction
//! commit is visible through every repository view. `MemoryDatabase` clones
//! share one store (like `SqliteDatabase` handles share one file).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use crate::application::ports::*;
use crate::domain::catalog::{CatalogCounts, OpaqueCursor, Paged, SongSort, RECENT_VIEW_LIMIT};
use crate::domain::entities::{
    LibraryRoot, LyricsCandidate, LyricsSource, MediaDiagnostic, PlaylistMember, Song,
    SongAvailability,
};
use crate::domain::ids::*;
use crate::domain::library::{DeviceId, HybridLogicalClock};
use crate::domain::state::scan::{ScanProgress, ScanState};
use crate::domain::text::playlist_name_key;
use crate::error::Error;

/// The shared transactional store.
#[derive(Clone, Debug, Default)]
struct Store {
    songs: BTreeMap<SongId, Song>,
    roots: BTreeMap<LibraryRootId, LibraryRoot>,
    playlists: BTreeMap<PlaylistId, (LibraryRootId, String, Option<String>)>,
    members: BTreeMap<(PlaylistId, SongId), PlaylistMember>,
    operations: BTreeMap<(OperationId, String), OperationItem>,
    envelopes: BTreeMap<OperationId, (LibraryRootId, String, Option<SongId>)>,
    undo_deadlines: BTreeMap<OperationId, i64>,
    released_claims: Vec<OperationId>,
    lyrics: BTreeMap<(SongId, LyricsSource), LyricsCandidate>,
    covers: BTreeMap<SongId, CoverAssetRef>,
    runs: BTreeMap<(LibraryRootId, u64), ScanRunRow>,
    issues: Vec<(LibraryRootId, u64, MediaDiagnostic)>,
    runtime_state: BTreeMap<String, String>,
    fail_root_isolation: bool,
    outbox_revisions: BTreeMap<(String, String), i64>,
    object_hlcs: BTreeMap<(String, String), HybridLogicalClock>,
    device_id: DeviceId,
}

/// The in-memory mirror of a `scan_runs` row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ScanRunRow {
    pub state: ScanState,
    pub progress: ScanProgress,
    pub finished: bool,
    pub updates: u64,
}

type Shared = Arc<Mutex<Store>>;

/// One fake database. Clone = same store.
#[derive(Clone, Debug, Default)]
pub struct MemoryDatabase {
    store: Shared,
    fail_commit: Arc<std::sync::atomic::AtomicBool>,
}

impl MemoryDatabase {
    #[must_use]
    pub fn new() -> Self {
        Self {
            store: Arc::new(Mutex::new(Store {
                device_id: DeviceId::from_uuid(uuid::Uuid::new_v4()),
                ..Store::default()
            })),
            fail_commit: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Store> {
        self.store
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Fail the next transaction commit (crash-injection point).
    pub fn set_fail_commit(&self, fail: bool) {
        self.fail_commit
            .store(fail, std::sync::atomic::Ordering::Relaxed);
    }

    /// Fail the root-isolation write inside a transaction (assertion hook for
    /// the trash unknown-outcome atomicity boundary).
    pub fn set_fail_root_isolation(&self, fail: bool) {
        self.lock().fail_root_isolation = fail;
    }

    /// All songs (assertion helper).
    #[must_use]
    pub fn songs(&self) -> Vec<Song> {
        self.lock().songs.values().cloned().collect()
    }

    /// One run row (assertion helper).
    #[must_use]
    pub fn run(&self, root: LibraryRootId, generation: u64) -> Option<ScanRunRow> {
        self.lock().runs.get(&(root, generation)).cloned()
    }

    /// Issues of one run (assertion helper).
    #[must_use]
    pub fn issues_of(&self, root: LibraryRootId, generation: u64) -> Vec<MediaDiagnostic> {
        self.lock()
            .issues
            .iter()
            .filter(|(r, g, _)| *r == root && *g == generation)
            .map(|(_, _, issue)| issue.clone())
            .collect()
    }

    /// Lyrics candidates of one song (assertion helper).
    #[must_use]
    pub fn lyrics_of(&self, song: SongId) -> Vec<LyricsCandidate> {
        self.lock()
            .lyrics
            .iter()
            .filter(|((s, _), _)| *s == song)
            .map(|(_, candidate)| candidate.clone())
            .collect()
    }

    /// Cover reference of one song (assertion helper).
    #[must_use]
    pub fn cover_of(&self, song: SongId) -> Option<CoverAssetRef> {
        self.lock().covers.get(&song).cloned()
    }

    /// Runtime state value (assertion helper).
    #[must_use]
    pub fn runtime_state(&self, key: &str) -> Option<String> {
        self.lock().runtime_state.get(key).cloned()
    }

    /// Operations whose claims were released (assertion helper).
    #[must_use]
    pub fn released_claims(&self) -> Vec<OperationId> {
        self.lock().released_claims.clone()
    }

    /// The journal envelope of one operation (assertion helper: the import
    /// must ensure the envelope before its first item write).
    #[must_use]
    pub fn envelope_of(&self, operation: OperationId) -> Option<(LibraryRootId, String)> {
        self.lock()
            .envelopes
            .get(&operation)
            .map(|(root, kind, _)| (*root, kind.clone()))
    }

    /// The journal operations reserved for one song (assertion helper for the
    /// external-missing vs Echo-delete distinction: external missing must
    /// create none, an Echo delete creates exactly one delete operation).
    #[must_use]
    pub fn operations_for_song(&self, song: SongId) -> Vec<OperationId> {
        self.lock()
            .envelopes
            .iter()
            .filter(|(_, (_, _, reserved))| *reserved == Some(song))
            .map(|(operation, _)| *operation)
            .collect()
    }

    /// The sync outbox rows of one object (assertion helper: recovery must
    /// converge to exactly one outbox row for the imported song).
    #[must_use]
    pub fn outbox_rows(&self, object_type: &str, object_uuid: &str) -> Vec<i64> {
        self.lock()
            .outbox_revisions
            .iter()
            .filter(|((kind, uuid), _)| kind == object_type && uuid == object_uuid)
            .map(|(_, revision)| *revision)
            .collect()
    }
}

// The per-port views and the unit of work live next to this file; each is a
// child module, so they share this module's imports and private `Store`.
mod catalog;
mod journal;
mod media;
mod repositories;
mod runtime;
mod transactions;

#[cfg(test)]
mod tests;
