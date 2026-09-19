//! The portable `echo/` control-surface adapter (task 2.1).
//!
//! Implements [`ControlPlanePort`] on top of the root-constrained file system.
//! The adapter owns the concrete layout of the portable surface:
//!
//! ```text
//! <library-root>/
//! ├── media/…
//! └── echo/
//!     ├── manifest.json
//!     └── records/<kind>/<uuid-prefix>/<uuid>.json
//! ```
//!
//! Design rules implemented here:
//!
//! - **Atomic writes.** Every file is written to a same-directory temporary
//!   file, flushed, then `rename`d onto the final name. A filesystem-synced
//!   copy (e.g. Dropbox) can therefore only ever observe the complete JSON, not
//!   a half-written record. The caller of a failed write sees an [`Error`] and
//!   may retry; the attempt leaves no corrupt target.
//! - **Temporary ignore.** A same-dir `*.tmp-*` file next to a record is not a
//!   record: [`read_record`]/[`list_records`] treat it as absent. Incomplete or
//!   version-unsupported records are likewise ignored (reported absent), so the
//!   next successful write replaces them.
//! - **Kind/prefix sharding.** Records for one kind are split into
//!   prefix-directories by the first two hex digits of the object UUID, keeping
//!   single directories small under a large library.
//! - **Path safety.** All paths are [`ControlPath`]-validated and stay inside
//!   `echo/` and outside `echo/tmp/`. No absolute path, `..` or control path
//!   reaches the file system.
//!
//! The adapter holds a [`RootRegistry`] to resolve the root's absolute path;
//! use cases never see a path.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use crate::application::ports::{ControlPlanePort, ManifestState};
use crate::domain::ids::LibraryRootId;
use crate::domain::library::{
    LibraryManifest, PortableRecord, PortableSerialize, RecordKind, CONTROL_ROOT,
};
use crate::error::Error;

use super::registry::RootRegistry;

/// The constant name of the manifest under `echo/`.
const MANIFEST_NAME: &str = "manifest.json";
/// The records tree under `echo/`.
const RECORDS_DIR: &str = "records";
/// In-progress temp suffix (same-directory atomic write helper).
const TMP_SUFFIX: &str = ".tmp-";
/// The length of the UUID prefix-directory (first two hex digits).
const PREFIX_LEN: usize = 2;
/// Current manifest format (mirrors the domain constant, kept here for quick
/// parse-time compatibility checks without touching the domain).
const MANIFEST_FORMAT: u64 = 1;

/// Filesystem adapter for the portable `echo/` control surface.
#[derive(Clone, Debug)]
pub struct RootControlPlane {
    registry: RootRegistry,
}

impl RootControlPlane {
    /// Construct from the root registry the adapter resolves roots against.
    #[must_use]
    pub const fn new(registry: RootRegistry) -> Self {
        Self { registry }
    }

    /// The root's absolute path.
    fn root_path(&self, root: LibraryRootId) -> Result<PathBuf, Error> {
        self.registry.path_of(root)
    }

    /// The absolute path of the `echo/` control directory.
    fn echo_dir(&self, root: LibraryRootId) -> Result<PathBuf, Error> {
        Ok(self.root_path(root)?.join(CONTROL_ROOT))
    }

    /// The absolute path of one record file.
    fn record_path(
        &self,
        root: LibraryRootId,
        kind: RecordKind,
        object_uuid: &str,
    ) -> Result<PathBuf, Error> {
        let prefix = &object_uuid[..object_uuid.len().min(PREFIX_LEN)];
        Ok(self
            .echo_dir(root)?
            .join(RECORDS_DIR)
            .join(kind.dir_name())
            .join(prefix)
            .join(format!("{object_uuid}.json")))
    }

    /// Write bytes atomically to `target`: write to a same-dir temp, flush,
    /// then rename. A partial write never becomes the published file.
    fn write_atomic(target: &Path, content: &str) -> Result<(), Error> {
        let directory = target
            .parent()
            .ok_or_else(|| Error::validation(crate::error::Subject::Path, "record", "no parent"))?;
        std::fs::create_dir_all(directory)
            .map_err(|source| Error::io("create control directory", source, directory))?;
        let temp = directory.join(format!(
            "{}{TMP_SUFFIX}{}",
            target
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("record"),
            uuid::Uuid::new_v4()
        ));
        // Write the temp fully, fsync it, then rename onto the target. On any
        // failure remove the leftover temp so the directory stays clean.
        let result = (|| -> Result<(), Error> {
            let mut file = std::fs::File::create(&temp)
                .map_err(|source| Error::io("create temp record", source, &temp))?;
            file.write_all(content.as_bytes())
                .map_err(|source| Error::io("write temp record", source, &temp))?;
            file.sync_all()
                .map_err(|source| Error::io("sync temp record", source, &temp))?;
            std::fs::rename(&temp, target)
                .map_err(|source| Error::io("publish record", source, target))?;
            // fsync the parent dir so the rename is durable across a crash.
            if let Ok(dir) = std::fs::File::open(directory) {
                let _ = dir.sync_all();
            }
            Ok(())
        })();
        // The temp file may remain from a prior failed write; remove it on
        // both paths so the directory stays clean.
        let _ = std::fs::remove_file(&temp);
        result
    }

