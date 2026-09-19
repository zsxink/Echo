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
use std::io::Read;
use std::sync::{Arc, Mutex};
#[cfg(test)]
use std::time::Duration;

use crate::application::ports::*;
use crate::domain::ids::*;
use crate::domain::library::{
    LibraryManifest, PortableRecord, PortableSerialize, RecordKind, CURRENT_FORMAT_VERSION,
    MEDIA_ROOT,
};
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
        let map = self.map.lock().unwrap();
        Ok(map
            .get(path.normalized())
            .cloned()
            // Prefix-tolerant: seeding may use either the `media/…` form the
            // scan sees, or a bare test path. Fall back across the two forms.
            .or_else(|| {
                let normalized = path.normalized();
                normalized
                    .strip_prefix(&format!("{MEDIA_ROOT}/"))
                    .and_then(|rest| map.get(rest).cloned())
                    .or_else(|| map.get(&format!("{MEDIA_ROOT}/{normalized}")).cloned())
            })
            .unwrap_or(ProbeOutcome::Unsupported))
    }
}

/// Deterministic metadata reader keyed by path (published files) and by raw
/// content (import sources, whose tags are parsed before any library write).
#[derive(Clone, Debug, Default)]
pub struct FakeMetadataReader {
    map: Arc<Mutex<BTreeMap<String, crate::domain::media::ParsedMetadata>>>,
    bytes_map: Arc<Mutex<BTreeMap<Vec<u8>, crate::domain::media::ParsedMetadata>>>,
}

impl FakeMetadataReader {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
    pub fn set(&self, path: &str, meta: crate::domain::media::ParsedMetadata) {
        self.map.lock().unwrap().insert(path.to_owned(), meta);
    }
    /// Register the tags an import source's *content* carries (task 5.2: the
    /// naming step parses tags from the source bytes before the target is
    /// planned or anything is staged).
    pub fn set_bytes(&self, content: &[u8], meta: crate::domain::media::ParsedMetadata) {
        self.bytes_map
            .lock()
            .unwrap()
            .insert(content.to_vec(), meta);
    }
}

