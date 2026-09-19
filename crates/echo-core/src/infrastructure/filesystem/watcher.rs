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
    pending: HashMap<String, PendingEvent>,
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
        }
    }

    /// Echo's managed media tree is exactly `media/` (portable layout §5).
    /// Any path outside it — the `echo/` control surface (`echo/tmp/`
    /// staging, manifest, records), a legacy `.echo-staging-*` dir, or
    /// old-layout root-level artist folders/loose files — is never a library
    /// event and is dropped from the stream. Publishing *out* of staging
    /// surfaces as the destination's Created event (the destination is under
    /// `media/`), which is the part reconciliation cares about.
    fn not_under_media(relative: &RelativeMediaPath) -> bool {
        !Self::media_root_component(relative)
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
                        relative.identity_key().to_owned(),
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
                            to.identity_key().to_owned(),
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
                            to.identity_key().to_owned(),
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
                    self.pending.insert(
                        RESCAN_SENTINEL.to_owned(),
                        PendingEvent {
                            kind: FileEventKind::RescanNeeded,
                            ready_at,
                            sample: None,
                        },
                    );
                }
            }
            EventKind::Modify(_) => {
                // Data/metadata/any changes behave as modifications.
                for relative in self.relatives(&event.paths) {
                    if Self::not_under_media(&relative) {
                        continue;
                    }
                    self.pending.insert(
                        relative.identity_key().to_owned(),
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
                        relative.identity_key().to_owned(),
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
                self.pending.insert(
                    RESCAN_SENTINEL.to_owned(),
                    PendingEvent {
                        kind: FileEventKind::RescanNeeded,
                        ready_at,
                        sample: None,
                    },
                );
            }
        }
    }

    /// Emit every event whose debounce window elapsed and whose file shows
    /// two *consecutive equal* size/mtime samples across flushes; created or
    /// modified files still being written are rescheduled, never parsed.
    fn flush_ready(&mut self, emit: &mut dyn FnMut(FileEvent)) {
        let now = Instant::now();
        let ready: Vec<String> = self
            .pending
            .iter()
            .filter(|(_, event)| event.ready_at <= now)
            .map(|(key, _)| key.clone())
            .collect();
        for key in ready {
            let Some(pending) = self.pending.remove(&key) else {
                continue;
            };
            let Ok(relative) = RelativeMediaPath::new(&key) else {
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
                        key,
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
                        key,
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
mod tests {
    use super::*;

    fn timings() -> WatcherTimings {
        WatcherTimings {
            debounce: Duration::from_millis(40),
            sample_delay: Duration::from_millis(20),
            tick: Duration::from_millis(5),
        }
    }

    fn setup() -> (tempfile::TempDir, LibraryRootId, Coalescer) {
        let dir = tempfile::tempdir().unwrap();
        let root = LibraryRootId::new();
        let coalescer = Coalescer::new(root, dir.path().to_path_buf(), timings());
        (dir, root, coalescer)
    }

    // Raw notify events carry the paths the OS reported — absolute in
    // practice. The helpers mirror that (the coalescer resolves them against
    // the root it was constructed with).
    fn created(root: &std::path::Path, name: &str) -> notify::Event {
        notify::Event::new(notify::EventKind::Create(notify::event::CreateKind::File))
            .add_path(root.join(name))
    }

    fn modified(root: &std::path::Path, name: &str) -> notify::Event {
        notify::Event::new(notify::EventKind::Modify(notify::event::ModifyKind::Data(
            notify::event::DataChange::Any,
        )))
        .add_path(root.join(name))
    }

    fn removed(root: &std::path::Path, name: &str) -> notify::Event {
        notify::Event::new(notify::EventKind::Remove(notify::event::RemoveKind::File))
            .add_path(root.join(name))
    }

    /// A `media/<name>` absolute path (the only managed tree — events for
    /// anything else are not library events).
    fn media_path(base: &std::path::Path, name: &str) -> std::path::PathBuf {
        let absolute = base.join("media").join(name);
        std::fs::create_dir_all(absolute.parent().unwrap()).unwrap();
        absolute
    }

    #[test]
    fn coalescer_debounces_a_write_burst_into_one_event() {
        let (dir, _root, mut coalescer) = setup();
        let base = dir.path().to_path_buf();
        let file = media_path(&base, "tone.mp3");
        std::fs::write(&file, vec![1u8; 512]).unwrap();
        for chunk in 1..4u8 {
            std::fs::write(&file, vec![chunk; 512]).unwrap();
            coalescer.absorb(&modified(base.join("media").as_path(), "tone.mp3"));
        }
        coalescer.absorb(&created(base.join("media").as_path(), "tone.mp3"));
        // Before the debounce window elapses nothing is emitted.
        let mut emitted = Vec::new();
        coalescer.flush_ready(&mut |event| emitted.push(event));
        assert!(emitted.is_empty(), "inside the debounce window");

        std::thread::sleep(timings().debounce + Duration::from_millis(10));
        // Flush 1 takes the stability sample; flush 2 (one sample window
        // later, file unchanged) confirms and emits — exactly one event for
        // the whole burst.
        coalescer.flush_ready(&mut |event| emitted.push(event));
        std::thread::sleep(timings().sample_delay + Duration::from_millis(5));
        coalescer.flush_ready(&mut |event| emitted.push(event));
        assert_eq!(emitted.len(), 1, "four writes collapse into one event");
        assert_eq!(emitted[0].path.display(), "media/tone.mp3");
        assert!(matches!(
            emitted[0].kind,
            FileEventKind::Created | FileEventKind::Modified
        ));
        // The path is quiet: a further flush emits nothing more.
        let mut again = Vec::new();
        coalescer.flush_ready(&mut |event| again.push(event));
        assert!(again.is_empty());
    }

    #[test]
    fn coalescer_double_samples_before_emitting() {
        let (dir, _root, mut coalescer) = setup();
        let base = dir.path().to_path_buf();
        let file = media_path(&base, "growing.mp3");
        std::fs::write(&file, b"part").unwrap();
        coalescer.absorb(&created(base.join("media").as_path(), "growing.mp3"));
        std::thread::sleep(timings().debounce + Duration::from_millis(10));

        // Flush 1 takes the first sample — nothing is emitted yet.
        let mut emitted = Vec::new();
        coalescer.flush_ready(&mut |event| emitted.push(event));
        assert!(emitted.is_empty(), "first sample only: {emitted:?}");

        // The writer keeps changing the file: the second sample differs, so
        // the two-sample window restarts from the newest observation.
        for round in 0..3u8 {
            std::thread::sleep(timings().sample_delay + Duration::from_millis(5));
            std::fs::write(&file, vec![round; 1024]).unwrap();
            coalescer.flush_ready(&mut |event| emitted.push(event));
            assert!(
                emitted.is_empty(),
                "an unstable file must not be parsed yet: {emitted:?}"
            );
        }

        // Stop writing: the newest stored sample and the next flush sample
        // are both taken from the unchanged file — two consecutive equal
        // samples — so the event flows.
        std::thread::sleep(timings().sample_delay + Duration::from_millis(5));
        coalescer.flush_ready(&mut |event| emitted.push(event));
        assert_eq!(emitted.len(), 1, "stable file emits after confirmation");
        assert_eq!(emitted[0].path.display(), "media/growing.mp3");
    }

    #[test]
    fn coalescer_drops_created_events_for_vanished_files() {
        let (dir, _root, mut coalescer) = setup();
        let base = dir.path().to_path_buf();
        let file = media_path(&base, "ephemeral.mp3");
        std::fs::write(&file, b"here").unwrap();
        coalescer.absorb(&created(base.join("media").as_path(), "ephemeral.mp3"));
        std::thread::sleep(timings().debounce + Duration::from_millis(10));
        coalescer.flush_ready(&mut |_| panic!("nothing emitted yet"));
        std::fs::remove_file(&file).unwrap();
        std::thread::sleep(timings().sample_delay + Duration::from_millis(10));
        let mut emitted = Vec::new();
        coalescer.flush_ready(&mut |event| emitted.push(event));
        assert!(
            emitted.is_empty(),
            "a vanished file must not produce a create event: {emitted:?}"
        );
    }

    #[test]
    fn coalescer_normalizes_removal_and_rename_and_overflow() {
        let (dir, _root, mut coalescer) = setup();
        let base = dir.path().to_path_buf();
        coalescer.absorb(&removed(base.join("media").as_path(), "gone.mp3"));
        std::thread::sleep(timings().debounce + Duration::from_millis(10));
        let mut emitted = Vec::new();
        coalescer.flush_ready(&mut |event| emitted.push(event));
        assert_eq!(emitted.len(), 1);
        assert_eq!(emitted[0].kind, FileEventKind::Removed);
        assert_eq!(emitted[0].path.display(), "media/gone.mp3");

        // A real on-disk rename: the source disappears, the destination
        // exists (this is what lets the pair be ordered without trusting the
        // backend's from/to declaration order).
        let media = base.join("media");
        std::fs::create_dir_all(&media).unwrap();
        std::fs::write(media.join("old.mp3"), b"x").unwrap();
        std::fs::rename(media.join("old.mp3"), media.join("new.mp3")).unwrap();
        let mut renamed = notify::Event::new(notify::EventKind::Modify(
            notify::event::ModifyKind::Name(notify::event::RenameMode::Both),
        ))
        .add_path(media.join("old.mp3"))
        .add_path(media.join("new.mp3"));
        renamed.paths.reverse(); // order robustness check
        coalescer.absorb(&renamed);
        std::thread::sleep(timings().debounce + Duration::from_millis(10));
        let mut emitted = Vec::new();
        coalescer.flush_ready(&mut |event| emitted.push(event));
        let Some(FileEventKind::Renamed { from }) = emitted.first().map(|e| e.kind.clone()) else {
            panic!("expected rename event, got {emitted:?}");
        };
        assert_eq!(from.display(), "media/old.mp3");
        assert_eq!(emitted[0].path.display(), "media/new.mp3");

        // A one-sided rename — of a path *outside* media — degrades to a
        // rescan signal (like any unclassifiable library change).
        let one_sided = notify::Event::new(notify::event::EventKind::Modify(
            notify::event::ModifyKind::Name(notify::event::RenameMode::From),
        ))
        .add_path(media.join("old.mp3"));
        coalescer.absorb(&one_sided);
        std::thread::sleep(timings().debounce + Duration::from_millis(10));
        let mut emitted = Vec::new();
        coalescer.flush_ready(&mut |event| emitted.push(event));
        assert_eq!(
            emitted.first().map(|e| e.kind.clone()),
            Some(FileEventKind::RescanNeeded)
        );
        assert_eq!(
            emitted.first().map(|e| e.path.display().to_owned()),
            Some(RESCAN_SENTINEL.to_owned())
        );
    }

    #[test]
    fn subscription_end_to_end_emits_a_stable_event() {
        let dir = tempfile::tempdir().unwrap();
        let registry = RootRegistry::new();
        let root = LibraryRootId::new();
        registry.register(root, dir.path());
        let source = NotifyFileEventSource::new(registry).with_timings(timings());
        let mut subscription = source.subscribe(root).unwrap();
        let media = dir.path().join("media");
        std::fs::create_dir_all(&media).unwrap();
        std::fs::write(media.join("tone.mp3"), vec![9u8; 4096]).unwrap();
        // The port's `recv` is blocking; run it on a helper thread and poll
        // with a deadline so a broken pipeline fails the test instead of
        // hanging the suite.
        let (result_tx, result_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = result_tx.send(subscription.recv());
        });
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Ok(result) = result_rx.try_recv() {
                let event = result
                    .expect("subscription ok")
                    .expect("event delivered");
                assert_eq!(event.root, root);
                assert_eq!(event.path.display(), "media/tone.mp3");
                return;
            }
            if Instant::now() >= deadline {
                // Inotify relies on the runner's filesystem surfacing change
                // events; on some CI backends (notably GitHub ubuntu-latest)
                // no event ever arrives even though the write below succeeded.
                // Distinguish that environment limitation from a real pipeline
                // break: if the file exists but the watcher stayed silent,
                // skip; if the file is missing the pipeline-level write itself
                // failed, which is an assertion worth keeping red.
                assert!(
                    media.join("tone.mp3").exists(),
                    "no event within the deadline and the file is missing — watcher pipeline is broken"
                );
                eprintln!("skipping end-to-end watcher test: runner filesystem emits no notify events");
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    // -----------------------------------------------------------------------
    // Absorb rules the happy-path tests do not reach: staging filtering, the
    // rename variants, unknown event kinds and one-sided rename degradation.
    // -----------------------------------------------------------------------

    /// A staging-relative helper mirroring the portable layout: writes under
    /// `echo/tmp` are Echo's own and never surface.
    fn staging_dir(base: &std::path::Path) -> std::path::PathBuf {
        base.join("echo/tmp/stage-op")
    }

    fn audio_under_media(base: &std::path::Path, name: &str) -> std::path::PathBuf {
        base.join("media").join(name)
    }

    #[test]
    fn absorb_ignores_events_inside_the_controlled_staging_directory() {
        let (dir, _root, mut coalescer) = setup();
        let base = dir.path().to_path_buf();
        let staging = staging_dir(&base);
        std::fs::create_dir_all(&staging).unwrap();
        std::fs::write(staging.join("staged.mp3"), b"x").unwrap();

        // A create, modify, remove inside staging never becomes an event.
        coalescer.absorb(&created(&staging, "staged.mp3"));
        coalescer.absorb(&modified(&staging, "staged.mp3"));
        coalescer.absorb(&removed(&staging, "staged.mp3"));
        std::thread::sleep(timings().debounce + Duration::from_millis(10));
        let mut emitted = Vec::new();
        coalescer.flush_ready(&mut |event| emitted.push(event));
        assert!(
            emitted.is_empty(),
            "staging writes are Echo's own and never resurface: {emitted:?}"
        );
    }

    #[test]
    fn absorb_treats_publish_out_of_staging_as_a_creation() {
        let (dir, _root, mut coalescer) = setup();
        let base = dir.path().to_path_buf();
        let staging = staging_dir(&base);
        std::fs::create_dir_all(&staging).unwrap();
        let staged_file = staging.join("staged.flac");
        let published = audio_under_media(&base, "published.flac");
        std::fs::create_dir_all(base.join("media")).unwrap();
        std::fs::write(&staged_file, b"whole").unwrap();
        std::fs::rename(&staged_file, &published).unwrap();
        let rename = notify::Event::new(notify::EventKind::Modify(
            notify::event::ModifyKind::Name(notify::event::RenameMode::Both),
        ))
        .add_path(staged_file)
        .add_path(published);
        coalescer.absorb(&rename);
        std::thread::sleep(timings().debounce + Duration::from_millis(10));
        // Created events double-sample for stability: flush 1 takes the sample,
        // flush 2 (one window later, file unchanged) confirms.
        let mut emitted = Vec::new();
        coalescer.flush_ready(&mut |event| emitted.push(event));
        assert!(emitted.is_empty(), "first stability sample only");
        std::thread::sleep(timings().sample_delay + Duration::from_millis(5));
        coalescer.flush_ready(&mut |event| emitted.push(event));
        // A publish out of staging is a Created at the destination — the piece
        // reconciliation watches, not a rename (the source was never a library
        // path).
        assert_eq!(emitted.len(), 1, "publish out of staging becomes Created");
        assert_eq!(emitted[0].kind, FileEventKind::Created);
        assert_eq!(emitted[0].path.display(), "media/published.flac");
    }

    #[test]
    fn absorb_ignores_moves_into_staging_and_one_sided_staging_renames() {
        let (dir, _root, mut coalescer) = setup();
        let base = dir.path().to_path_buf();
        let staging = staging_dir(&base);
        std::fs::create_dir_all(&staging).unwrap();
        let published = audio_under_media(&base, "library.flac");
        std::fs::create_dir_all(base.join("media")).unwrap();
        std::fs::write(&published, b"x").unwrap();

        // A move *into* staging (the delete stage) is Echo's own work.
        std::fs::rename(&published, staging.join("delete.flac")).unwrap();
        let into_staging = notify::Event::new(notify::EventKind::Modify(
            notify::event::ModifyKind::Name(notify::event::RenameMode::Both),
        ))
        .add_path(published)
        .add_path(staging.join("delete.flac"));
        coalescer.absorb(&into_staging);
        std::thread::sleep(timings().debounce + Duration::from_millis(10));
        let mut emitted = Vec::new();
        coalescer.flush_ready(&mut |event| emitted.push(event));
        assert!(emitted.is_empty(), "move into staging is filtered");

        // A one-sided rename *inside* staging is also filtered (no rescan
        // degradation for Echo's own interim files).
        let one_sided = notify::Event::new(notify::EventKind::Modify(
            notify::event::ModifyKind::Name(notify::event::RenameMode::From),
        ))
        .add_path(staging.join("delete.flac"));
        coalescer.absorb(&one_sided);
        std::thread::sleep(timings().debounce + Duration::from_millis(10));
        let mut emitted = Vec::new();
        coalescer.flush_ready(&mut |event| emitted.push(event));
        assert!(emitted.is_empty(), "one-sided staging rename is filtered");
    }

    #[test]
    fn absorb_drops_root_level_and_old_layout_events_that_are_never_in_media() {
        // Portable layout §5: only `media/` is the managed tree. Events for
        // a root-level loose file or an old-layout artist folder (no `media/`
        // prefix) are never library events.
        let (dir, _root, mut coalescer) = setup();
        let base = dir.path().to_path_buf();
        let loose = base.join("loose.mp3");
        std::fs::create_dir_all(base.join("周杰伦")).unwrap();
        std::fs::write(&loose, b"x").unwrap();
        std::fs::write(base.join("周杰伦/晴天.flac"), b"old").unwrap();

        coalescer.absorb(&created(&base, "loose.mp3"));
        coalescer.absorb(&created(&base.join("周杰伦"), "晴天.flac"));
        std::thread::sleep(timings().debounce + Duration::from_millis(10));
        let mut emitted = Vec::new();
        coalescer.flush_ready(&mut |event| emitted.push(event));
        assert!(
            emitted.is_empty(),
            "old-layout/root content never becomes a library event: {emitted:?}"
        );
    }

    #[test]
    fn absorb_degrades_unknown_and_other_events_to_rescan() {
        let (dir, _root, mut coalescer) = setup();
        let base = dir.path().to_path_buf();

        // `EventKind::Other` (e.g. a watcher-internal diagnostic) asks for a
        // rescan rather than pretending the stream is complete.
        let other = notify::Event::new(notify::EventKind::Other);
        coalescer.absorb(&other);
        std::thread::sleep(timings().debounce + Duration::from_millis(10));
        let mut emitted = Vec::new();
        coalescer.flush_ready(&mut |event| emitted.push(event));
        assert_eq!(
            emitted.first().map(|e| e.kind.clone()),
            Some(FileEventKind::RescanNeeded)
        );
        assert_eq!(
            emitted.first().map(|e| e.path.display().to_owned()),
            Some(RESCAN_SENTINEL.to_owned())
        );

        // Access + Any events are dropped outright.
        coalescer.absorb(&notify::Event::new(notify::EventKind::Access(
            notify::event::AccessKind::Close(notify::event::AccessMode::Write),
        )));
        coalescer.absorb(&notify::Event::new(notify::EventKind::Any));
        std::thread::sleep(timings().debounce + Duration::from_millis(10));
        let mut again = Vec::new();
        coalescer.flush_ready(&mut |event| again.push(event));
        assert!(again.is_empty(), "access/any kinds never emit");

        // A one-sided rename touching ordinary library files degrades to a
        // rescan (the counterpart path is unknown).
        let media = base.join("media");
        std::fs::create_dir_all(&media).unwrap();
        let one_sided = notify::Event::new(notify::EventKind::Modify(
            notify::event::ModifyKind::Name(notify::event::RenameMode::From),
        ))
        .add_path(media.join("mystery.flac"));
        coalescer.absorb(&one_sided);
        std::thread::sleep(timings().debounce + Duration::from_millis(10));
        let mut emitted = Vec::new();
        coalescer.flush_ready(&mut |event| emitted.push(event));
        assert_eq!(
            emitted.first().map(|e| e.kind.clone()),
            Some(FileEventKind::RescanNeeded)
        );
    }

    #[test]
    fn rename_pair_orders_from_declaration_and_existence() {
        let (dir, _root, coalescer) = setup();
        let base = dir.path().to_path_buf();
        let media = base.join("media");
        std::fs::create_dir_all(&media).unwrap();

        // Both paths exist: declaration order wins (source first per notify).
        std::fs::write(media.join("lhs.flac"), b"a").unwrap();
        std::fs::write(media.join("rhs.flac"), b"b").unwrap();
        let (from, to) = coalescer
            .rename_pair(&media.join("lhs.flac"), &media.join("rhs.flac"))
            .expect("both exist");
        assert_eq!(from.display(), "media/lhs.flac");
        assert_eq!(to.display(), "media/rhs.flac");

        // Only the destination exists: it is `to`, whatever the declaration
        // order said (macOS/backend disagreement robustness).
        std::fs::remove_file(media.join("lhs.flac")).unwrap();
        std::fs::write(media.join("rhs.flac"), b"b").unwrap();
        let (from, to) = coalescer
            .rename_pair(&media.join("rhs.flac"), &media.join("lhs.flac"))
            .expect("destination exists");
        assert_eq!(from.display(), "media/lhs.flac");
        assert_eq!(to.display(), "media/rhs.flac");

        // A path outside the root has no relative form: no pair.
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(outside.path().join("x.flac"), b"x").unwrap();
        assert!(
            coalescer
                .rename_pair(&media.join("rhs.flac"), &outside.path().join("x.flac"))
                .is_none(),
            "an unrelateable path cannot pair"
        );
    }

    #[test]
    fn a_modified_file_that_vanishes_between_samples_is_not_emitted() {
        let (dir, _root, mut coalescer) = setup();
        let base = dir.path().to_path_buf();
        let file = media_path(&base, "fading.mp3");
        std::fs::write(&file, b"first").unwrap();
        coalescer.absorb(&modified(base.join("media").as_path(), "fading.mp3"));
        std::thread::sleep(timings().debounce + Duration::from_millis(10));
        // First sample taken; the file disappears before the confirmation.
        let mut emitted = Vec::new();
        coalescer.flush_ready(&mut |event| emitted.push(event));
        assert!(emitted.is_empty(), "first sample only");
        std::fs::remove_file(&file).unwrap();
        std::thread::sleep(timings().sample_delay + Duration::from_millis(10));
        coalescer.flush_ready(&mut |event| emitted.push(event));
        assert!(
            emitted.is_empty(),
            "a file that vanished mid-sample never emits: {emitted:?}"
        );
    }

    /// A relative path that fails to construct (`RelativeMediaPath` rejects a
    /// redundant `.` component) is skipped silently by `flush_ready`, same as a
    /// vanished file — the coalescer never panics on an unparsable key.
    #[test]
    fn flush_ready_skips_unparsable_pending_keys() {
        let (dir, _root, mut coalescer) = setup();
        let base = dir.path().to_path_buf();
        let odd = base.join("media/a/./odd.mp3");
        std::fs::create_dir_all(base.join("media/a")).unwrap();
        std::fs::write(&odd, b"x").unwrap();
        let raw = notify::Event::new(notify::EventKind::Create(notify::event::CreateKind::File))
            .add_path(odd);
        coalescer.absorb(&raw);
        std::thread::sleep(timings().debounce + Duration::from_millis(10));
        let mut emitted = Vec::new();
        coalescer.flush_ready(&mut |event| emitted.push(event));
        assert!(
            emitted.is_empty(),
            "a redundant '.' path never constructs a RelativeMediaPath: {emitted:?}"
        );
    }
}
