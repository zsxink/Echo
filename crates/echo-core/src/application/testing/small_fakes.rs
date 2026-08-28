//! Small deterministic fakes: one double per remaining port.

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
    clippy::wildcard_imports,
    clippy::bool_assert_comparison,
    clippy::type_complexity,
    clippy::missing_const_for_fn,
    clippy::significant_drop_in_scrutinee,
    clippy::significant_drop_tightening,
    clippy::manual_map,
    clippy::map_unwrap_or
)]

use std::collections::{BTreeMap, VecDeque};
use std::sync::{Arc, Mutex};
#[cfg(test)]
use std::time::Duration;

use crate::application::ports::*;
use crate::domain::ids::*;
use crate::error::Error;

/// Shared interior-mutability cell backing every in-memory fake.
type Shared<T> = Arc<Mutex<T>>;

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
                source: None,
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
    queue: Shared<VecDeque<FileEvent>>,
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
        let is_plain = *self.plain.lock().unwrap();
        crate::domain::entities::LyricsCandidate::with_raw_text(
            crate::domain::entities::LyricsSource::Embedded,
            raw.to_owned(),
            raw.lines()
                .enumerate()
                .map(|(i, l)| crate::domain::entities::LyricsLine {
                    timestamp_ms: (i as i64 + 1) * 1000,
                    text: l.to_owned(),
                    original_index: i,
                })
                .collect(),
            is_plain.then(|| raw.to_owned()),
            None,
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
    fn metadata_and_probe_doubles_are_keyed_by_path() {
        let probe = FakeMediaProbe::new();
        probe.set(
            "a.flac",
            ProbeOutcome::Audio {
                format: crate::domain::media::AudioFormat::Flac,
                duration: Some(Duration::from_secs(269)),
            },
        );
        let outcome = probe
            .probe(root(), &RelativeMediaPath::new("a.flac").unwrap())
            .unwrap();
        assert!(matches!(
            outcome,
            ProbeOutcome::Audio {
                format: crate::domain::media::AudioFormat::Flac,
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
}