impl MetadataReader for FakeMetadataReader {
    fn read(
        &self,
        _root: LibraryRootId,
        path: &RelativeMediaPath,
    ) -> Result<crate::domain::media::ParsedMetadata, Error> {
        let map = self.map.lock().unwrap();
        let normalized = path.normalized();
        let meta = map
            .get(normalized)
            .cloned()
            // Prefix-tolerant seeding, mirroring [`FakeMediaProbe`]: the scan
            // looks up `media/…`, a test may seed either form.
            .or_else(|| {
                normalized
                    .strip_prefix(&format!("{MEDIA_ROOT}/"))
                    .and_then(|rest| map.get(rest).cloned())
                    .or_else(|| map.get(&format!("{MEDIA_ROOT}/{normalized}")).cloned())
            })
            .unwrap_or_default();
        Ok(meta)
    }
    fn read_bytes(&self, content: &[u8]) -> Result<crate::domain::media::ParsedMetadata, Error> {
        Ok(self
            .bytes_map
            .lock()
            .unwrap()
            .get(content)
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

/// Scripted external import sources (the [`ImportSourceReader`] double):
/// handles map to display names + bytes, and a source can be scripted to fail
/// its describe/read the way a revoked permission or vanished file would. A
/// same-basename `.lrc` sidecar can be registered per source (task 5.4) and
/// scripted to fail the way an unreadable sidecar would.
#[derive(Clone, Debug, Default)]
pub struct FakeImportSources {
    sources: Shared<BTreeMap<String, FakeSource>>,
    sidecars: Shared<BTreeMap<String, FakeSidecar>>,
    reads: Shared<Vec<String>>,
    sidecar_reads: Shared<Vec<String>>,
}

#[derive(Clone, Debug)]
struct FakeSource {
    display_name: String,
    bytes: Vec<u8>,
    /// When `Some`, the source describes this size but serves only `bytes` —
    /// the mid-read-truncation fault recovery must reject (task 5.5). It is
    /// always >= the served bytes for a genuine truncation.
    described_size: Option<u64>,
    failure: Option<String>,
}

/// A scripted same-basename `.lrc` sidecar beside a source.
#[derive(Clone, Debug)]
struct FakeSidecar {
    display_name: String,
    bytes: Vec<u8>,
    failure: Option<String>,
}

impl FakeImportSources {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Register one selectable source.
    pub fn add(&self, key: &str, display_name: &str, bytes: &[u8]) {
        self.sources.lock().unwrap().insert(
            key.to_owned(),
            FakeSource {
                display_name: display_name.to_owned(),
                bytes: bytes.to_vec(),
                described_size: None,
                failure: None,
            },
        );
    }

    /// Register a source that *describes* `described_size` bytes but opens
    /// serving only `bytes` — a mid-read truncation (vanished/truncated file)
    /// that the import must reject instead of publishing a partial copy.
    pub fn add_truncated(&self, key: &str, display_name: &str, described_size: u64, bytes: &[u8]) {
        self.sources.lock().unwrap().insert(
            key.to_owned(),
            FakeSource {
                display_name: display_name.to_owned(),
                bytes: bytes.to_vec(),
                described_size: Some(described_size),
                failure: None,
            },
        );
    }

    /// Register the same-basename `.lrc` beside one source (task 5.4).
    pub fn add_sidecar(&self, key: &str, display_name: &str, bytes: &[u8]) {
        self.sidecars.lock().unwrap().insert(
            key.to_owned(),
            FakeSidecar {
                display_name: display_name.to_owned(),
                bytes: bytes.to_vec(),
                failure: None,
            },
        );
    }

    /// Make a source fail its describe/read (permission, vanished file…).
    pub fn fail(&self, key: &str, message: &str) {
        if let Some(source) = self.sources.lock().unwrap().get_mut(key) {
            source.failure = Some(message.to_owned());
        }
    }

    /// Make a source's sidecar fail its describe/read (unreadable sidecar).
    pub fn fail_sidecar(&self, key: &str, message: &str) {
        if let Some(sidecar) = self.sidecars.lock().unwrap().get_mut(key) {
            sidecar.failure = Some(message.to_owned());
        }
    }

    /// Handles actually read so far (assertion helper: the "refuse the whole
    /// batch" path must never read a source).
    #[must_use]
    pub fn read_keys(&self) -> Vec<String> {
        self.reads.lock().unwrap().clone()
    }

    /// Sidecar handles actually opened so far (assertion helper: the import
    /// must not open a sidecar for sources that have none).
    #[must_use]
    pub fn sidecar_read_keys(&self) -> Vec<String> {
        self.sidecar_reads.lock().unwrap().clone()
    }
}

impl ImportSourceReader for FakeImportSources {
    fn describe(&self, source: &ImportSource) -> Result<ImportSourceInfo, Error> {
        let map = self.sources.lock().unwrap();
        let source = map
            .get(source.key())
            .ok_or_else(|| Error::unavailable("import source", "unknown handle"))?;
        source.failure.as_ref().map_or_else(
            || {
                let size = source
                    .described_size
                    .unwrap_or_else(|| u64::try_from(source.bytes.len()).unwrap_or(u64::MAX));
                Ok(ImportSourceInfo {
                    display_name: source.display_name.clone(),
                    size,
                })
            },
            |message| Err(source_failure(message)),
        )
    }

    fn open<'a>(&'a self, source: &ImportSource) -> Result<Box<dyn Read + 'a>, Error> {
        self.reads.lock().unwrap().push(source.key().to_owned());
        let map = self.sources.lock().unwrap();
        let source = map
            .get(source.key())
            .ok_or_else(|| Error::unavailable("import source", "unknown handle"))?;
        if let Some(message) = &source.failure {
            return Err(source_failure(message));
        }
        Ok(Box::new(std::io::Cursor::new(source.bytes.clone())))
    }

    fn sidecar(&self, source: &ImportSource) -> Result<Option<SidecarInfo>, Error> {
        let map = self.sidecars.lock().unwrap();
        let Some(sidecar) = map.get(source.key()) else {
            return Ok(None);
        };
        if let Some(message) = &sidecar.failure {
            return Err(Error::unavailable("import sidecar", message));
        }
        Ok(Some(SidecarInfo {
            display_name: sidecar.display_name.clone(),
            size: u64::try_from(sidecar.bytes.len()).unwrap_or(u64::MAX),
        }))
    }

    fn open_sidecar<'a>(
        &'a self,
        source: &ImportSource,
    ) -> Result<Option<Box<dyn Read + 'a>>, Error> {
        self.sidecar_reads
            .lock()
            .unwrap()
            .push(source.key().to_owned());
        let map = self.sidecars.lock().unwrap();
        let Some(sidecar) = map.get(source.key()) else {
            return Ok(None);
        };
        if let Some(message) = &sidecar.failure {
            return Err(source_failure(message));
        }
        Ok(Some(Box::new(std::io::Cursor::new(sidecar.bytes.clone()))))
    }
}

