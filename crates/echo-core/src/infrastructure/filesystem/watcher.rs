//! The `notify`-backed [`FileEventSource`](crate::application::ports::FileEventSource)
//! with per-path debounce and file-stability double sampling (task 4.9).
//!
//! Raw watcher events are normalized by a [`Coalescer`], then:
//!
//! - **Debounced per identity path**: further events for the same path inside
//!   the debounce window replace the pending one (a duplicate or a quick
//!   create+modify burst becomes one event).
//! - **Double-sampled for stability**: a created/modified file is emitted only
//!   after its size/mtime stayed identical across two samples taken
//!   `sample_delay` apart — Echo never parses a half-written file.
//! - **Overflow degrades to a rescan signal**: a watcher queue overflow or an
//!   unclassifiable one-sided rename emits [`FileEventKind::RescanNeeded`]
//!   instead of pretending the stream is complete.
//!
//! The coalescing rules live in [`Coalescer`] (unit-testable without threads);
//! the notify wiring is a thin loop around it.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use notify::Watcher as _;

use crate::application::ports::{
    FileEvent, FileEventKind, FileEventSource, FileEventSubscription, RESCAN_SENTINEL,
};
use crate::domain::ids::{LibraryRootId, RelativeMediaPath};
use crate::domain::library::MEDIA_ROOT;
use crate::error::Error;

use super::registry::RootRegistry;

/// Debounce/sampling timings (production defaults; tests shrink them).
#[derive(Clone, Copy, Debug)]
pub struct WatcherTimings {
    /// How long a path must stay quiet before its pending event is emitted.
    pub debounce: Duration,
    /// Distance between the two stability samples.
    pub sample_delay: Duration,
    /// How often the coalescing thread re-checks its state.
    pub tick: Duration,
}

impl Default for WatcherTimings {
    fn default() -> Self {
        Self {
            debounce: Duration::from_millis(250),
            sample_delay: Duration::from_millis(120),
            tick: Duration::from_millis(100),
        }
    }
}

/// Debounce + double-sampling state for one subscription. Thread-free by
/// design: the notify loop feeds [`Coalescer::absorb`] and drains it with
/// [`Coalescer::flush_ready`]; tests drive it directly.
struct Coalescer {
    root: LibraryRootId,
    base: PathBuf,
    timings: WatcherTimings,
    /// Keyed on the real path, not its folded comparison key: `Eq`/`Hash` of
    /// [`RelativeMediaPath`] already fold case and Unicode, so two spellings of
    /// one file still share an entry, while `normalized()` keeps the spelling
    /// that can actually be opened for the stability sample.
    pending: HashMap<RelativeMediaPath, PendingEvent>,
    /// A rescan request is not a path, so it lives beside the map rather than
    /// inside it.
    rescan_at: Option<Instant>,
}

/// A debounced event waiting for its window and stability samples.
#[derive(Clone, Debug)]
struct PendingEvent {
    kind: FileEventKind,
    ready_at: Instant,
    /// First stability sample (`(size, mtime_ns)`) for created/modified
    /// events. The second flush after `sample_delay` takes the comparison
    /// sample: only two *consecutive equal* samples emit the event.
    sample: Option<(u64, i64)>,
}

impl Coalescer {
    fn new(root: LibraryRootId, base: PathBuf, timings: WatcherTimings) -> Self {
        Self {
            root,
            base,
            timings,
            pending: HashMap::new(),
            rescan_at: None,
        }
    }

    /// Normal media and isolated sync versions may produce candidate events.
    /// The application accepts Echo version events only for a durable current
    /// song association; staging, records and all other control files stay out.
    fn not_under_media(relative: &RelativeMediaPath) -> bool {
        !Self::media_root_component(relative) && !relative.normalized().starts_with("echo/media/")
    }

    /// Whether the path's first component is exactly `media` — the managed
    /// tree. `media_upload/x.mp3` or a loose root file is NOT under media.
    fn media_root_component(relative: &RelativeMediaPath) -> bool {
        relative
            .normalized()
            .split('/')
            .next()
            .is_some_and(|component| component == MEDIA_ROOT)
    }

