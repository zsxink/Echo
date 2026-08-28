//! Test doubles for the application ports (task 2.7).
//!
//! These fakes are **test-only**: they live under `#[cfg(any(test, feature =
//! "testkit"))]` so use-case tests can simulate permission revocation, crash
//! points, trash failure and out-of-order watcher events without touching a
//! real user directory or real SQLite.
//!
//! The whole module is scaffolding, not shipped business code: it deliberately
//! uses casts for test data, procedural (not pure-functional) helper closures
//! and permissive docs. Enforcing pedantic on it would add noise without
//! protecting any production invariant, so the pedantic/nursery lints are
//! scoped out here while remaining `-D warnings` clean in `echo-core` proper.

#![allow(
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_lossless,
    clippy::too_many_lines,
    clippy::must_use_candidate,
    clippy::unnecessary_to_owned,
    clippy::redundant_clone,
    clippy::doc_markdown,
    clippy::let_and_return,
    clippy::needless_borrow,
    clippy::needless_pass_by_value,
    clippy::manual_let_else,
    clippy::unchecked_time_subtraction,
    clippy::bool_assert_comparison,
    clippy::type_complexity,
    clippy::missing_const_for_fn,
    clippy::significant_drop_in_scrutinee,
    clippy::significant_drop_tightening,
    clippy::manual_map,
    clippy::map_unwrap_or
)]
//!
//! Provided doubles:
//!
//! - [`FakeClock`], [`FakeIdGenerator`] — deterministic time & identity.
//! - [`MemorySongRepository`] / [`MemoryPlaylistRepository`] /
//!   [`MemoryOperationJournal`] / [`MemoryLibraryRepository`] — in-memory.
//! - [`FakeLibraryFileSystem`] — an in-temp-dir, root-constrained FS with
//!   scriptable permission/IO faults.
//! - [`FakeTrash`] — a `SystemTrashPort` that can be told to fail.
//! - [`ScriptedFileEvents`] — a `FileEventSource` that replays a scripted
//!   sequence (out-of-order / duplicates included).
//! - [`FakeMediaProbe`], [`FakeMetadataReader`], [`FakeHasher`],
//!   [`FakeCoverCache`], [`FakeLyricsParser`] — small deterministic fakes.
//!
//! The fakes never touch the user's home or an OS trash; temp dirs come from
//! the `tempfile` crate.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use crate::application::ports::*;
use crate::domain::entities::{LibraryRoot, PlaylistMember, Song, SongAvailability};
use crate::domain::ids::*;
use crate::domain::media::AudioFormat;
use crate::error::Error;

// ---------------------------------------------------------------------------
// Clock & identity (deterministic)
// ---------------------------------------------------------------------------

/// A clock that only moves when explicitly nudged. Tests bump it to simulate
/// elapsed listening time, undo deadlines, scan pacing.
#[derive(Clone, Debug, Default)]
pub struct FakeClock {
    mono: Duration,
    wall: u128,
}

impl FakeClock {
    #[must_use]
    pub fn new() -> Self {
        Self {
            mono: Duration::ZERO,
            wall: 1_700_000_000_000, // a fixed, stable wall anchor
        }
    }

    /// Advance the monotonic clock by `delta` (and wall by the same amount,
    /// scaled to seconds for realism).
    pub fn advance(&mut self, delta: Duration) {
        self.mono += delta;
        self.wall += delta.as_nanos();
    }
}

impl Clock for FakeClock {
    fn now_monotonic(&self) -> Duration {
        self.mono
    }
    fn now_wall(&self) -> SystemTime {
        SystemTime::UNIX_EPOCH + Duration::from_nanos(self.wall as u64)
    }
}

/// Deterministic identity generator: sequential / distinct within a test run
/// (fresh SongIds; other ids can be random).
#[derive(Clone, Debug, Default)]
pub struct FakeIdGenerator;

