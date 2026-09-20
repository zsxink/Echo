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
use echo_core::application::ports::{ProbeOutcome, RuntimeStateStore};
use echo_core::application::root_switch::Blockers;
use echo_core::application::scan::{ScanDeps, ScanSupervisor};
use echo_core::domain::catalog::{PlaybackContextRequest, SongSort, ViewRef};
use echo_core::domain::entities::LyricsCandidate;
use echo_core::domain::ids::{PlaylistId, SongId};
use echo_core::domain::playback_restore::{
    is_playable_in_active_root, playback_restore_disposition, PlaybackRestoreDisposition,
};
use echo_core::error::Error;
use echo_core::infrastructure::filesystem::RootRegistry;

use crate::ipc::dto::BootstrapSnapshot;
use crate::platform::dialogs::SystemDialogs;
use crate::player::queue::{TemporaryLyricLine, TemporaryLyrics, TemporaryMetadata};
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
    /// The one and only startup supervisor: the shell `manage`s this very
    /// `Arc`, so the readiness gate and the file-open delivery gate that
    /// `deliver_file_open` reads are the same state this composition root
    /// exposes ([`Self::startup`]). A second, separately-constructed instance
    /// would let the two gates diverge silently
    /// (deliver-file-opens-after-frontend-ready, D3).
    startup: std::sync::Arc<StartupSupervisor>,
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
    /// Read presentation metadata for a file opened outside the library.
    ///
    /// This intentionally does not create a song row. It only gives the
    /// session-only player enough data to render the same title/artist/album,
    /// cover and lyrics that a scanned library song would expose.
    #[must_use]
    pub fn read_temporary_metadata(&self, path: &std::path::Path) -> TemporaryMetadata {
        let Ok(bytes) = std::fs::read(path) else {
            return TemporaryMetadata::default();
        };
        let parsed = self.deps.metadata.read_bytes(&bytes).ok();
        let Some(parsed) = parsed else {
            return TemporaryMetadata::default();
        };

        // Duration is probe-owned — the tag reader deliberately leaves it to
        // the probe, which for a library song runs during the scan. A file
        // opened from outside the library never gets that pass, so it is
        // probed here: without it the queue row reads 时长未知 while the
        // progress bar, fed by the engine, already shows the real length.
        // Best-effort by design: a file that will not parse keeps the rest of
        // its metadata and plays with an unknown length.
        let duration = self
            .deps
            .probe
            .probe_bytes(&bytes, path.extension().and_then(std::ffi::OsStr::to_str))
            .ok()
            .and_then(|outcome| match outcome {
                ProbeOutcome::Audio { duration, .. } => duration,
                ProbeOutcome::NoAudioTrack | ProbeOutcome::Unsupported => None,
            });

        let cover_key = parsed
            .cover
            .as_ref()
            .and_then(|cover| self.deps.cover_cache.put(&cover.bytes, &cover.mime).ok());
        let embedded = parsed
            .embedded_lyrics
            .as_deref()
            .map(|raw| self.deps.lyrics_parser.parse(raw))
            .filter(LyricsCandidate::is_effective)
            .map(|candidate| ("embedded", candidate));
        let lyrics = embedded.or_else(|| {
            let sidecar = path.with_extension("lrc");
            let raw = std::fs::read(&sidecar).ok()?;
            if raw.len() > self.deps.config.lyrics_limit {
                return None;
            }
            let text = String::from_utf8(raw).ok()?;
            let candidate = self.deps.lyrics_parser.parse(&text);
            candidate.is_effective().then_some(("sidecar", candidate))
        });

        TemporaryMetadata {
            title: parsed.title,
            artist: parsed.artist,
            album: parsed.album,
            duration: duration.map(|duration| duration.as_secs_f64()),
            cover_key,
            lyrics: lyrics.map(|(source, candidate)| TemporaryLyrics {
                source: Some(source.to_owned()),
                timed: !candidate.lines().is_empty(),
                lines: candidate
                    .lines()
                    .iter()
                    .map(|line| {
                        // Lyrics timestamps are milliseconds, and the f64
                        // mantissa only caps out at ~285 000 years of audio.
                        #[allow(clippy::cast_precision_loss)]
                        let seconds = line.timestamp_ms as f64 / 1000.0;
                        TemporaryLyricLine {
                            seconds,
                            text: line.text.clone(),
                        }
                    })
                    .collect(),
                plain_text: candidate.plain_text().unwrap_or_default().to_owned(),
                parse_error: candidate.parse_error().map(ToOwned::to_owned),
            }),
        }
    }

    /// Resolve a complete, deterministic library view through Core. The
    /// desktop only adapts the command parameters and returns Core's ordered
    /// context to its player coordinator.
    ///
    /// # Errors
    ///
    /// `Validation` when `view` names no known library view; `Conflict` when
    /// `selected` is not part of the resolved view (including when the recent
    /// filter excludes it); catalog failures propagate.
    pub fn resolve_library_playback_context(
        &self,
        view: &str,
        query: &str,
        sort: SongSort,
        selected: SongId,
    ) -> Result<Vec<SongId>, Error> {
        let request = PlaybackContextRequest::library_view(view, query, sort, selected)?;
        Ok(ResolvePlaybackContext::new(self.deps.catalog.as_ref())
            .run(&request)?
            .songs)
    }

    /// Resolve a playlist's visible order through the shared Core use case.
    ///
    /// # Errors
    ///
    /// `Conflict` when `selected` is not a member of the playlist; catalog
    /// failures propagate.
    pub fn resolve_playlist_playback_context(
        &self,
        playlist: PlaylistId,
        selected: SongId,
    ) -> Result<Vec<SongId>, Error> {
        Ok(ResolvePlaybackContext::new(self.deps.catalog.as_ref())
            .run(&PlaybackContextRequest::new(
                ViewRef::Playlist { id: playlist },
                SongSort::default(),
                selected,
            ))?
            .songs)
    }

    /// Classify a persisted player session against the active library without
    /// exposing paths to the `WebView`. Missing media is retryable/blocked;
    /// absent, foreign-root and Echo-pending-delete identities are dropped.
    #[must_use]
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
    #[must_use]
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

    /// Resolve a system-opened file to an active-library identity when it is
    /// already managed. The absolute path never crosses the IPC boundary or
    /// reaches Core; it is used only by this desktop adapter to choose between
    /// a one-item library queue and a session-only temporary item.
    ///
    /// # Errors
    ///
    /// Propagates active-root and song-repository read failures.
    pub fn active_song_for_path(&self, path: &std::path::Path) -> Result<Option<SongId>, Error> {
        let Some(root) = self.deps.roots.active_root()? else {
            return Ok(None);
        };
        let opened = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        for song in self.deps.songs.all_in_root(root.id())? {
            let candidate = root.resolve_song_path(&song)?;
            let candidate = candidate.canonicalize().unwrap_or(candidate);
            if candidate == opened {
                return Ok(Some(song.id()));
            }
        }
        Ok(None)
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
            startup: std::sync::Arc::new(startup),
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
    ///
    /// `startup` is the supervisor the shell already `manage`s: the caller
    /// passes its own `Arc`, so the gate this root reads is the gate the shell
    /// reads (deliver-file-opens-after-frontend-ready, D3).
    #[must_use]
    pub fn with_runtime(
        deps: std::sync::Arc<ScanDeps>,
        supervisor: ScanSupervisor,
        startup: std::sync::Arc<StartupSupervisor>,
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

    /// The shared startup gate (runtime wires it in before IPC opens). This is
    /// the shell-managed instance, not a copy: the shell's file-open delivery
    /// gate and this composition root's readiness gate are one object.
    #[must_use]
    pub fn startup(&self) -> &StartupSupervisor {
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
