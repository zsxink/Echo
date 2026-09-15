//! Coarse-grained command services (task 7.3, design §10).
//!
//! `AppServices` is the desktop composition root for commands: it owns the
//! shared core dependencies ([`ScanDeps`], [`ScanSupervisor`]) and the startup
//! gate, and exposes the coarse commands the Tauri layer maps one-to-one.
//!
//! Rules honoured here:
//!
//! - **No raw SQL / fs / shell command from the UI** — the UI invokes these
//!   named commands only; every filesystem/DB operation stays inside `echo-core`.
//! - **Every mutation returns the committed revision/snapshot** — commands
//!   re-read the committed row through the same `SongId`/`PlaylistId` and return
//!   the authoritative `SongView`/`PagedSongs` the UI renders, so views, detail
//!   and now-playing share one state.
//! - **Writes obey the gate** — destructive operations return `Unavailable`
//!   until recovery resolves, and stay disabled on a read-only gate.

use echo_core::application::catalog::CatalogQuery;
use echo_core::application::delete::{DeleteSongs, RestoreDeletedOperation};
use echo_core::application::detail::{GetSongDetail, GetSongLyrics};
use echo_core::application::favorite::SetFavorite;
use echo_core::application::import::PlanImport;
use echo_core::application::playlist::{
    AddToPlaylists, CreatePlaylist, DeletePlaylist, PlaylistMembers, RemoveFromPlaylist,
    RenamePlaylist,
};
use echo_core::application::ports::{PlaylistRepository, RuntimeStateStore};
use echo_core::application::root_switch::{
    derive_root_id, ActivateLibrary, Blockers, PrepareLibraryCandidate,
};
use echo_core::application::scan::{CancelScan, ScanDeps, ScanSummary, ScanSupervisor, StartScan};
use echo_core::domain::catalog::{OpaqueCursor, SongSort};
use echo_core::domain::ids::{LibraryRootId, OperationId, PlaylistId, SongId};
use echo_core::error::Error;

use crate::ipc::dto::{
    BootstrapSnapshot, ImportBatchDto, ImportResultDto, LibraryCountsDto, LibraryRootStatusDto,
    LibraryStatus, PagedSongs, PlaylistView, RevealResultDto, ScanSnapshot, SongDetailView,
    SongView,
};
use crate::platform::dialogs::{RevealOutcome, SystemDialogs};
use crate::platform::import::SingleFileImport;
use crate::runtime::StartupSupervisor;
use echo_core::infrastructure::filesystem::RootRegistry;

/// A brand-new runtime-state store with no persisted values: a first launch has
/// no `root_epoch` yet, so a switch sees epoch 0 and advances to 1. The real
/// desktop composition root passes its durable `SQLite` store via
/// [`AppServices::with_runtime`]; this is the deterministic default used by the
/// test-only [`AppServices::new`] composition root.
#[cfg(test)]
#[derive(Clone, Copy, Debug, Default)]
struct DefaultRuntimeState;

#[cfg(test)]
impl RuntimeStateStore for DefaultRuntimeState {
    fn load(&self, _key: &str) -> Result<Option<String>, Error> {
        Ok(None)
    }
}

/// The desktop command surface. Thin over `echo-core` use cases; the Tauri
/// binary exposes these as commands with no added logic.
///
/// `dialogs` is the picker/reveal boundary (task 7.5): it keeps the `WebView`
/// from ever receiving a general filesystem capability or an absolute path.
pub struct AppServices {
    deps: std::sync::Arc<ScanDeps>,
    supervisor: ScanSupervisor,
    startup: StartupSupervisor,
    dialogs: std::sync::Arc<dyn SystemDialogs>,
    /// The root-id → absolute-path binding the file-system adapters resolve.
    /// The desktop owns it so candidate prepare/activate can bind a picked
    /// directory without Core ever storing or returning absolute paths.
    registry: RootRegistry,
    /// Runtime-persisted state (the `root_epoch` counter) read by root
    /// activation. Shares the datebase the repositories wrap.
    state: std::sync::Arc<dyn RuntimeStateStore>,
    /// In-flight import / delete-undo blockers the runtime feeds; a non-empty
    /// snapshot refuses a root switch (design §5.2).
    blockers: Blockers,
}

impl AppServices {
    /// Test-only composition root: a dialog boundary that always cancels, a
    /// fresh root registry, a fresh blocker registry and a brand-new (empty)
    /// runtime-state store. Production wiring goes through [`Self::with_runtime`]
    /// with the shell's real `SystemDialogs` adapter.
    #[cfg(test)]
    #[must_use]
    pub fn new(
        deps: std::sync::Arc<ScanDeps>,
        supervisor: ScanSupervisor,
        startup: StartupSupervisor,
    ) -> Self {
        Self {
            deps,
            supervisor,
            startup,
            dialogs: std::sync::Arc::new(crate::platform::dialogs::TestDialogs::cancelling()),
            registry: RootRegistry::new(),
            state: std::sync::Arc::new(DefaultRuntimeState),
            blockers: Blockers::new(),
        }
    }