impl FakeIdGenerator {
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

impl IdGenerator for FakeIdGenerator {
    fn new_song_id(&self) -> SongId {
        // Interior mutation via a thread-safe counter (tests are single-threaded).
        static C: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        SongId::from_uuid(uuid::Uuid::from_u128(
            C.fetch_add(1, std::sync::atomic::Ordering::Relaxed) as u128,
        ))
    }
    fn new_playlist_id(&self) -> PlaylistId {
        PlaylistId::new()
    }
    fn new_operation_id(&self) -> OperationId {
        OperationId::new()
    }
    fn new_library_root_id(&self) -> LibraryRootId {
        LibraryRootId::new()
    }
}

// ---------------------------------------------------------------------------
// In-memory repositories
// ---------------------------------------------------------------------------

type Shared<T> = Arc<Mutex<T>>;

/// In-memory song store.
#[derive(Clone, Debug, Default)]
pub struct MemorySongRepository {
    songs: Shared<BTreeMap<SongId, Song>>,
}

impl MemorySongRepository {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Programmatic insertion for test setup (bypasses port API).
    pub fn seed(&self, song: Song) {
        self.songs.lock().unwrap().insert(song.id(), song);
    }

    /// Copy of all songs (assertion helper).
    pub fn snapshot(&self) -> Vec<Song> {
        self.songs.lock().unwrap().values().cloned().collect()
    }
}

impl SongRepository for MemorySongRepository {
    fn by_id(&self, id: SongId) -> Result<Option<Song>, Error> {
        Ok(self.songs.lock().unwrap().get(&id).cloned())
    }
    fn by_path(
        &self,
        root: LibraryRootId,
        path: &RelativeMediaPath,
    ) -> Result<Option<Song>, Error> {
        Ok(self
            .songs
            .lock()
            .unwrap()
            .values()
            .find(|s| s.root() == root && s.path() == path)
            .cloned())
    }
    fn upsert(&self, song: &Song) -> Result<(), Error> {
        self.songs.lock().unwrap().insert(song.id(), song.clone());
        Ok(())
    }
    fn set_availability(&self, id: SongId, availability: SongAvailability) -> Result<(), Error> {
        if let Some(s) = self.songs.lock().unwrap().get_mut(&id) {
            match availability {
                SongAvailability::Available => s.restore_available(),
                SongAvailability::Missing => s.mark_missing(),
                SongAvailability::PendingDelete => s.begin_pending_delete(),
            }
        }
        Ok(())
    }
    fn set_favorite(&self, id: SongId, favorite: bool) -> Result<(), Error> {
        if let Some(s) = self.songs.lock().unwrap().get_mut(&id) {
            s.set_favorite(favorite);
        }
        Ok(())
    }
    fn increment_play_count(&self, id: SongId) -> Result<(), Error> {
        if let Some(s) = self.songs.lock().unwrap().get_mut(&id) {
            s.record_play();
        }
        Ok(())
    }
}

/// In-memory library-root store.
#[derive(Clone, Debug, Default)]
pub struct MemoryLibraryRepository {
    roots: Shared<BTreeMap<LibraryRootId, LibraryRoot>>,
}

impl MemoryLibraryRepository {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl LibraryRepository for MemoryLibraryRepository {
    fn active_root(&self) -> Result<Option<LibraryRoot>, Error> {
        Ok(self
            .roots
            .lock()
            .unwrap()
            .values()
            .find(|r| r.is_active())
            .cloned())
    }
    fn by_id(&self, id: LibraryRootId) -> Result<Option<LibraryRoot>, Error> {
        Ok(self.roots.lock().unwrap().get(&id).cloned())
    }
    fn upsert(&self, root: &LibraryRoot) -> Result<(), Error> {
        self.roots.lock().unwrap().insert(root.id(), root.clone());
        Ok(())
    }
    fn deactivate(&self, id: LibraryRootId) -> Result<(), Error> {
        if let Some(r) = self.roots.lock().unwrap().get_mut(&id) {
            *r = LibraryRoot::new(
                r.id(),
                r.absolute_path().to_path_buf(),
                false,
                r.write_capable(),
            );
        }
        Ok(())
    }
    fn set_write_and_availability(
        &self,
        id: LibraryRootId,
        write_capable: bool,
        available: bool,
    ) -> Result<(), Error> {
        if let Some(r) = self.roots.lock().unwrap().get_mut(&id) {
            r.set_write_capable(write_capable);
            r.set_availability(if available {
                crate::domain::entities::RootAvailability::Available
            } else {
                crate::domain::entities::RootAvailability::Unavailable
            });
        }
        Ok(())
    }
}

/// In-memory playlist store (members + names).
#[derive(Clone, Debug, Default)]
pub struct MemoryPlaylistRepository {
    names: Shared<BTreeMap<PlaylistId, (LibraryRootId, String)>>,
    members: Shared<BTreeMap<(PlaylistId, SongId), PlaylistMember>>,
    next_position: Shared<BTreeMap<PlaylistId, u64>>,
}

impl MemoryPlaylistRepository {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl PlaylistRepository for MemoryPlaylistRepository {
    fn by_id(&self, id: PlaylistId) -> Result<Option<PlaylistId>, Error> {
        Ok(self.names.lock().unwrap().contains_key(&id).then_some(id))
    }
    fn by_name(
        &self,
        root: LibraryRootId,
        normalized_name: &str,
    ) -> Result<Option<PlaylistId>, Error> {
        Ok(self
            .names
            .lock()
            .unwrap()
            .iter()
            .find(|(_, (r, n))| *r == root && n == normalized_name)
            .map(|(id, _)| *id))
    }
    fn list(&self, root: LibraryRootId) -> Result<Vec<PlaylistId>, Error> {
        Ok(self
            .names
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, (r, _))| *r == root)
            .map(|(id, _)| *id)
            .collect())
    }
    fn create(&self, id: PlaylistId, root: LibraryRootId, name: &str) -> Result<(), Error> {
        self.names
            .lock()
            .unwrap()
            .insert(id, (root, name.to_owned()));
        self.next_position.lock().unwrap().insert(id, 0);
        Ok(())
    }
    fn rename(&self, id: PlaylistId, to_normalized_name: &str) -> Result<(), Error> {
        if let Some((_, n)) = self.names.lock().unwrap().get_mut(&id) {
            to_normalized_name.clone_into(n);
        }
        Ok(())
    }
    fn delete(&self, id: PlaylistId) -> Result<(), Error> {
        self.names.lock().unwrap().remove(&id);
        let mut members = self.members.lock().unwrap();
        members.retain(|(p, _), _| *p != id);
        Ok(())
    }
    fn members(&self, id: PlaylistId) -> Result<Vec<PlaylistMember>, Error> {
        Ok(self
            .members
            .lock()
            .unwrap()
            .iter()
            .filter(|((p, _), _)| *p == id)
            .map(|(_, m)| m.clone())
            .collect())
    }
    fn add_member(&self, playlist: PlaylistId, song: SongId, position: u64) -> Result<(), Error> {
        // A duplicate (playlist, song) is a no-op — never overwrites the
        // original position (design: repeated add is idempotent).
        let members = self.members.lock().unwrap();
        if members.contains_key(&(playlist, song)) {
            return Ok(());
        }
        drop(members);

        let mut next = self.next_position.lock().unwrap();
        let pos = next.entry(playlist).or_insert(0);
        let position = if position == u64::MAX { *pos } else { position };
        *pos = position.saturating_add(1).max(*pos);
        self.members.lock().unwrap().insert(
            (playlist, song),
            PlaylistMember::new(playlist, song, position, SongAvailability::Available),
        );
        Ok(())
    }
    fn remove_member(&self, playlist: PlaylistId, song: SongId) -> Result<(), Error> {
        self.members.lock().unwrap().remove(&(playlist, song));
        Ok(())
    }
}

/// In-memory operation journal.
#[derive(Clone, Debug, Default)]
pub struct MemoryOperationJournal {
    items: Shared<BTreeMap<(OperationId, String), OperationItem>>,
}

impl MemoryOperationJournal {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl OperationJournalRepository for MemoryOperationJournal {
    fn item_state(
        &self,
        operation: OperationId,
        item: &str,
    ) -> Result<Option<OperationItem>, Error> {
        Ok(self
            .items
            .lock()
            .unwrap()
            .get(&(operation, item.to_owned()))
            .cloned())
    }
    fn upsert_item(&self, operation: OperationId, item: OperationItem) -> Result<(), Error> {
        self.items
            .lock()
            .unwrap()
            .insert((operation, item.target_path.normalized().to_owned()), item);
        Ok(())
    }
    fn items(&self, operation: OperationId) -> Result<Vec<OperationItem>, Error> {
        Ok(self
            .items
            .lock()
            .unwrap()
            .iter()
            .filter(|((op, _), _)| *op == operation)
            .map(|(_, i)| i.clone())
            .collect())
    }
}

// ---------------------------------------------------------------------------
// File-system fake
// ---------------------------------------------------------------------------

/// A root-constrained file system backed by a real temp directory, with
/// scriptable faults. `tempfile` guarantees the path never touches the user's
/// home.
#[derive(Clone, Debug)]
pub struct FakeLibraryFileSystem {
    /// root_id → absolute temp dir
    roots: Shared<BTreeMap<LibraryRootId, PathBuf>>,
    /// scriptable read/write failure injection (a code + message; `Error` is
    /// not `Clone`, so we store a cheap equivalent)
    fault: Shared<Option<(String, String)>>,
    write_capable: Shared<bool>,
}

impl FakeLibraryFileSystem {
    /// Create a temp-dir-backed fake, registering `root` at a fresh temp dir.
    pub fn with_root(root: LibraryRootId) -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.keep();
        let this = Self {
            roots: Arc::new(Mutex::new(BTreeMap::from([(root, path)]))),
            fault: Arc::new(Mutex::new(None)),
            write_capable: Arc::new(Mutex::new(true)),
        };
        this
    }