    /// Normalize one raw notify event into pending state.
    fn absorb(&mut self, event: &notify::Event) {
        use notify::event::{EventKind, ModifyKind, RenameMode};
        let ready_at = Instant::now() + self.timings.debounce;
        match &event.kind {
            EventKind::Create(_) => {
                for relative in self.relatives(&event.paths) {
                    if Self::not_under_media(&relative) {
                        continue;
                    }
                    self.pending.insert(
                        relative,
                        PendingEvent {
                            kind: FileEventKind::Created,
                            ready_at,
                            sample: None,
                        },
                    );
                }
            }
            EventKind::Modify(ModifyKind::Name(RenameMode::Both)) if event.paths.len() == 2 => {
                if let Some((from, to)) = self.rename_pair(&event.paths[0], &event.paths[1]) {
                    match (Self::not_under_media(&from), Self::not_under_media(&to)) {
                        // Publish out of staging: the destination appearing is
                        // a creation, not a library-visible rename.
                        (true, false) => self.pending.insert(
                            to,
                            PendingEvent {
                                kind: FileEventKind::Created,
                                ready_at,
                                sample: None,
                            },
                        ),
                        // Anything moving *into* staging is Echo's own work.
                        (_, true) => None,
                        // A genuine user rename inside the library.
                        (false, false) => self.pending.insert(
                            to,
                            PendingEvent {
                                kind: FileEventKind::Renamed { from },
                                ready_at,
                                sample: None,
                            },
                        ),
                    };
                }
            }
            EventKind::Modify(ModifyKind::Name(_)) => {
                // One-sided rename. If it touches Echo's staging area it is
                // our own work — never a rescan downgrade.
                let touches_staging = self
                    .relatives(&event.paths)
                    .iter()
                    .any(Self::not_under_media);
                if !touches_staging {
                    // The counterpart is unknown, so reconciliation must
                    // rescan instead of guessing.
                    self.rescan_at = Some(ready_at);
                }
            }
            EventKind::Modify(_) => {
                // Data/metadata/any changes behave as modifications.
                for relative in self.relatives(&event.paths) {
                    if Self::not_under_media(&relative) {
                        continue;
                    }
                    self.pending.insert(
                        relative,
                        PendingEvent {
                            kind: FileEventKind::Modified,
                            ready_at,
                            sample: None,
                        },
                    );
                }
            }
            EventKind::Remove(_) => {
                for relative in self.relatives(&event.paths) {
                    if Self::not_under_media(&relative) {
                        continue;
                    }
                    self.pending.insert(
                        relative,
                        PendingEvent {
                            kind: FileEventKind::Removed,
                            ready_at,
                            sample: None,
                        },
                    );
                }
            }
            EventKind::Access(_) | EventKind::Any => {}
            EventKind::Other => {
                self.rescan_at = Some(ready_at);
            }
        }
    }

    /// Emit every event whose debounce window elapsed and whose file shows
    /// two *consecutive equal* size/mtime samples across flushes; created or
    /// modified files still being written are rescheduled, never parsed.
    fn flush_ready(&mut self, emit: &mut dyn FnMut(FileEvent)) {
        let now = Instant::now();
        if self.rescan_at.is_some_and(|at| at <= now) {
            self.rescan_at = None;
            emit(FileEvent {
                root: self.root,
                path: RelativeMediaPath::new(RESCAN_SENTINEL)
                    .expect("the sentinel is a valid relative path"),
                kind: FileEventKind::RescanNeeded,
            });
        }
        let ready: Vec<RelativeMediaPath> = self
            .pending
            .iter()
            .filter(|(_, event)| event.ready_at <= now)
            .map(|(key, _)| key.clone())
            .collect();
        for relative in ready {
            let Some(pending) = self.pending.remove(&relative) else {
                continue;
            };
            if matches!(
                pending.kind,
                FileEventKind::Created | FileEventKind::Modified
            ) {
                let current = sample(&self.base.join(relative.normalized()));
                let Some(current) = current else {
                    // Vanished between events: a Remove event will follow.
                    continue;
                };
                let Some(first) = pending.sample else {
                    // First sample taken; the next flush (one sample window
                    // later) decides.
                    self.pending.insert(
                        relative,
                        PendingEvent {
                            kind: pending.kind,
                            ready_at: Instant::now() + self.timings.sample_delay,
                            sample: Some(current),
                        },
                    );
                    continue;
                };
                if first != current {
                    // Still changing: restart the two-sample window from the
                    // newest observation.
                    self.pending.insert(
                        relative,
                        PendingEvent {
                            kind: pending.kind,
                            ready_at: Instant::now() + self.timings.sample_delay,
                            sample: Some(current),
                        },
                    );
                    continue;
                }
                // Two consecutive equal samples: the file is stable.
            }
            emit(FileEvent {
                root: self.root,
                path: relative,
                kind: pending.kind,
            });
        }
    }

    fn relatives(&self, paths: &[PathBuf]) -> Vec<RelativeMediaPath> {
        paths.iter().filter_map(|p| self.relative(p)).collect()
    }

    fn relative(&self, path: &Path) -> Option<RelativeMediaPath> {
        let relative = path.strip_prefix(&self.base).ok()?;
        RelativeMediaPath::new(&relative.to_string_lossy()).ok()
    }

    /// Order a rename pair. Backends disagree on `[from, to]` vs `[to, from]`
    /// ordering; the filesystem does not — the destination exists, the source
    /// is gone (unless both still exist, where declaration order wins).
    fn rename_pair(
        &self,
        first: &Path,
        second: &Path,
    ) -> Option<(RelativeMediaPath, RelativeMediaPath)> {
        let first_exists = first.exists();
        let second_exists = second.exists();
        let (from, to) = match (first_exists, second_exists) {
            (true, false) => (second, first),
            _ => (first, second),
        };
        Some((self.relative(from)?, self.relative(to)?))
    }
}