/// The scripted per-source failure (a permission-shaped error, path-free).
fn source_failure(message: &str) -> Error {
    Error::permission(message.to_owned(), crate::error::PermKind::Denied)
}

/// Content-addressed file hasher over the [`LibraryFileSystem`] port: the
/// hash is the real BLAKE3 of the file's bytes, so identical content in two
/// paths hashes identically (needed by the duplicate/re-link tests).
#[derive(Clone)]
pub struct FakeFileHasher {
    fs: Arc<dyn LibraryFileSystem>,
}

impl FakeFileHasher {
    #[must_use]
    pub fn new(fs: Arc<dyn LibraryFileSystem>) -> Self {
        Self { fs }
    }
}

impl ContentHasher for FakeFileHasher {
    fn hash(&self, root: LibraryRootId, path: &RelativeMediaPath) -> Result<String, Error> {
        let bytes = self.fs.read_head(root, path, u64::MAX)?;
        Ok(self.hash_of_bytes(&bytes))
    }
    fn hash_of_bytes(&self, bytes: &[u8]) -> String {
        blake3::hash(bytes).to_hex().to_string()
    }
}

/// Probe double that stalls each probe (keeps scans in flight for
/// concurrency/buffering assertions).
#[derive(Clone)]
pub struct SlowProbe {
    inner: Arc<dyn MediaProbe>,
    delay: std::time::Duration,
}

impl SlowProbe {
    #[must_use]
    pub fn new(inner: Arc<dyn MediaProbe>, delay: std::time::Duration) -> Self {
        Self { inner, delay }
    }
}

impl MediaProbe for SlowProbe {
    fn probe(&self, root: LibraryRootId, path: &RelativeMediaPath) -> Result<ProbeOutcome, Error> {
        std::thread::sleep(self.delay);
        self.inner.probe(root, path)
    }
}

/// An in-memory [`ControlPlanePort`] double. Records are stored in a
/// `BTreeMap` keyed by `(root, kind, uuid)`; the manifest is stored per root.
/// Shared via [`Shared`] so the same fake backs a `ScanDeps` used across many
/// use cases in one test.
#[derive(Clone, Debug, Default)]
pub struct MemoryControlPlane {
    manifest: Shared<BTreeMap<LibraryRootId, LibraryManifest>>,
    records: Shared<BTreeMap<(LibraryRootId, RecordKind, String), PortableRecord>>,
    usable: Arc<Mutex<bool>>,
}