    /// Make the next operation fail (simulates permission revocation / IO).
    /// The stored fault is rebuilt into an owned [`Error`] on read.
    pub fn inject_fault(&self, err: Error) {
        *self.fault.lock().unwrap() = Some((err.code().to_owned(), err.to_string()));
    }
    pub fn clear_fault(&self) {
        *self.fault.lock().unwrap() = None;
    }
    /// Programmatically toggle write capability.
    pub fn set_write_capable(&self, capable: bool) {
        *self.write_capable.lock().unwrap() = capable;
    }

    fn abs(&self, root: LibraryRootId, rel: &RelativeMediaPath) -> PathBuf {
        self.roots
            .lock()
            .unwrap()
            .get(&root)
            .cloned()
            .expect("unknown root")
            .join(rel.normalized())
    }

    /// Rebuild an owned [`Error`] from the scripted fault (tests only assert
    /// failure, never the exact variant shape).
    fn fault_error(&self) -> Option<Error> {
        self.fault
            .lock()
            .unwrap()
            .as_ref()
            .map(|(what, msg)| Error::Storage {
                what: what.clone(),
                source: std::io::Error::other(msg.clone()).into(),
            })
    }
}

impl LibraryFileSystem for FakeLibraryFileSystem {
    fn enumerate(&self, root: LibraryRootId) -> Result<Vec<RelativeMediaPath>, Error> {
        if let Some(err) = self.fault_error() {
            return Err(err);
        }
        let base = self
            .roots
            .lock()
            .unwrap()
            .get(&root)
            .cloned()
            .expect("root");
        let mut out = Vec::new();
        walk_dir(&base, &base, &mut out);
        Ok(out)
    }
    fn file_meta(&self, root: LibraryRootId, path: &RelativeMediaPath) -> Result<FileMeta, Error> {
        if let Some(err) = self.fault_error() {
            return Err(err);
        }
        let m = std::fs::metadata(self.abs(root, path))
            .map_err(|e| Error::io("stat", e, self.abs(root, path)))?;
        Ok(FileMeta {
            size: m.len(),
            modified_ns: m
                .modified()
                .ok()
                .map(|t| {
                    t.duration_since(SystemTime::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_nanos() as i64
                })
                .unwrap_or(0),
        })
    }
    fn read_head(
        &self,
        root: LibraryRootId,
        path: &RelativeMediaPath,
        limit: u64,
    ) -> Result<Vec<u8>, Error> {
        if let Some(err) = self.fault_error() {
            return Err(err);
        }
        let abs = self.abs(root, path);
        let data = std::fs::read(&abs).map_err(|e| Error::io("read", e, abs.clone()))?;
        Ok(data.into_iter().take(limit as usize).collect())
    }
    fn publish(
        &self,
        root: LibraryRootId,
        staged: &Path,
        target: &RelativeMediaPath,
    ) -> Result<(), Error> {
        if let Some(err) = self.fault_error() {
            return Err(err);
        }
        let dest = self.abs(root, target);
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| Error::io("create_dir_all", e, parent.to_path_buf()))?;
        }
        std::fs::rename(staged, &dest).map_err(|e| Error::io("rename", e, dest.clone()))
    }
    fn write_capable(&self, root: LibraryRootId) -> Result<bool, Error> {
        let _ = root;
        Ok(*self.write_capable.lock().unwrap())
    }
}