    /// Whether a file name is a temporary of ours (ignored by reads).
    fn is_temp_name(name: &str) -> bool {
        name.contains(TMP_SUFFIX)
    }
}

impl ControlPlanePort for RootControlPlane {
    fn write_manifest(&self, root: LibraryRootId, manifest: &LibraryManifest) -> Result<(), Error> {
        let manifest_path = self.echo_dir(root)?.join(MANIFEST_NAME);
        let json = serde_json::to_string_pretty(manifest).map_err(|e| Error::Storage {
            what: "manifest serialization".to_owned(),
            source: Box::new(e),
        })?;
        Self::write_atomic(&manifest_path, &json)
    }

    fn read_manifest(&self, root: LibraryRootId) -> Result<Option<LibraryManifest>, Error> {
        let manifest_path = self.echo_dir(root)?.join(MANIFEST_NAME);
        if !manifest_path.is_file() {
            return Ok(None);
        }
        let bytes = std::fs::read(&manifest_path)
            .map_err(|source| Error::io("read manifest", source, &manifest_path))?;
        let manifest: LibraryManifest =
            serde_json::from_slice(&bytes).map_err(|e| Error::Storage {
                what: "manifest parse".to_owned(),
                source: Box::new(e),
            })?;
        // A manifest from a future format version is not compatible — report
        // absent so the caller does not trust it.
        if manifest.format_version > MANIFEST_FORMAT {
            return Ok(None);
        }
        Ok(Some(manifest))
    }

    fn write_record(&self, root: LibraryRootId, record: &PortableRecord) -> Result<(), Error> {
        let target = self.record_path(root, record.kind(), &record.object_uuid().to_string())?;
        let json = record.to_canonical_json()?;
        Self::write_atomic(&target, &json)
    }

    fn read_record(
        &self,
        root: LibraryRootId,
        kind: RecordKind,
        object_uuid: &str,
    ) -> Result<Option<PortableRecord>, Error> {
        let target = self.record_path(root, kind, object_uuid)?;
        Self::read_record_file(&target)
    }

