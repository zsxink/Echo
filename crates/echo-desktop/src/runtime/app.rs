//! The desktop composition root assembly (task 7.3 wiring, design §10).
//!
//! Until this module, `AppServices` existed but nothing constructed it with
//! real dependencies: @unsafe test fakes assembled `ScanDeps`. This module
//! assembles a production [`ScanDeps`] over the real SQLite database, the
//! root-constrained file system and the real metadata/probe/lyrics/cover
//! adapters, plus the production clock and id generator (infrastructure/core).
//!
//! It deliberately does **not** own the Tauri runtime, libmpv, or OS dialog
//! boundaries (those remain the app-shell/platform-gate responsibilities): it
//! hands the caller a shared [`RootRegistry`] and a ready [`Arc<ScanDeps>`] so
//! [`crate::runtime::services::AppServices::with_runtime`] can be built and the
//! Tauri command layer spawned on top.
//!
//! Rules honoured here (CODE_STANDARDS §3): no SQL/fs escapes this layer into
//! the UI, and no adapter ever returns an absolute path — the registry keeps
//! root-id → directory binding internal to file-system adapters.

use std::path::Path;
use std::sync::Arc;

use echo_core::application::ports::LibraryRepository;
use echo_core::application::scan::ScanDeps;
use echo_core::error::Error;
use echo_core::infrastructure::core::{UuidV4Generator, WallClock};
use echo_core::infrastructure::filesystem::{
    Blake3ContentHasher, RootConstrainedFileSystem, RootRegistry,
};
use echo_core::infrastructure::metadata::{
    DiskCoverCache, LoftyMetadataReader, LrcLyricsParser, SymphoniaMediaProbe,
};
use echo_core::infrastructure::sqlite::SqliteDatabase;

/// The DB backing directory and the shared root registry for one installation.
///
/// The composition root owns a single [`RootRegistry`] and passes it by `Clone`
/// to every file-system-bound adapter (fs/probe/metadata/hasher) so all of them
/// resolve a root id to the same directory, and hands it to
/// [`AppServices::with_runtime`] by value afterwards.
pub struct RoutedRuntime {
    /// Shared root-id → directory binding.
    pub registry: RootRegistry,
    /// Fully assembled, production `ScanDeps` for `AppServices`.
    pub deps: Arc<ScanDeps>,
    /// The database opened over `db_path`; also serves as the runtime-state
    /// store (`RuntimeStateStore` implementation) that `AppServices::with_runtime`
    /// persists the root epoch through.
    pub database: Arc<SqliteDatabase>,
}

/// Assemble the production `ScanDeps`.
///
/// `db_path` is the SQLite database file (created if absent); `cover_cache_dir`
/// is the directory for the disk cover cache. Returns the ready deps plus the
/// shared registry.
///
/// # Errors
///
/// `Io` when the database or cover cache cannot be opened; `Storage` when
/// migrations/integrity fail.
pub fn assemble(db_path: &Path, cover_cache_dir: &Path) -> Result<RoutedRuntime, Error> {
    let database = Arc::new(SqliteDatabase::open(db_path)?);
    let registry = RootRegistry::new();

    // The registry is process-local, while the active root lives in SQLite.
    // Rebind it before recovery and IPC open so scans, probes and playback can
    // resolve an existing library immediately after an application restart.
    if let Some(root) = database.active_root()? {
        registry.register(root.id(), root.absolute_path());
    }

    // The disk cover cache needs its own directory and a real capacity.
    let cover_cache = Arc::new(DiskCoverCache::new(cover_cache_dir)?);

    let deps = Arc::new(ScanDeps {
        roots: database.clone(),
        songs: database.clone(),
        catalog: database.clone(),
        playlists: database.clone(),
        lyrics: database.clone(),
        covers: database.clone(),
        runs: database.clone(),
        uow: database.clone(),
        journal: database.clone(),
        // File-system-bound adapters all share the registry.
        fs: Arc::new(RootConstrainedFileSystem::new(registry.clone())),
        probe: Arc::new(SymphoniaMediaProbe::new(registry.clone())),
        metadata: Arc::new(LoftyMetadataReader::new(registry.clone())),
        hasher: Arc::new(Blake3ContentHasher::new(registry.clone())),
        lyrics_parser: Arc::new(LrcLyricsParser),
        cover_cache,
        ids: Arc::new(UuidV4Generator),
        clock: Arc::new(WallClock::new()),
        config: Default::default(),
    });

    Ok(RoutedRuntime {
        registry,
        deps,
        database,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use echo_core::domain::entities::LibraryRoot;

    fn dirs() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("covers");
        (dir, cache)
    }

    #[test]
    fn assemble_builds_a_real_scan_deps_over_a_fresh_db() {
        let (dir, cache) = dirs();
        // `assemble` returning ok is the wiring proof: the real SQLite database
        // applies migrations and passes its quick-check, the disk cover cache
        // below opens its directory, and every fs/probe/metadata/hasher/lyrics
        // adapter constructs with the shared registry — all headless.
        let routed = assemble(&dir.path().join("echo.sqlite"), &cache).unwrap();
        // The deps must be usable across the runtime/command boundary.
        fn assert_send_sync<T: Send + Sync>(_: &T) {}
        assert_send_sync(routed.deps.as_ref());
    }

    #[test]
    fn assemble_registers_a_root_that_all_adapters_resolve_consistently() {
        let (dir, cache) = dirs();
        let routed = assemble(&dir.path().join("echo.sqlite"), &cache).unwrap();
        let root_id = routed.deps.ids.new_library_root_id();
        routed.registry.register(root_id, dir.path());
        // A newly registered empty root resolves to the directory the fs
        // adapter was bound to (no cross-root escape): path_of(root) = dir.
        let resolved = routed.registry.path_of(root_id).unwrap();
        assert_eq!(resolved, dir.path());
    }

    #[test]
    fn assemble_rebinds_an_empty_persisted_active_root_after_restart() {
        let (data, cache) = dirs();
        let db_path = data.path().join("echo.sqlite");
        let library = data.path().join("empty-library");
        std::fs::create_dir(&library).expect("empty library directory");

        let first = assemble(&db_path, &cache).expect("first assembly");
        let root_id = first.deps.ids.new_library_root_id();
        LibraryRepository::upsert(
            first.database.as_ref(),
            &LibraryRoot::new(root_id, library.clone(), true, true),
        )
        .expect("persist active root");
        drop(first);

        let restarted = assemble(&db_path, &cache).expect("restart assembly");
        assert_eq!(
            restarted
                .registry
                .path_of(root_id)
                .expect("active root rebound"),
            library,
            "an existing empty library keeps its root identity and path binding"
        );
    }
}