    /// Assemble the composition root with the desktop's real dialog/reveal
    /// boundary (`choose_library_root`, `choose_and_import_files`,
    /// `reveal_song`), the shared root registry the runtime uses and a
    /// caller-provided runtime-state store + blocker registry.
    #[must_use]
    pub fn with_runtime(
        deps: std::sync::Arc<ScanDeps>,
        supervisor: ScanSupervisor,
        startup: StartupSupervisor,
        dialogs: std::sync::Arc<dyn SystemDialogs>,
        registry: RootRegistry,
        state: std::sync::Arc<dyn RuntimeStateStore>,
        blockers: Blockers,
    ) -> Self {
        Self {
            deps,
            supervisor,
            startup,
            dialogs,
            registry,
            state,
            blockers,
        }
    }

    /// The root registry owned by this composition root.
    #[must_use]
    pub const fn registry(&self) -> &RootRegistry {
        &self.registry
    }

    /// The blocker registry the runtime feeds (import in flight, delete-undo
    /// window open, unknown journal state).
    #[must_use]
    pub const fn blockers(&self) -> &Blockers {
        &self.blockers
    }

    /// The shared startup gate (runtime wires it in before IPC opens).
    pub const fn startup(&self) -> &StartupSupervisor {
        &self.startup
    }

    /// Reject a destructive op before it reaches core when the gate forbids
    /// writes (no active root, or a read-only journal outcome).
    fn guard_writes(&self) -> Result<(), Error> {
        if self.startup.writes_allowed() {
            Ok(())
        } else {
            Err(Error::unavailable(
                "library",
                "library is not ready for writes",
            ))
        }
    }

    /// `get_bootstrap_state`: the resolved snapshot of readiness.
    ///
    /// # Errors
    ///
    /// Never errors — readiness is reported as a snapshot, not an error.
    pub fn bootstrap(&self) -> Result<BootstrapSnapshot, Error> {
        let report = self.startup.report();
        Ok(BootstrapSnapshot {
            ready: self.startup.reads_allowed(),
            writes_allowed: self.startup.writes_allowed(),
            active_root: report.clone().and_then(|r| r.root.map(|id| id.to_string())),
            recovered_operations: report.map_or(0, |r| r.recovered_operations as u64),
        })
    }

    /// 全部歌曲, keyset-paginated, mapped to `PagedSongs`.
    ///
    /// # Errors
    ///
    /// `Unavailable` when there is no active root; `Conflict` for a stale
    /// cursor; storage errors propagate.
    pub fn all_songs(
        &self,
        sort: SongSort,
        cursor: Option<&OpaqueCursor>,
        limit: usize,
    ) -> Result<PagedSongs, Error> {
        let page = CatalogQuery::new(self.deps.catalog.as_ref()).all_songs(sort, cursor, limit)?;
        Ok(PagedSongs::from(page))
    }

    /// Search overlay on the active root.
    ///
    /// # Errors
    ///
    /// `Unavailable` when there is no active root; `Conflict` for a stale
    /// cursor; storage errors propagate.
    pub fn search(
        &self,
        query: &str,
        in_favorites: bool,
        sort: SongSort,
        cursor: Option<&OpaqueCursor>,
        limit: usize,
    ) -> Result<PagedSongs, Error> {
        let page = CatalogQuery::new(self.deps.catalog.as_ref()).search(
            query,
            in_favorites,
            sort,
            cursor,
            limit,
        )?;
        Ok(PagedSongs::from(page))
    }

    /// 喜欢的音乐 (keyset-paginated).
    ///
    /// # Errors
    ///
    /// `Unavailable` when there is no active root; `Conflict` for a stale
    /// cursor; storage errors propagate.
    pub fn favorites(
        &self,
        sort: SongSort,
        cursor: Option<&OpaqueCursor>,
        limit: usize,
    ) -> Result<PagedSongs, Error> {
        let page = CatalogQuery::new(self.deps.catalog.as_ref()).favorites(sort, cursor, limit)?;
        Ok(PagedSongs::from(page))
    }

    /// 资料库导航计数: one authoritative total per library view.
    ///
    /// The sidebar prints these next to views the user may never have opened,
    /// so they are answered by Core directly — never assembled from rows the
    /// desktop layer happens to have paged through.
    ///
    /// # Errors
    ///
    /// `Unavailable` when there is no active root; storage errors propagate.
    pub fn library_counts(&self) -> Result<LibraryCountsDto, Error> {
        let counts = CatalogQuery::new(self.deps.catalog.as_ref()).counts()?;
        Ok(LibraryCountsDto::from(counts))
    }