fn sample(path: &Path) -> Option<(u64, i64)> {
    let meta = std::fs::metadata(path).ok()?;
    // Directory notifications are a by-product of recursively watching the
    // library root. They are not media changes: only regular files may reach
    // scan reconciliation. In particular, creating `media/` must not mask the
    // immediately following audio-file notification.
    if !meta.is_file() {
        return None;
    }
    let modified = meta
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    Some((meta.len(), i64::try_from(modified).unwrap_or(i64::MAX)))
}

/// The production file-event source: one OS watcher plus one coalescing
/// thread per subscribed root.
#[derive(Clone, Debug)]
pub struct NotifyFileEventSource {
    registry: RootRegistry,
    timings: WatcherTimings,
}

impl NotifyFileEventSource {
    #[must_use]
    pub fn new(registry: RootRegistry) -> Self {
        Self {
            registry,
            timings: WatcherTimings::default(),
        }
    }

    /// Override the timings (tests use millisecond-scale values).
    #[must_use]
    pub const fn with_timings(mut self, timings: WatcherTimings) -> Self {
        self.timings = timings;
        self
    }
}

impl FileEventSource for NotifyFileEventSource {
    fn subscribe(&self, root: LibraryRootId) -> Result<Box<dyn FileEventSubscription>, Error> {
        // The OS backend reports *canonical* event paths while the registry
        // stores the root as given (on macOS `/var/...` is a symlink to
        // `/private/var/...`). Canonicalize once so `strip_prefix` matches
        // every event; failure to canonicalize means the root is gone.
        let base = self
            .registry
            .path_of(root)?
            .canonicalize()
            .map_err(|source| {
                Error::unavailable_with_source("library root", "root unreadable", source)
            })?;
        let (raw_tx, raw_rx) = mpsc::channel::<notify::Result<notify::Event>>();
        let (events_tx, events_rx) = mpsc::channel::<FileEvent>();
        let cancel = Arc::new(AtomicBool::new(false));

        let mut watcher =
            notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
                // A send error only means the coalescing thread is gone.
                let _ = raw_tx.send(result);
            })
            .map_err(|source| {
                Error::unavailable_with_source("file watcher", "watcher unavailable", source)
            })?;
        watcher
            .watch(&base, notify::RecursiveMode::Recursive)
            .map_err(|source| {
                Error::unavailable_with_source("file watcher", "watch registration failed", source)
            })?;

        let timings = self.timings;
        let thread_cancel = Arc::clone(&cancel);
        std::thread::Builder::new()
            .name(format!("echo-watcher-{root}"))
            .spawn(move || {
                // The OS watcher lives (and dies) with this thread: dropping
                // the subscription cancels the loop, which drops the watcher
                // and unregisters the watch.
                let _watcher = watcher;
                let mut coalescer = Coalescer::new(root, base, timings);
                let mut emit = |event: FileEvent| {
                    // A send error means the subscriber is gone.
                    let _ = events_tx.send(event);
                };
                loop {
                    if thread_cancel.load(Ordering::Relaxed) {
                        return;
                    }
                    match raw_rx.recv_timeout(timings.tick) {
                        Ok(Ok(event)) => coalescer.absorb(&event),
                        Ok(Err(source)) => {
                            // Watcher failure: the stream is no longer
                            // trustworthy — degrade to a rescan signal.
                            emit(FileEvent {
                                root,
                                path: RelativeMediaPath::new(RESCAN_SENTINEL)
                                    .expect("valid sentinel"),
                                kind: FileEventKind::RescanNeeded,
                            });
                            tracing::debug!(error = %source, "watcher error degraded to rescan");
                        }
                        Err(mpsc::RecvTimeoutError::Timeout) => {}
                        Err(mpsc::RecvTimeoutError::Disconnected) => return,
                    }
                    coalescer.flush_ready(&mut emit);
                }
            })
            .map_err(|source| {
                Error::unavailable_with_source("file watcher", "coalescer unavailable", source)
            })?;

        Ok(Box::new(NotifySubscription {
            events: Mutex::new(events_rx),
            cancel,
        }))
    }
}

struct NotifySubscription {
    events: Mutex<mpsc::Receiver<FileEvent>>,
    cancel: Arc<AtomicBool>,
}

impl FileEventSubscription for NotifySubscription {
    fn recv(&mut self) -> Result<Option<FileEvent>, Error> {
        // The receiver is shared only to satisfy the port's `Send + Sync`
        // bound; every `recv` serializes through this mutex.
        Ok(self.events.lock().unwrap().recv().ok())
    }
}

impl Drop for NotifySubscription {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests;