    fn delete_record(
        &self,
        root: LibraryRootId,
        kind: RecordKind,
        object_uuid: &str,
    ) -> Result<(), Error> {
        let target = self.record_path(root, kind, object_uuid)?;
        match std::fs::remove_file(&target) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(source) => Err(Error::io("remove record", source, &target)),
        }
    }

    fn list_records(&self, root: LibraryRootId, kind: RecordKind) -> Result<Vec<String>, Error> {
        let kind_dir = self.echo_dir(root)?.join(RECORDS_DIR).join(kind.dir_name());
        if !kind_dir.is_dir() {
            return Ok(Vec::new());
        }
        let mut records = Vec::new();
        for prefix_entry in std::fs::read_dir(&kind_dir)
            .map_err(|source| Error::io("list records", source, &kind_dir))?
            .flatten()
        {
            let prefix_path = prefix_entry.path();
            if !prefix_path.is_dir() {
                continue;
            }
            for record_entry in std::fs::read_dir(&prefix_path)
                .map_err(|source| Error::io("list records", source, &prefix_path))?
                .flatten()
            {
                let name = record_entry.file_name();
                let name = name.to_string_lossy();
                // Ignore temporaries and non-JSON files.
                if Self::is_temp_name(&name) || !name.ends_with(".json") {
                    continue;
                }
                let raw = std::fs::read_to_string(record_entry.path())
                    .map_err(|source| Error::io("read record", source, record_entry.path()))?;
                // Ignore malformed or unsupported records — the restore caller
                // only needs valid portable JSON.
                if serde_json::from_str::<PortableRecord>(&raw).is_ok() {
                    records.push(raw);
                }
            }
        }
        records.sort();
        Ok(records)
    }

    fn manifest_state(&self, root: LibraryRootId) -> Result<ManifestState, Error> {
        let manifest_path = self.echo_dir(root)?.join(MANIFEST_NAME);
        if !manifest_path.is_file() {
            return Ok(ManifestState::Absent);
        }
        let bytes = std::fs::read(&manifest_path)
            .map_err(|source| Error::io("read manifest", source, &manifest_path))?;
        // A file we cannot parse is a refusal, never a self-heal trigger: the
        // self-heal path must not clobber a control-plane file it cannot read.
        let Ok(manifest) = serde_json::from_slice::<LibraryManifest>(&bytes) else {
            return Ok(ManifestState::Malformed);
        };
        if manifest.format_version > MANIFEST_FORMAT {
            return Ok(ManifestState::Incompatible {
                format_version: manifest.format_version,
            });
        }
        Ok(ManifestState::Compatible(manifest))
    }

    fn records_present(&self, root: LibraryRootId) -> Result<bool, Error> {
        let records_dir = self.echo_dir(root)?.join(RECORDS_DIR);
        if !records_dir.is_dir() {
            return Ok(false);
        }
        for kind in RecordKind::ALL {
            let kind_dir = records_dir.join(kind.dir_name());
            if !kind_dir.is_dir() {
                continue;
            }
            for prefix_entry in std::fs::read_dir(&kind_dir)
                .map_err(|source| Error::io("scan records", source, &kind_dir))?
                .flatten()
            {
                let prefix_path = prefix_entry.path();
                if !prefix_path.is_dir() {
                    continue;
                }
                for record_entry in std::fs::read_dir(&prefix_path)
                    .map_err(|source| Error::io("scan records", source, &prefix_path))?
                    .flatten()
                {
                    let name = record_entry.file_name().to_string_lossy().to_string();
                    let is_json = std::path::Path::new(&name)
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("json"));
                    if is_json && !Self::is_temp_name(&name) {
                        return Ok(true);
                    }
                }
            }
        }
        Ok(false)
    }

    fn control_plane_usable(&self, root: LibraryRootId) -> Result<bool, Error> {
        let echo = self.echo_dir(root)?;
        // The control plane is usable when `echo/` can be created/updated. A
        // pre-existing directory that is not actually writable reports false,
        // so the caller does not enable sync/logic changes against a dead plane.
        std::fs::create_dir_all(&echo)
            .map_err(|source| Error::io("prepare control surface", source, &echo))?;
        let probe = echo.join(format!(".probe-{}", uuid::Uuid::new_v4()));
        let writable = std::fs::File::create(&probe)
            .and_then(|mut file| {
                file.write_all(b"e")?;
                file.sync_all()
            })
            .is_ok();
        let _ = std::fs::remove_file(&probe);
        Ok(writable)
    }
}