    /// 最近添加.
    ///
    /// # Errors
    ///
    /// `Unavailable` when there is no active root; storage errors propagate.
    pub fn recent(&self) -> Result<Vec<SongView>, Error> {
        let songs = CatalogQuery::new(self.deps.catalog.as_ref()).recent_100()?;
        Ok(songs.iter().map(SongView::from).collect())
    }

    /// List playlists of the active root with member counts.
    ///
    /// # Errors
    ///
    /// Storage errors propagate. Returns `Ok(vec![])` when no root is active.
    pub fn playlists(&self) -> Result<Vec<PlaylistView>, Error> {
        let root = self.deps.roots.active_root()?.map(|r| r.id());
        let Some(root) = root else {
            return Ok(Vec::new());
        };
        let repos = self.deps.playlists.as_ref();
        let ids = PlaylistRepository::list(repos, root)?;
        let mut views = Vec::with_capacity(ids.len());
        for id in ids {
            let name = PlaylistRepository::name(repos, id)?.unwrap_or_default();
            let count = repos.members(id)?.len();
            views.push(PlaylistView::from((id, name, count)));
        }
        Ok(views)
    }

    /// One playlist's members (available + missing shown, pending-delete hidden).
    ///
    /// # Errors
    ///
    /// Storage errors propagate.
    pub fn playlist_members(&self, playlist: PlaylistId) -> Result<Vec<SongView>, Error> {
        let rows = PlaylistMembers::new(self.deps.playlists.as_ref()).execute(playlist)?;
        let mut out = Vec::with_capacity(rows.len());
        for member in rows {
            if let Some(song) = self.deps.songs.by_id(member.song())? {
                out.push(SongView::from(&song));
            }
        }
        // Keep the position order the query returned.
        Ok(out)
    }

    /// Read-only song detail.
    ///
    /// # Errors
    ///
    /// `Unavailable` when the song is unknown; storage errors propagate.
    pub fn song_detail(&self, song: SongId) -> Result<SongDetailView, Error> {
        let detail = GetSongDetail::new(
            self.deps.songs.as_ref(),
            self.deps.lyrics.as_ref(),
            self.deps.covers.as_ref(),
        )
        .execute(song)?;
        Ok(SongDetailView::from(&detail))
    }

    /// Effective lyrics of a song (task 11.4–11.6): source, timed/plain lines,
    /// plain text and parse diagnostic. Line/path-free; the UI receives only
    /// the strongest non-corrupt candidate.
    ///
    /// # Errors
    ///
    /// `Unavailable` when the song is not in the library; storage errors propagate.
    pub fn get_lyrics(
        &self,
        song: SongId,
    ) -> Result<echo_core::application::detail::SongLyrics, Error> {
        GetSongLyrics::new(self.deps.lyrics.as_ref()).execute(song)
    }

    /// The opaque cover-asset keys of the given songs (design §16).
    ///
    /// Design §115 resolves artwork **内置优先**: the scan persists the artwork
    /// embedded in the audio file (and the `.lrc`-sidecar equivalent for
    /// lyrics), and that asset is what this returns. A song without embedded
    /// artwork is simply **absent** from the map — never a fabricated key — and
    /// the list keeps the prototype's palette placeholder for it.
    ///
    /// The values are the [`echo_core`] cover cache's opaque `cv1-…` identifiers,
    /// so the caller composes `cover://<key>` and never sees a filesystem path.
    /// Unknown ids are skipped rather than failing the batch: a stale row on
    /// screen must not break artwork for the rows that are still valid.
    ///
    /// # Errors
    ///
    /// Storage errors propagate; a read never consults the write gate.
    pub fn cover_keys(
        &self,
        song_ids: &[SongId],
    ) -> Result<std::collections::BTreeMap<String, String>, Error> {
        let mut keys = std::collections::BTreeMap::new();
        for song in song_ids {
            if let Some(cover) = self.deps.covers.cover_of(*song)? {
                keys.insert(song.to_string(), cover.asset_key);
            }
        }
        Ok(keys)
    }

    /// Toggle favorite; returns the authoritative committed song view.
    ///
    /// # Errors
    ///
    /// `Unavailable` when writes are disabled or the song is not toggleable;
    /// storage errors propagate.
    pub fn set_favorite(&self, song: SongId, favorite: bool) -> Result<SongView, Error> {
        self.guard_writes()?;
        let result = SetFavorite::new(self.deps.songs.as_ref()).execute(song, favorite)?;
        Ok(SongView::from(&result.song))
    }

    /// Create a playlist; returns its id.
    ///
    /// # Errors
    ///
    /// `Validation` for an invalid name; `Conflict` for a duplicate name;
    /// `Unavailable` when writes are disabled; storage errors propagate.
    pub fn create_playlist(&self, root: LibraryRootId, name: &str) -> Result<String, Error> {
        self.guard_writes()?;
        Ok(CreatePlaylist::new(self.deps.playlists.as_ref())
            .execute(root, name)?
            .to_string())
    }