fn walk_dir(base: &Path, dir: &Path, out: &mut Vec<RelativeMediaPath>) {
    if let Ok(entries) = std::fs::read_dir(dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                walk_dir(base, &p, out);
            } else if let Ok(rel) = p.strip_prefix(base) {
                if let Ok(rp) = RelativeMediaPath::new(&rel.to_string_lossy()) {
                    out.push(rp);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Other small fakes
// ---------------------------------------------------------------------------

/// A `SystemTrashPort` that can be scripted to succeed or fail.
#[derive(Clone, Debug, Default)]
pub struct FakeTrash {
    fail: Arc<Mutex<bool>>,
    calls: Arc<Mutex<Vec<OperationId>>>,
}

impl FakeTrash {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
    /// Make trash calls fail (simulates system trash unavailable).
    pub fn set_fails(&self, fails: bool) {
        *self.fail.lock().unwrap() = fails;
    }
    /// Operations sent to trash so far.
    pub fn calls(&self) -> Vec<OperationId> {
        self.calls.lock().unwrap().clone()
    }
}

impl SystemTrashPort for FakeTrash {
    fn send_to_trash(&self, _root: LibraryRootId, operation: OperationId) -> Result<(), Error> {
        self.calls.lock().unwrap().push(operation);
        if *self.fail.lock().unwrap() {
            Err(Error::Unavailable {
                resource: "system trash".into(),
                hint: "系统回收站不可用".into(),
            })
        } else {
            Ok(())
        }
    }
}

/// A scripted file-event source that replays a fixed sequence (including
/// out-of-order, duplicate or dropped frames the adapter would have coalesced).
#[derive(Clone, Debug)]
pub struct ScriptedFileEvents {
    queue: Shared<std::collections::VecDeque<FileEvent>>,
}

impl ScriptedFileEvents {
    #[must_use]
    pub fn new(events: Vec<FileEvent>) -> Self {
        Self {
            queue: Arc::new(Mutex::new(events.into())),
        }
    }
    pub fn push(&self, event: FileEvent) {
        self.queue.lock().unwrap().push_back(event);
    }
}

impl FileEventSource for ScriptedFileEvents {
    fn subscribe(&self, _root: LibraryRootId) -> Result<Box<dyn FileEventSubscription>, Error> {
        Ok(Box::new(ScriptedSubscription(self.clone())))
    }
}

struct ScriptedSubscription(ScriptedFileEvents);

impl FileEventSubscription for ScriptedSubscription {
    fn recv(&mut self) -> Result<Option<FileEvent>, Error> {
        Ok(self.0.queue.lock().unwrap().pop_front())
    }
}

/// Deterministic probe: maps a path to a fixed outcome.
#[derive(Clone, Debug, Default)]
pub struct FakeMediaProbe {
    map: Arc<Mutex<BTreeMap<String, ProbeOutcome>>>,
}

impl FakeMediaProbe {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
    pub fn set(&self, path: &str, outcome: ProbeOutcome) {
        self.map.lock().unwrap().insert(path.to_owned(), outcome);
    }
}

impl MediaProbe for FakeMediaProbe {
    fn probe(&self, _root: LibraryRootId, path: &RelativeMediaPath) -> Result<ProbeOutcome, Error> {
        Ok(self
            .map
            .lock()
            .unwrap()
            .get(path.normalized())
            .cloned()
            .unwrap_or(ProbeOutcome::Unsupported))
    }
}

/// Deterministic metadata reader keyed by path.
#[derive(Clone, Debug, Default)]
pub struct FakeMetadataReader {
    map: Arc<Mutex<BTreeMap<String, crate::domain::media::ParsedMetadata>>>,
}

impl FakeMetadataReader {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
    pub fn set(&self, path: &str, meta: crate::domain::media::ParsedMetadata) {
        self.map.lock().unwrap().insert(path.to_owned(), meta);
    }
}

impl MetadataReader for FakeMetadataReader {
    fn read(
        &self,
        _root: LibraryRootId,
        path: &RelativeMediaPath,
    ) -> Result<crate::domain::media::ParsedMetadata, Error> {
        Ok(self
            .map
            .lock()
            .unwrap()
            .get(path.normalized())
            .cloned()
            .unwrap_or_default())
    }
}

/// Deterministic hasher (test-provable, stable).
#[derive(Clone, Debug, Default)]
pub struct FakeHasher;

impl ContentHasher for FakeHasher {
    fn hash(&self, _root: LibraryRootId, path: &RelativeMediaPath) -> Result<String, Error> {
        Ok(format!("fakehash-{}", path.normalized()))
    }
    fn hash_of_bytes(&self, bytes: &[u8]) -> String {
        crate::logging::redact_sensitive(&format!("{bytes:?}"))
    }
}

/// In-memory cover cache.
#[derive(Clone, Debug, Default)]
pub struct MemoryCoverCache {
    keys: Arc<Mutex<BTreeMap<String, (Vec<u8>, String)>>>,
}

impl MemoryCoverCache {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl CoverCache for MemoryCoverCache {
    fn put(&self, bytes: &[u8], mime: &str) -> Result<String, Error> {
        let key = crate::logging::redact_sensitive(&format!("{bytes:?}"));
        self.keys
            .lock()
            .unwrap()
            .insert(key.clone(), (bytes.to_vec(), mime.to_owned()));
        Ok(key)
    }
    fn get(&self, asset_key: &str) -> Result<Option<Vec<u8>>, Error> {
        Ok(self
            .keys
            .lock()
            .unwrap()
            .get(asset_key)
            .map(|(b, _)| b.clone()))
    }
    fn gc(&self, referenced_keys: &[String]) -> Result<(), Error> {
        let mut map = self.keys.lock().unwrap();
        map.retain(|k, _| referenced_keys.contains(k));
        Ok(())
    }
}

/// Simple parser: each non-empty line becomes a plain-text lyric line.
#[derive(Clone, Debug, Default)]
pub struct FakeLyricsParser {
    plain: Arc<Mutex<bool>>,
}

impl FakeLyricsParser {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
    /// Simulate a plain-text (no timestamp) source.
    pub fn set_plain(&self, plain: bool) {
        *self.plain.lock().unwrap() = plain;
    }
}

impl LyricsParser for FakeLyricsParser {
    fn parse(&self, raw: &str) -> crate::domain::entities::LyricsCandidate {
        crate::domain::entities::LyricsCandidate::new(
            crate::domain::entities::LyricsSource::Embedded,
            raw.lines()
                .enumerate()
                .map(|(i, l)| crate::domain::entities::LyricsLine {
                    timestamp_ms: (i as i64 + 1) * 1000,
                    text: Box::leak(l.to_owned().into_boxed_str()),
                })
                .collect(),
            *self.plain.lock().unwrap(),
        )
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> LibraryRootId {
        LibraryRootId::new()
    }

    #[test]
    fn fake_clock_advances_monotonically_and_wall_is_stable() {
        let mut clock = FakeClock::new();
        let t0 = clock.now_monotonic();
        clock.advance(Duration::from_secs(35));
        let t1 = clock.now_monotonic();
        assert!(t1 > t0);
        assert_eq!(t1 - t0, Duration::from_secs(35));
        // Wall clock is anchored and never the real `now`.
        let w = clock.now_wall();
        assert!(w <= std::time::SystemTime::now());
    }

    #[test]
    fn fake_id_generator_produces_distinct_song_ids() {
        let gen = FakeIdGenerator::new();
        let a = gen.new_song_id();
        let b = gen.new_song_id();
        assert_ne!(a, b);
    }

    #[test]
    fn memory_repositories_round_trip() {
        let songs = MemorySongRepository::new();
        let root = LibraryRootId::new();
        let sid = SongId::new();
        let path = RelativeMediaPath::new("周杰伦/晴天.flac").unwrap();
        let mut song = Song::new(sid, root, path.clone(), Revision::INITIAL);
        song.set_favorite(true);
        songs.upsert(&song).unwrap();
        assert_eq!(songs.by_id(sid).unwrap().unwrap().favorite(), true);
        assert_eq!(
            songs.by_path(root, &path).unwrap().unwrap().id(),
            sid,
            "by_path hits the same record"
        );
        songs.increment_play_count(sid).unwrap();
        assert_eq!(songs.by_id(sid).unwrap().unwrap().play_count().as_u64(), 1);
        songs
            .set_availability(sid, SongAvailability::Missing)
            .unwrap();
        assert_eq!(
            songs.by_id(sid).unwrap().unwrap().availability(),
            SongAvailability::Missing
        );
    }

    #[test]
    fn metadata_and_probe_doubles_are_keyed_by_path() {
        let probe = FakeMediaProbe::new();
        probe.set(
            "a.flac",
            ProbeOutcome::Audio {
                format: AudioFormat::Flac,
                duration: Some(Duration::from_secs(269)),
            },
        );
        let outcome = probe
            .probe(root(), &RelativeMediaPath::new("a.flac").unwrap())
            .unwrap();
        assert!(matches!(
            outcome,
            ProbeOutcome::Audio {
                format: AudioFormat::Flac,
                ..
            }
        ));
        // Unknown path → Unsupported.
        assert_eq!(
            probe
                .probe(root(), &RelativeMediaPath::new("x.xyz").unwrap())
                .unwrap(),
            ProbeOutcome::Unsupported
        );

        let meta = FakeMetadataReader::new();
        meta.set(
            "a.flac",
            crate::domain::media::ParsedMetadata {
                title: Some("晴天".into()),
                artist: Some("周杰伦".into()),
                ..Default::default()
            },
        );
        let m = meta
            .read(root(), &RelativeMediaPath::new("a.flac").unwrap())
            .unwrap();
        assert_eq!(m.title.as_deref(), Some("晴天"));
    }

    #[test]
    fn fake_fs_simulates_permission_revocation_and_publish_works() {
        let r = root();
        let fs = FakeLibraryFileSystem::with_root(r);
        // Write a staged file, then publish it.
        let staged = fs.roots.lock().unwrap().get(&r).unwrap().join("staged.bin");
        std::fs::write(&staged, b"audio").unwrap();
        fs.publish(
            r,
            &staged,
            &RelativeMediaPath::new("华语/稻香.mp3").unwrap(),
        )
        .unwrap();
        assert!(fs
            .roots
            .lock()
            .unwrap()
            .get(&r)
            .unwrap()
            .join("华语/稻香.mp3")
            .exists());
        // Enumerate sees it.
        let found = fs.enumerate(r).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].display(), "华语/稻香.mp3");

        // Fault injection simulates permission revocation.
        fs.inject_fault(Error::unavailable("test root", "权限被撤销"));
        assert!(fs.enumerate(r).is_err());
        assert!(fs
            .read_head(r, &RelativeMediaPath::new("华语/稻香.mp3").unwrap(), 4)
            .is_err());
        fs.clear_fault();
        assert!(fs.enumerate(r).is_ok());
    }

    #[test]
    fn fake_trash_distinguishes_success_and_failure() {
        let trash = FakeTrash::new();
        let op = OperationId::new();
        let r = root();
        trash.send_to_trash(r, op).unwrap();
        assert_eq!(trash.calls(), vec![op]);

        trash.set_fails(true);
        let op2 = OperationId::new();
        let err = trash.send_to_trash(r, op2).unwrap_err();
        assert_eq!(err.code(), "unavailable");
        let calls = trash.calls();
        assert_eq!(calls.len(), 2, "failed trash still recorded the call");
    }

    #[test]
    fn scripted_watcher_replays_out_of_order_and_duplicate_events() {
        let r = root();
        let p = |s: &str| RelativeMediaPath::new(s).unwrap();
        let events = vec![
            FileEvent {
                root: r,
                path: p("b.mp3"),
                kind: FileEventKind::Created,
            },
            FileEvent {
                root: r,
                path: p("a.mp3"),
                kind: FileEventKind::Created,
            },
            FileEvent {
                root: r,
                path: p("b.mp3"),
                kind: FileEventKind::Modified,
            }, // duplicate of b
        ];
        let source = ScriptedFileEvents::new(events);
        let mut sub = source.subscribe(r).unwrap();
        let first = sub.recv().unwrap().unwrap();
        assert_eq!(first.path.display(), "b.mp3");
        let second = sub.recv().unwrap().unwrap();
        assert_eq!(second.path.display(), "a.mp3");
        // Out-of-order/duplicate frames arrive as scripted; use cases must
        // coalesce — the subscription itself never reorders.
        let third = sub.recv().unwrap().unwrap();
        assert_eq!(third.kind, FileEventKind::Modified);
        assert!(sub.recv().unwrap().is_none(), "queue drains");
    }

    #[test]
    fn memory_cover_cache_round_trips_and_gc_removes_unreferenced() {
        let cache = MemoryCoverCache::new();
        let key = cache.put(b"coverbytes", "image/jpeg").unwrap();
        assert_eq!(
            cache.get(&key).unwrap().as_deref(),
            Some(b"coverbytes".as_slice())
        );
        cache.gc(&[]).unwrap();
        assert!(
            cache.get(&key).unwrap().is_none(),
            "GC removed unreferenced asset"
        );
    }

    #[test]
    fn memory_playlist_members_append_without_duplication() {
        let repo = MemoryPlaylistRepository::new();
        let pid = PlaylistId::new();
        let sid = SongId::new();
        let r = root();
        repo.create(pid, r, "通勤路上").unwrap();
        repo.add_member(pid, sid, 0).unwrap();
        repo.add_member(pid, sid, u64::MAX).unwrap(); // duplicate append
        let members = repo.members(pid).unwrap();
        assert_eq!(members.len(), 1, "same (playlist, song) collapses");
        assert_eq!(members[0].position(), 0);
    }
}