impl MemoryControlPlane {
    /// A fresh, usable in-memory control surface.
    #[must_use]
    pub fn new() -> Self {
        Self {
            usable: Arc::new(Mutex::new(true)),
            ..Default::default()
        }
    }
}

impl MemoryControlPlane {
    /// The manifest stored for `root`.
    #[must_use]
    pub fn manifest_of(&self, root: LibraryRootId) -> Option<LibraryManifest> {
        self.manifest.lock().unwrap().get(&root).cloned()
    }

    /// All records of one root.
    #[must_use]
    pub fn records_of(&self, root: LibraryRootId) -> Vec<PortableRecord> {
        self.records
            .lock()
            .unwrap()
            .iter()
            .filter(|((r, _, _), _)| *r == root)
            .map(|(_, rc)| rc.clone())
            .collect()
    }

    /// Script whether the control plane reports itself usable.
    pub fn set_usable(&self, usable: bool) {
        *self.usable.lock().unwrap() = usable;
    }

    /// Seed a manifest as if it were already on disk — including one from a
    /// *future* format version, which the self-heal path must refuse to touch.
    pub fn set_manifest(&self, root: LibraryRootId, manifest: LibraryManifest) {
        self.manifest.lock().unwrap().insert(root, manifest);
    }
}

impl ControlPlanePort for MemoryControlPlane {
    fn write_manifest(&self, root: LibraryRootId, manifest: &LibraryManifest) -> Result<(), Error> {
        self.manifest.lock().unwrap().insert(root, manifest.clone());
        Ok(())
    }

    fn read_manifest(&self, root: LibraryRootId) -> Result<Option<LibraryManifest>, Error> {
        // Mirrors the real adapter: a manifest from a newer format version is
        // reported absent so no caller trusts it.
        Ok(self
            .manifest_of(root)
            .filter(LibraryManifest::is_compatible))
    }

    fn write_record(&self, root: LibraryRootId, record: &PortableRecord) -> Result<(), Error> {
        let key = (root, record.kind(), record.object_uuid().to_string());
        self.records.lock().unwrap().insert(key, record.clone());
        Ok(())
    }

    fn read_record(
        &self,
        root: LibraryRootId,
        kind: RecordKind,
        object_uuid: &str,
    ) -> Result<Option<PortableRecord>, Error> {
        Ok(self
            .records
            .lock()
            .unwrap()
            .get(&(root, kind, object_uuid.to_string()))
            .cloned())
    }

    fn delete_record(
        &self,
        root: LibraryRootId,
        kind: RecordKind,
        object_uuid: &str,
    ) -> Result<(), Error> {
        self.records
            .lock()
            .unwrap()
            .remove(&(root, kind, object_uuid.to_string()));
        Ok(())
    }

    fn list_records(&self, root: LibraryRootId, kind: RecordKind) -> Result<Vec<String>, Error> {
        let mut out: Vec<String> = self
            .records
            .lock()
            .unwrap()
            .iter()
            .filter(|((r, k, _), _)| *r == root && *k == kind)
            .map(|(_, rc)| rc.to_canonical_json().unwrap_or_default())
            .collect();
        out.sort();
        Ok(out)
    }

    fn control_plane_usable(&self, root: LibraryRootId) -> Result<bool, Error> {
        let _ = root;
        Ok(*self.usable.lock().unwrap())
    }

    fn manifest_state(&self, root: LibraryRootId) -> Result<ManifestState, Error> {
        Ok(match self.manifest_of(root) {
            None => ManifestState::Absent,
            Some(manifest) if manifest.format_version > CURRENT_FORMAT_VERSION => {
                ManifestState::Incompatible {
                    format_version: manifest.format_version,
                }
            }
            Some(manifest) => ManifestState::Compatible(manifest),
        })
    }

    fn records_present(&self, root: LibraryRootId) -> Result<bool, Error> {
        Ok(self
            .records
            .lock()
            .unwrap()
            .keys()
            .any(|(r, _, _)| *r == root))
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