    /// Rename a playlist (members preserved).
    ///
    /// # Errors
    ///
    /// `Validation`/`Conflict` for an invalid or colliding name; `Unavailable`
    /// for an unknown playlist or disabled writes; storage errors propagate.
    pub fn rename_playlist(&self, id: PlaylistId, name: &str) -> Result<(), Error> {
        self.guard_writes()?;
        RenamePlaylist::new(self.deps.playlists.as_ref()).execute(id, name)
    }

    /// Delete a playlist (songs untouched).
    ///
    /// # Errors
    ///
    /// `Unavailable` when writes are disabled; storage errors propagate.
    pub fn delete_playlist(&self, id: PlaylistId) -> Result<(), Error> {
        self.guard_writes()?;
        DeletePlaylist::new(self.deps.playlists.as_ref()).execute(id)
    }

    /// Add one song to one or more playlists atomically.
    ///
    /// # Errors
    ///
    /// `Unavailable` when writes are disabled or a target playlist does not
    /// exist; storage errors propagate.
    pub fn add_to_playlists(&self, song: SongId, targets: &[PlaylistId]) -> Result<(), Error> {
        self.guard_writes()?;
        AddToPlaylists::new(
            self.deps.songs.as_ref(),
            self.deps.playlists.as_ref(),
            self.deps.uow.as_ref(),
        )
        .execute(song, targets, u64::MAX)
    }

    /// Echo delete one song; returns the undo-operation id.
    ///
    /// # Errors
    ///
    /// `Validation`/`Conflict` for an unknown or non-deletable song; `Unavailable`
    /// when writes are disabled or the root is read-only; storage errors propagate.
    pub fn delete_song(&self, root: LibraryRootId, song: SongId) -> Result<String, Error> {
        self.guard_writes()?;
        Ok(DeleteSongs::new(self.deps.as_ref())
            .delete(root, song)?
            .operation
            .to_string())
    }

    /// Undo a delete within its 10-second window; returns the restored `SongId`.
    ///
    /// # Errors
    ///
    /// `Conflict` when the undo window expired; `InvariantViolation` for a
    /// non-delete operation; `Unavailable` when writes are disabled; storage
    /// errors propagate.
    pub fn undo_delete(
        &self,
        root: LibraryRootId,
        operation: OperationId,
    ) -> Result<String, Error> {
        self.guard_writes()?;
        Ok(RestoreDeletedOperation::new(self.deps.as_ref())
            .restore(root, operation)?
            .to_string())
    }

    /// A read-only snapshot of the library's availability + write capability.
    /// This never mutates; it answers "is the library usable, and can I write?"
    /// without exposing any absolute path.
    ///
    /// # Errors
    ///
    /// Storage errors propagate.
    pub fn library_status(&self) -> Result<LibraryStatus, Error> {
        let root = self.deps.roots.active_root()?;
        let scanning = root
            .as_ref()
            .is_some_and(|root| self.supervisor.is_scanning(root.id()));
        Ok(LibraryStatus {
            configured: root.is_some(),
            read_only: root.as_ref().is_some_and(|root| !root.write_capable()),
            unavailable: root.as_ref().is_some_and(|root| {
                !matches!(
                    root.availability(),
                    echo_core::domain::entities::RootAvailability::Available
                )
            }),
            scanning,
            active_root: root.map(|root| root.id().to_string()),
        })
    }

    /// Run the directory picker and, on a confirmed selection, prepare and
    /// activate the chosen library root (design §5). The whole flow — pick,
    /// candidate scan, activation barrier — stays on the Rust/desktop side; the
    /// UI receives only a path-free, relative/none outcome. A cancelled dialog
    /// returns `Ok(None)` — never an empty or fake success.
    ///
    /// # Errors
    ///
    /// - `Unavailable` when the picker errored or the candidate could not be
    ///   prepared (unreadable, scan failed) — the old active root is untouched.
    /// - `Conflict` when a root switch is blocked by active operations
    ///   (in-flight import / delete-undo window) or the candidate's scan
    ///   failed (resolve and re-prepare before activating).
    /// - `InvariantViolation` if the assembled runtime-state store is missing —
    ///   the desktop's real stack always provides it.
    /// - Storage errors propagate.
    pub fn choose_library_root(&self) -> Result<Option<LibraryRootStatusDto>, Error> {
        let Some(absolute) = self.dialogs.pick_library_directory()? else {
            // Cancelled: not an error, not a success — a genuine no-op.
            return Ok(None);
        };

        // Bind the directory under its derivable root id so the file-system
        // adapters can resolve it during the candidate scan (Core never stores
        // the absolute path beyond the root record).
        let canonical = absolute.canonicalize().map_err(|source| {
            Error::io("resolve chosen library directory", source, absolute.clone())
        })?;
        let root_id = derive_root_id(&canonical);
        self.registry.register(root_id, canonical.clone());

        // Candidate scan + two-phase activation.
        let prepare =
            PrepareLibraryCandidate::new(&self.deps, self.deps.roots.as_ref(), &self.supervisor)
                .prepare(&canonical)?;
        if !self.startup.writes_allowed() {
            // A read-only gate (recovery left an indeterminate outcome, or the
            // runtime has not resolved) still allows reads on the *current*
            // root, but a root *switch* is refused: activating would advance the
            // persisted epoch over a library whose operations are unresolved.
            // The old root keeps serving (spec: 切换失败保留旧库).
            return Err(Error::unavailable(
                "library",
                "library is not in a switchable state",
            ));
        }
        let _outcome = ActivateLibrary::new(
            &self.deps,
            self.deps.roots.as_ref(),
            &self.supervisor,
            self.state.as_ref(),
            &self.blockers,
        )
        .activate(root_id)?;

        let status = LibraryRootStatusDto {
            configured: true,
            read_only: !prepare.write_capable,
            active_root: root_id.to_string(),
        };
        Ok(Some(status))
    }

