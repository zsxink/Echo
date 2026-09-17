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
//!
//! The command surface is split into capability submodules
//! ([`catalog`], [`favorites`], [`playlists`], [`delete`], [`library`],
//! [`import`], [`reveal`]); every submodule's `impl AppServices` shares the
//! single dependency value owned by this struct, so no dependency parameter is
//! ever copied or threaded by hand.

use echo_core::application::playback_context::ResolvePlaybackContext;
use echo_core::application::ports::RuntimeStateStore;
use echo_core::application::root_switch::Blockers;
use echo_core::application::scan::{ScanDeps, ScanSupervisor};
use echo_core::domain::catalog::{PlaybackContextRequest, SongSort, ViewRef};
use echo_core::domain::ids::{PlaylistId, SongId};
use echo_core::domain::playback_restore::{
    is_playable_in_active_root, playback_restore_disposition, PlaybackRestoreDisposition,
};
use echo_core::error::Error;
use echo_core::infrastructure::filesystem::RootRegistry;

use crate::ipc::dto::BootstrapSnapshot;
use crate::platform::dialogs::SystemDialogs;
use crate::runtime::StartupSupervisor;

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
    /// Resolve a complete, deterministic library view through Core. The
    /// desktop only adapts the command parameters and returns Core's ordered
    /// context to its player coordinator.
    pub fn resolve_library_playback_context(
        &self,
        view: &str,
        query: &str,
        sort: SongSort,
        selected: SongId,
    ) -> Result<Vec<SongId>, Error> {
        let request = PlaybackContextRequest::library_view(view, query, sort, selected)?;
        Ok(ResolvePlaybackContext::new(self.deps.catalog.as_ref())
            .run(request)?
            .songs)
    }

    /// Resolve a playlist's visible order through the shared Core use case.
    pub fn resolve_playlist_playback_context(
        &self,
        playlist: PlaylistId,
        selected: SongId,
    ) -> Result<Vec<SongId>, Error> {
        Ok(ResolvePlaybackContext::new(self.deps.catalog.as_ref())
            .run(PlaybackContextRequest::new(
                ViewRef::Playlist { id: playlist },
                SongSort::default(),
                selected,
            ))?
            .songs)
    }

    /// Classify a persisted player session against the active library without
    /// exposing paths to the `WebView`. Missing media is retryable/blocked;
    /// absent, foreign-root and Echo-pending-delete identities are dropped.
    pub fn playback_restore_verdicts(
        &self,
        session: &crate::player::session::PlaybackSession,
    ) -> Vec<crate::player::session::RestoreVerdict> {
        let active = self
            .deps
            .roots
            .active_root()
            .ok()
            .flatten()
            .map(|root| root.id());
        session
            .entries
            .iter()
            .filter_map(|entry| entry.clone().parse().map(|(_, song)| song))
            .map(|song_id| {
                let song = self.deps.songs.by_id(song_id).ok().flatten();
                let disposition = match playback_restore_disposition(active, song.as_ref()) {
                    PlaybackRestoreDisposition::Restore => {
                        crate::player::session::RestoreDisposition::Restore
                    }
                    PlaybackRestoreDisposition::Blocked => {
                        crate::player::session::RestoreDisposition::Blocked
                    }
                    PlaybackRestoreDisposition::Drop => {
                        crate::player::session::RestoreDisposition::Drop
                    }
                };
                crate::player::session::RestoreVerdict {
                    song_id,
                    disposition,
                }
            })
            .collect()
    }

    /// True only for a currently active, available library song; used to make
    /// preserved blocked queue entries retryable after a recovery scan.
    pub fn playback_song_is_playable(&self, song_id: SongId) -> bool {
        let active = self
            .deps
            .roots
            .active_root()
            .ok()
            .flatten()
            .map(|root| root.id());
        is_playable_in_active_root(
            active,
            self.deps.songs.by_id(song_id).ok().flatten().as_ref(),
        )
    }

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
}

mod catalog;
mod delete;
mod favorites;
mod import;
mod library;
mod playlists;
mod reveal;

#[cfg(test)]
mod tests;