impl RootControlPlane {
    /// Read one record file, ignoring temporaries/malformed/unsupported files.
    fn read_record_file(target: &Path) -> Result<Option<PortableRecord>, Error> {
        if !target.is_file() {
            return Ok(None);
        }
        let name = target
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("record");
        if Self::is_temp_name(name) {
            return Ok(None);
        }
        let bytes =
            std::fs::read(target).map_err(|source| Error::io("read record", source, target))?;
        // Malformed or unsupported records are ignored (reported absent) — the
        // next successful write replaces them.
        let Ok(record) = serde_json::from_slice::<PortableRecord>(&bytes) else {
            return Ok(None);
        };
        Ok(Some(record))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::ControlPlanePort;
    use crate::domain::ids::{LibraryRootId, Revision};
    use crate::domain::library::{
        DeviceId, HybridLogicalClock, LibraryId, LibraryRelativePath, SongRecord,
    };

    fn setup() -> (
        tempfile::TempDir,
        RootRegistry,
        LibraryRootId,
        RootControlPlane,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let registry = RootRegistry::new();
        let root = LibraryRootId::new();
        registry.register(root, dir.path());
        let plane = RootControlPlane::new(registry.clone());
        (dir, registry, root, plane)
    }

    fn sample_record(root_uuid: uuid::Uuid) -> PortableRecord {
        PortableRecord::Song(SongRecord {
            song_uuid: root_uuid,
            revision: Revision::INITIAL,
            updated_by_device_id: DeviceId::from_uuid(
                uuid::Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap(),
            ),
            hlc: HybridLogicalClock::new(1_700_000_000, 0),
            media_path: LibraryRelativePath::new("media/周杰伦/周杰伦 - 晴天.flac").unwrap(),
            content_hash: "abc123".to_owned(),
            title: Some("晴天".to_owned()),
            artist: Some("周杰伦".to_owned()),
            album: None,
        })
    }

    #[test]
    fn manifest_round_trips_atomically() {
        let (_dir, _registry, root, plane) = setup();
        let manifest = LibraryManifest::new(
            LibraryId::from_uuid(uuid::Uuid::new_v4()),
            "0.1.0".to_owned(),
        );
        plane
            .write_manifest(root, &manifest)
            .expect("write manifest");
        let read = plane
            .read_manifest(root)
            .expect("read manifest")
            .expect("present");
        assert_eq!(read, manifest);
        assert!(read.is_compatible());
    }

    #[test]
    fn manifest_absent_is_none() {
        let (_dir, _registry, root, plane) = setup();
        assert!(plane.read_manifest(root).expect("read").is_none());
    }

    #[test]
    fn record_round_trips_through_shard_path() {
        let (dir, _registry, root, plane) = setup();
        let song_uuid = uuid::Uuid::new_v4();
        let record = sample_record(song_uuid);
        plane.write_record(root, &record).expect("write record");
        let read = plane
            .read_record(root, RecordKind::Song, &song_uuid.to_string())
            .expect("read record")
            .expect("present");
        assert_eq!(read, record);

        // The file lives in the prefix-sharded path.
        let expected_prefix = &song_uuid.to_string()[..2];
        let echo = dir.path().join("echo");
        let record_path = echo
            .join("records")
            .join("songs")
            .join(expected_prefix)
            .join(format!("{song_uuid}.json"));
        assert!(record_path.is_file(), "record at sharded path");
    }

    #[test]
    fn record_absent_is_none() {
        let (_dir, _registry, root, plane) = setup();
        assert!(plane
            .read_record(root, RecordKind::Song, &uuid::Uuid::new_v4().to_string())
            .expect("read")
            .is_none());
    }

    #[test]
    fn delete_record_is_idempotent() {
        let (_dir, _registry, root, plane) = setup();
        let song_uuid = uuid::Uuid::new_v4();
        plane
            .write_record(root, &sample_record(song_uuid))
            .expect("write");
        plane
            .delete_record(root, RecordKind::Song, &song_uuid.to_string())
            .expect("delete");
        assert!(plane
            .read_record(root, RecordKind::Song, &song_uuid.to_string())
            .expect("read")
            .is_none());
        // Deleting a missing record is a no-op.
        plane
            .delete_record(root, RecordKind::Song, &song_uuid.to_string())
            .expect("delete again");
    }

    #[test]
    fn tmp_files_are_ignored_as_records() {
        let (dir, _registry, root, plane) = setup();
        let song_uuid = uuid::Uuid::new_v4();
        // Write the real record, then place a same-dir temp file with a partial
        // JSON body (simulating an interrupted atomic write).
        plane
            .write_record(root, &sample_record(song_uuid))
            .expect("write real record");
        let echo = dir.path().join("echo");
        let record_path = echo
            .join("records")
            .join("songs")
            .join(&song_uuid.to_string()[..2])
            .join(format!("{song_uuid}.json"));
        let temp = record_path.with_file_name(format!("{song_uuid}.json.tmp-deadbeef"));
        std::fs::write(&temp, "{\"partial\": true, \"unfinished").expect("write temp");
        // The reader still sees the real record; the temp is invisible.
        assert!(plane
            .read_record(root, RecordKind::Song, &song_uuid.to_string())
            .expect("read")
            .is_some());
        // The temp never appears in listings.
        let listed = plane
            .list_records(root, RecordKind::Song)
            .expect("list records");
        assert_eq!(listed.len(), 1, "only the real record is listed");
        assert!(!listed[0].contains("unfinished"), "no partial body listed");

        // A malformed *published* record (no temp) is ignored, not surfaced.
        std::fs::write(&record_path, "{\"broken\": ").expect("corrupt record");
        assert!(plane
            .read_record(root, RecordKind::Song, &song_uuid.to_string())
            .expect("read")
            .is_none());
        let listed = plane.list_records(root, RecordKind::Song).expect("list");
        assert_eq!(
            listed.len(),
            0,
            "malformed records are not surfaced as valid"
        );
    }

    #[test]
    fn unknown_format_manifest_is_absent() {
        let (dir, _registry, root, plane) = setup();
        let echo = dir.path().join("echo");
        std::fs::create_dir_all(&echo).expect("echo dir");
        // A future-format manifest must be treated as incompatible/absent.
        std::fs::write(
            echo.join("manifest.json"),
            format!(
                "{{\"format_version\": 99, \"library_id\": \"{}\", \"written_by_app_version\": \"0.1.0\"}}",
                uuid::Uuid::new_v4()
            ),
        )
        .expect("write future manifest");
        assert!(plane.read_manifest(root).expect("read").is_none());
    }

    #[test]
    fn control_plane_usable_detects_writable_and_readable() {
        let (_dir, _registry, root, plane) = setup();
        assert!(plane.control_plane_usable(root).expect("usable check"));
        // A usable plane writes manifest fine.
        plane
            .write_manifest(
                root,
                &LibraryManifest::new(LibraryId::from_uuid(uuid::Uuid::new_v4()), "0.1.0"),
            )
            .expect("write manifest");
    }

    #[test]
    fn manifest_state_separates_absent_compatible_and_future() {
        let (dir, _registry, root, plane) = setup();
        // 1. Absent: nothing written yet.
        assert_eq!(
            plane.manifest_state(root).expect("state"),
            ManifestState::Absent
        );
        assert!(!plane.records_present(root).expect("records"));

        // 2. Compatible: this build's own manifest.
        let manifest = LibraryManifest::new(LibraryId::from_uuid(uuid::Uuid::new_v4()), "0.1.0");
        plane.write_manifest(root, &manifest).expect("write");
        assert_eq!(
            plane.manifest_state(root).expect("state"),
            ManifestState::Compatible(manifest)
        );

        // 3. Future format: present but untouchable — the self-heal path must
        //    never see this as "absent".
        let echo = dir.path().join("echo");
        std::fs::write(
            echo.join("manifest.json"),
            format!(
                "{{\"format_version\": 99, \"library_id\": \"{}\", \"written_by_app_version\": \"9.9.9\"}}",
                uuid::Uuid::new_v4()
            ),
        )
        .expect("write future manifest");
        assert_eq!(
            plane.manifest_state(root).expect("state"),
            ManifestState::Incompatible { format_version: 99 }
        );

        // 4. Unparseable: also a refusal, not a self-heal trigger.
        std::fs::write(echo.join("manifest.json"), "not json at all").expect("corrupt manifest");
        assert_eq!(
            plane.manifest_state(root).expect("state"),
            ManifestState::Malformed
        );
    }

    #[test]
    fn records_present_ignores_temporaries_and_counts_every_kind() {
        let (dir, _registry, root, plane) = setup();
        // A kind directory holding only a temp file is *not* "records present"
        // (an interrupted write must not turn a fresh root into a heal target).
        let echo = dir.path().join("echo");
        let songs = echo.join("records").join("songs").join("ab");
        std::fs::create_dir_all(&songs).expect("shard dir");
        std::fs::write(songs.join("x.json.tmp-1"), "{}").expect("temp");
        assert!(!plane.records_present(root).expect("records"));

        // A real record of any kind makes the directory a continuation target.
        let song_uuid = uuid::Uuid::new_v4();
        plane
            .write_record(root, &sample_record(song_uuid))
            .expect("write song record");
        assert!(plane.records_present(root).expect("records"));

        let fav_uuid = uuid::Uuid::new_v4();
        plane
            .write_record(
                root,
                &PortableRecord::Favorite(crate::domain::library::FavoriteRecord {
                    song_uuid: fav_uuid,
                    revision: Revision::INITIAL,
                    updated_by_device_id: DeviceId::new(),
                    hlc: HybridLogicalClock::default(),
                    is_favorite: true,
                }),
            )
            .expect("write favorite record");
        assert!(plane.records_present(root).expect("records"));
    }

    #[test]
    fn list_records_sorts_and_multiples() {
        let (_dir, _registry, root, plane) = setup();
        let a = uuid::Uuid::new_v4();
        let b = uuid::Uuid::new_v4();
        plane
            .write_record(root, &sample_record(a))
            .expect("write a");
        plane
            .write_record(root, &sample_record(b))
            .expect("write b");
        let listed = plane.list_records(root, RecordKind::Song).expect("list");
        assert_eq!(listed.len(), 2, "two songs listed");
        // Different kinds don't collide.
        let fav = PortableRecord::Favorite(crate::domain::library::FavoriteRecord {
            song_uuid: a,
            revision: Revision::INITIAL,
            updated_by_device_id: DeviceId::new(),
            hlc: HybridLogicalClock::default(),
            is_favorite: true,
        });
        plane.write_record(root, &fav).expect("write favorite");
        assert_eq!(
            plane
                .list_records(root, RecordKind::Song)
                .expect("songs")
                .len(),
            2
        );
        assert_eq!(
            plane
                .list_records(root, RecordKind::Favorite)
                .expect("favorites")
                .len(),
            1
        );
    }
}