    /// Start a full scan of `root`. Blocking: the Tauri layer calls this on a
    /// worker thread. Returns the terminal summary.
    ///
    /// # Errors
    ///
    /// `Conflict` when a scan is already running for the root; root-level scan
    /// failures propagate.
    pub fn start_scan(&self, root: LibraryRootId) -> Result<ScanSnapshot, Error> {
        let summary: ScanSummary = StartScan::new(&self.deps, &self.supervisor).run(root)?;
        Ok(ScanSnapshot::from(&summary))
    }

    /// Cancel the active scan of `root`; `true` when one was cancelled.
    pub fn cancel_scan(&self, root: LibraryRootId) -> bool {
        CancelScan::new(&self.supervisor).cancel(root)
    }

    /// Remove `song` from `playlist`; removing a non-member is a no-op success.
    ///
    /// # Errors
    ///
    /// `Unavailable` when writes are disabled; storage errors propagate.
    pub fn remove_playlist_song(&self, playlist: PlaylistId, song: SongId) -> Result<(), Error> {
        self.guard_writes()?;
        RemoveFromPlaylist::new(self.deps.playlists.as_ref()).execute(playlist, song)
    }

    /// Run the desktop import-file dialog and import the chosen files into the
    /// active root. The dialog runs entirely on the Rust/Tauri boundary; the
    /// `WebView` only ever receives the per-input result DTO (relative paths and
    /// user-safe errors).
    ///
    /// Returns `Ok(None)` when the user **cancelled** the dialog — a cancel is
    /// a no-op, never a successful (or failed) batch.
    ///
    /// # Errors
    ///
    /// `Unavailable` when writes are disabled or no root is active; only
    /// batch-level failures propagate — per-input problems become
    /// `ImportResultDto::Failed` so one bad file cannot abort the batch.
    pub fn choose_and_import_files(&self) -> Result<Option<ImportBatchDto>, Error> {
        self.guard_writes()?;
        let Some(picked) = self.dialogs.pick_audio_files()? else {
            // Cancelled: not an error, not a success — a genuine no-op.
            return Ok(None);
        };
        let root = self
            .deps
            .roots
            .active_root()?
            .map(|r| r.id())
            .ok_or_else(|| Error::unavailable("library", "no active library root"))?;
        if picked.sources.is_empty() {
            return Ok(Some(ImportBatchDto { results: vec![] }));
        }
        let report = PlanImport::new(self.deps.as_ref(), picked.reader.as_ref())
            .run(root, &picked.sources)?;
        Ok(Some(ImportBatchDto::from(report)))
    }

    /// Import a single file (by absolute, desktop-owned path) into the active
    /// library root (task 11.7: "import current temporary playback item").
    ///
    /// The path stays entirely desktop-side — it is never forwarded to the
    /// WebView. The caller (the Tauri command layer) extracts it from the
    /// coordinator's current queue entry and passes it here.
    ///
    /// Returns `Ok(result)` with the single-file import result (imported /
    /// duplicate / unsupported / failed) or `Err` when the root is unavailable
    /// or the path is unreadable.
    ///
    /// # Errors
    ///
    /// `Unavailable` when writes are disabled, no root is active, or the path
    /// is unreadable; infrastructure errors propagate; per-input problems
    /// become `ImportResultDto::Failed`.
    pub fn import_single_path(
        &self,
        absolute_path: &std::path::Path,
        display_name: &str,
    ) -> Result<ImportResultDto, Error> {
        self.guard_writes()?;
        let root_id = self
            .deps
            .roots
            .active_root()?
            .map(|r| r.id())
            .ok_or_else(|| Error::unavailable("library", "no active library root"))?;
        let reader = SingleFileImport::new(absolute_path, display_name)?;
        let source = reader.source().clone();
        let report = PlanImport::new(self.deps.as_ref(), &reader).run(root_id, &[source])?;
        // A single-source batch always has exactly one result.
        let outcome = report
            .results
            .into_iter()
            .next()
            .expect("import batch has at least one result");
        Ok(ImportResultDto::from(outcome))
    }

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
        let absolute = root.absolute_path().join(record.path().normalized());
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::str::FromStr;

    use echo_core::application::ports::LibraryRepository;
    use echo_core::application::testing::scan_fixture::ScanFixture;
    use echo_core::application::testing::small_fakes::FakeTrash;
    use echo_core::domain::entities::LibraryRoot;

    use crate::platform::dialogs::TestDialogs;
    use crate::runtime::StartupSupervisor;

    /// A writable `AppServices` over the in-memory fixture: an active, writable
    /// root plus a startup gate resolved to `Writable`, using the default
    /// (cancelling) dialogs port.
    fn services(fixture: &ScanFixture) -> AppServices {
        services_with_dialogs(fixture, TestDialogs::cancelling())
    }

    /// Like [`services`](self::services) but with an explicit dialogs port.
    fn services_with_dialogs(fixture: &ScanFixture, dialogs: TestDialogs) -> AppServices {
        LibraryRepository::upsert(
            &fixture.database,
            &LibraryRoot::new(fixture.root, "/library".into(), true, true),
        )
        .expect("active root");
        let startup = StartupSupervisor::new();
        startup
            .run_recovery(&fixture.deps, &fixture.supervisor, &FakeTrash::new())
            .expect("clean recovery");
        AppServices::with_runtime(
            std::sync::Arc::clone(&fixture.deps),
            ScanSupervisor::new(),
            startup,
            std::sync::Arc::new(dialogs),
            RootRegistry::new(),
            std::sync::Arc::new(fixture.database.clone()),
            Blockers::new(),
        )
    }

    /// A `ScanFixture` bound to a *fresh* directory, for the root-choice tests.
    /// The dialog returns `dir`; the services register it under its derivable
    /// root id (in the filesystem registry) and prepare/activate it.
    fn choose_root_fixture(dir: &std::path::Path) -> ScanFixture {
        let fixture = ScanFixture::new();
        // Bind the real directory in the fake filesystem so prepare/activate
        // can scan it. `prepare` itself creates the root record.
        let canonical = dir.canonicalize().expect("canonical dir");
        let root_id = derive_root_id(&canonical);
        fixture.fs.add_root_at(root_id, canonical);
        fixture
    }

    /// Seed `n` songs by writing files + scripting probe/metadata, then run a
    /// full scan so they land in the committed library.
    fn seed_songs(fixture: &ScanFixture, n: usize) -> Vec<SongId> {
        for index in 0..n {
            let path = format!("song-{index}.flac");
            fixture.write_file(&path, format!("audio-{index}").as_bytes());
            fixture.set_audio(&path, &format!("标题{index}"), 1_000);
        }
        StartScan::new(&fixture.deps, &fixture.supervisor)
            .run(fixture.root)
            .expect("scan seeds the library");
        fixture
            .all_songs()
            .iter()
            .map(echo_core::domain::entities::Song::id)
            .collect()
    }

    /// `cover_keys` answers with the embedded artwork of the songs that have
    /// one, keyed by an opaque cache key — and stays silent about the rest
    /// (design §115: a file with no embedded cover keeps the palette
    /// placeholder, it does not get a fabricated asset).
    #[test]
    fn cover_keys_returns_only_songs_with_embedded_artwork() {
        let fixture = ScanFixture::new();
        fixture.write_file("with-art.flac", b"audio-with-art");
        fixture.set_audio_with_cover("with-art.flac", "有封面", 1_000, &b"cover-".repeat(64));
        fixture.write_file("no-art.flac", b"audio-no-art");
        fixture.set_audio("no-art.flac", "无封面", 1_000);
        StartScan::new(&fixture.deps, &fixture.supervisor)
            .run(fixture.root)
            .expect("scan seeds the library");
        let app = services(&fixture);

        let songs = fixture.all_songs();
        let with_art = songs
            .iter()
            .find(|song| song.title() == Some("有封面"))
            .expect("song with artwork");
        let without_art = songs
            .iter()
            .find(|song| song.title() == Some("无封面"))
            .expect("song without artwork");

        let keys = app
            .cover_keys(&[with_art.id(), without_art.id()])
            .expect("cover keys");

        assert_eq!(keys.len(), 1, "only the song that carries artwork is keyed");
        let key = keys
            .get(&with_art.id().to_string())
            .expect("the song with embedded artwork has a key");
        // Whatever the cache emits must be resolvable by the `cover://`
        // boundary the WebView will hand it to — and that boundary rejects
        // anything that could be read as a path.
        assert!(
            crate::platform::security::CoverProtocol::is_valid_key(key),
            "cover key must pass the cover:// whitelist: {key}"
        );
        assert!(!keys.contains_key(&without_art.id().to_string()));
    }

    /// A stale or unknown id costs nothing: the batch still answers for the
    /// rows that are still valid, so one dead row cannot blank the window.
    #[test]
    fn cover_keys_skips_unknown_ids_instead_of_failing_the_batch() {
        let fixture = ScanFixture::new();
        fixture.write_file("with-art.flac", b"audio-with-art");
        fixture.set_audio_with_cover("with-art.flac", "有封面", 1_000, &b"cover-".repeat(64));
        StartScan::new(&fixture.deps, &fixture.supervisor)
            .run(fixture.root)
            .expect("scan seeds the library");
        let app = services(&fixture);

        let known = fixture.all_songs()[0].id();
        let keys = app
            .cover_keys(&[known, SongId::new()])
            .expect("an unknown id is skipped, not fatal");

        assert_eq!(keys.len(), 1);
        assert!(keys.contains_key(&known.to_string()));
    }

    #[test]
    fn set_favorite_returns_the_committed_authoritative_song() {
        let fixture = ScanFixture::new();
        let ids = seed_songs(&fixture, 1);
        let app = services(&fixture);

        let view = app
            .set_favorite(ids[0], true)
            .expect("favourite returns committed snapshot");
        assert!(view.favorite, "returned view reflects the committed write");

        // The same SongId drives the favorites view: it must now appear.
        let favs = app.favorites(SongSort::default(), None, 100).expect("favs");
        assert_eq!(favs.items.len(), 1);
        assert_eq!(favs.items[0].id, ids[0].to_string());
    }

    #[test]
    fn playlist_mutations_return_snapshots_seen_by_subsequent_reads() {
        let fixture = ScanFixture::new();
        let ids = seed_songs(&fixture, 2);
        let app = services(&fixture);

        let playlist = app
            .create_playlist(fixture.root, "我的歌单")
            .expect("create");
        let id = PlaylistId::from_str(&playlist).expect("valid playlist id");

        // Add both songs; the list and members reflect them committed.
        app.add_to_playlists(ids[0], &[id]).expect("add 1");
        app.add_to_playlists(ids[1], &[id]).expect("add 2");
        let members = app.playlist_members(id).expect("members");
        assert_eq!(members.len(), 2, "members commit in position order");

        let views = app.playlists().expect("list");
        assert_eq!(views.len(), 1);
        assert_eq!(views[0].member_count, 2);

        // Removing a member is reflected by the next read; a non-member is a
        // no-op success.
        app.remove_playlist_song(id, ids[0]).expect("remove");
        assert_eq!(app.playlist_members(id).expect("after").len(), 1);
        app.remove_playlist_song(id, ids[0]).expect("no-op"); // already gone
    }

    #[test]
    fn delete_and_undo_roundtrip_restores_the_song() {
        let fixture = ScanFixture::new();
        let ids = seed_songs(&fixture, 2);
        let app = services(&fixture);

        let operation = app
            .delete_song(fixture.root, ids[0])
            .expect("delete returns undo op");
        // The song is hidden from the catalog (pending-delete) right away.
        let gone = app.all_songs(SongSort::default(), None, 100).expect("all");
        assert!(
            gone.items.iter().all(|s| s.id != ids[0].to_string()),
            "pending-delete song hides from the catalog"
        );

        let restored = app
            .undo_delete(fixture.root, OperationId::from_str(&operation).expect("op"))
            .expect("undo restores");
        assert_eq!(restored, ids[0].to_string());
        let back = app
            .all_songs(SongSort::default(), None, 100)
            .expect("after");
        assert!(back.items.iter().any(|s| s.id == ids[0].to_string()));
    }

    #[test]
    fn library_status_reflects_configuration_and_scan_in_flight() {
        let fixture = ScanFixture::new();
        let app = services(&fixture);
        // Active writable root.
        let status = app.library_status().expect("status");
        assert!(status.configured);
        assert!(!status.read_only);
        assert!(!status.unavailable);
        assert_eq!(
            status.active_root.as_deref(),
            Some(fixture.root.to_string().as_str())
        );
    }

    #[test]
    fn cancelled_library_root_dialog_is_a_noop_never_a_success() {
        let fixture = ScanFixture::new();
        // Cancelling dialogs port; writes are allowed, so a real switch *could*
        // run — the cancel must still map to `None`, never an empty/fake root.
        let app = services(&fixture);
        assert!(app
            .choose_library_root()
            .expect("a cancelled dialog is not an error")
            .is_none());
    }

    #[test]
    fn choose_library_root_prepares_and_activates_a_directory() {
        let dir = tempfile::tempdir().expect("temp dir");
        let fixture = choose_root_fixture(dir.path());
        let dialogs = TestDialogs::with_directory(Some(dir.path().to_path_buf()));
        let app = services_with_dialogs(&fixture, dialogs);

        let maybe = app.choose_library_root().expect("choose");
        assert!(maybe.is_some(), "a confirmed directory is a success");
        let status = maybe.expect("some");
        assert!(status.configured);
        assert!(status.active_root != fixture.root.to_string());
        // The old default root is not active; the chosen one is.
        let active = fixture
            .database
            .active_root()
            .expect("read")
            .expect("now configured");
        assert_ne!(
            active.id(),
            fixture.root,
            "the picked root supersedes the default"
        );
        assert_eq!(active.id().to_string(), status.active_root);
    }

    #[test]
    fn choose_library_root_cancelled_preserves_previous_configuration() {
        let fixture = ScanFixture::new();
        let app = services(&fixture);
        // A cancelled dialog: old configuration intact.
        let maybe = app.choose_library_root().expect("cancel");
        assert!(maybe.is_none());
        let status = app.library_status().expect("status");
        assert!(status.configured, "previous active root preserved");
    }

    #[test]
    fn choose_library_root_is_refused_when_fs_is_unavailable() {
        let dir = tempfile::tempdir().expect("temp dir");
        // Simulate an unreadable directory (prepare fails, no activation). The fake
        // reproduces any injected fault as `Storage`; the contract is that a
        // root-level failure is surfaced and nothing activates — the candidate
        // record stays but is never the active root.
        let fixture = choose_root_fixture(dir.path());
        fixture
            .fs
            .inject_fault(Error::unavailable("library root", "unreadable"));
        let dialogs = TestDialogs::with_directory(Some(dir.path().to_path_buf()));
        // No pre-registered active root: a clean first-launch state.
        let startup = StartupSupervisor::new();
        let app = AppServices::with_runtime(
            std::sync::Arc::clone(&fixture.deps),
            ScanSupervisor::new(),
            startup,
            std::sync::Arc::new(dialogs),
            RootRegistry::new(),
            std::sync::Arc::new(fixture.database.clone()),
            Blockers::new(),
        );
        let err = app.choose_library_root().expect_err("prepare rejects");
        assert!(matches!(err, Error::Storage { .. }));
        let status = app.library_status().expect("status");
        assert_eq!(
            status.active_root, None,
            "a failed prepare never activates a root"
        );
    }

    #[test]
    fn destructive_commands_obey_the_write_gate() {
        // A gate-left-unresolved (no recovery yet) must refuse writes: the
        // coarse commands are thin but never bypass the readiness gate.
        let fixture = ScanFixture::new();
        let ids = seed_songs(&fixture, 1);
        let startup = StartupSupervisor::new(); // gate: None → writes disabled
        let app = AppServices::new(
            std::sync::Arc::clone(&fixture.deps),
            ScanSupervisor::new(),
            startup,
        );

        let err = app.set_favorite(ids[0], true).expect_err("writes disabled");
        assert!(
            matches!(err, Error::Unavailable { .. }),
            "the gate refuses writes before recovery resolves"
        );
        assert!(
            !app.cancel_scan(fixture.root),
            "no scan in flight on a fresh supervisor"
        );
    }

    #[test]
    fn cancelled_import_dialog_is_a_noop_never_a_success() {
        let fixture = ScanFixture::new();
        // `services()` resolves the gate to Writable and keeps the default
        // dialogs port (always cancel). Writes are allowed, so a real import
        // *could* run — the cancel must still map to `None`, never a fake
        // empty or successful batch.
        let app = services(&fixture);
        assert!(app
            .choose_and_import_files()
            .expect("a cancelled dialog is not an error")
            .is_none());
    }

    #[test]
    fn import_dialog_is_refused_before_the_write_gate_resolves() {
        let fixture = ScanFixture::new();
        let app = AppServices::new(
            std::sync::Arc::clone(&fixture.deps),
            ScanSupervisor::new(),
            StartupSupervisor::new(), // gate unresolved -> writes disabled
        );
        let err = app
            .choose_and_import_files()
            .expect_err("write gate blocks import before recovery");
        assert!(matches!(err, Error::Unavailable { .. }));
    }

    #[test]
    fn reveal_song_reveals_by_id_and_returns_only_the_relative_path() {
        let fixture = ScanFixture::new();
        let ids = seed_songs(&fixture, 1);
        let dialogs = TestDialogs::cancelling();
        let app = services_with_dialogs(&fixture, dialogs);

        let view = app.reveal_song(ids[0]).expect("reveal");
        // Never an absolute path: only the library-relative path reaches the UI.
        assert!(!view.relative_path.starts_with('/'));
        assert!(view.revealed);
        assert_eq!(view.song_id, ids[0].to_string());
    }
}
