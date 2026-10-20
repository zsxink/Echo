//! Recoverable portable-format upgrade. A durable original-byte backup and
//! replay plan precede the future-version manifest fence; v2 is published last.

use std::collections::{BTreeMap, BTreeSet};
use std::io::{Read as _, Write as _};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::RootRegistry;
use crate::domain::ids::{LibraryRootId, RelativeMediaPath};
use crate::domain::library::{
    LibraryManifest, PortableRecord, PortableRecordDocument, PortableRecordEnvelope, RecordKind,
};
use crate::error::Error;

pub(super) const MAX_RECORD_BYTES: usize = 4 * 1024 * 1024;
const MAX_PLAN_BYTES: usize = 128 * 1024 * 1024;
const BACKUP: &str = "echo/migrations/v1-backup.json";
const JOURNAL: &str = "echo/migrations/v1-v2-journal.json";
const MANIFEST: &str = "echo/manifest.json";
pub(super) const FENCE_VERSION: u64 = u64::MAX;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Journal {
    version: u32,
    backup_digest: String,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Plan {
    version: u32,
    original_manifest: String,
    manifest: LibraryManifest,
    originals: Vec<Original>,
    updates: Vec<PortableRecordEnvelope>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Original {
    kind: RecordKind,
    uuid: uuid::Uuid,
    // None denotes the historical flat tombstone directory.
    tombstone_kind: Option<RecordKind>,
    raw: String,
}

fn invalid(reason: &str) -> Error {
    Error::conflict(format!("portable migration: {reason}"))
}

pub(super) fn path(
    registry: &RootRegistry,
    root: LibraryRootId,
    relative: &str,
) -> Result<PathBuf, Error> {
    registry.resolve_relative(root, &RelativeMediaPath::new(relative)?)
}

pub(super) fn record_relative(record: &PortableRecord) -> Result<String, Error> {
    record.object_key()?;
    let uuid = record.object_uuid();
    let deleted_kind = match record {
        PortableRecord::Tombstone(value) => Some(value.deleted_kind),
        _ => None,
    };
    Ok(original_relative(record.kind(), uuid, deleted_kind))
}

pub(super) fn original_relative(
    kind: RecordKind,
    uuid: uuid::Uuid,
    tombstone_kind: Option<RecordKind>,
) -> String {
    let id = uuid.to_string();
    let typed = tombstone_kind.map_or_else(String::new, |kind| format!("{}/", kind.dir_name()));
    format!(
        "echo/records/{}/{typed}{}/{id}.json",
        kind.dir_name(),
        &id[..2]
    )
}

pub(super) fn read(
    registry: &RootRegistry,
    root: LibraryRootId,
    relative: &str,
    limit: usize,
) -> Result<Option<String>, Error> {
    let target = path(registry, root, relative)?;
    let file = match std::fs::File::open(&target) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(Error::io("open portable file", error, &target)),
    };
    if !file
        .metadata()
        .map_err(|error| Error::io("inspect portable file", error, &target))?
        .is_file()
    {
        return Err(invalid("portable file is not a regular file"));
    }
    let mut bytes = Vec::new();
    file.take((limit + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|error| Error::io("read portable file", error, &target))?;
    if bytes.len() > limit {
        return Err(invalid("portable file exceeds its size limit"));
    }
    String::from_utf8(bytes)
        .map(Some)
        .map_err(|_| invalid("portable JSON is not UTF-8"))
}

pub(super) fn write(
    registry: &RootRegistry,
    root: LibraryRootId,
    relative: &str,
    bytes: &[u8],
) -> Result<(), Error> {
    let target = path(registry, root, relative)?;
    let parent = target.parent().ok_or_else(|| invalid("missing parent"))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| Error::io("create portable directory", error, parent))?;
    // Recheck ancestors after creating them, before any file is opened.
    let target = path(registry, root, relative)?;
    let temporary = target.with_extension(format!("json.tmp-{}", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| Error::io("create portable temporary", error, &temporary))?;
        file.write_all(bytes)
            .map_err(|error| Error::io("write portable temporary", error, &temporary))?;
        file.sync_all()
            .map_err(|error| Error::io("sync portable temporary", error, &temporary))?;
        path(registry, root, relative)?;
        std::fs::rename(&temporary, &target)
            .map_err(|error| Error::io("publish portable file", error, &target))?;
        sync_directory(parent)
    })();
    let _ = std::fs::remove_file(temporary);
    result
}

#[cfg_attr(
    windows,
    allow(clippy::missing_const_for_fn, clippy::unnecessary_wraps)
)]
fn sync_directory(directory: &std::path::Path) -> Result<(), Error> {
    #[cfg(not(windows))]
    {
        std::fs::File::open(directory)
            .and_then(|file| file.sync_all())
            .map_err(|error| Error::io("sync portable directory", error, directory))?;
    }
    #[cfg(windows)]
    let _ = directory;
    Ok(())
}

pub(super) fn remove(
    registry: &RootRegistry,
    root: LibraryRootId,
    relative: &str,
) -> Result<(), Error> {
    let target = path(registry, root, relative)?;
    match std::fs::remove_file(&target) {
        Ok(()) => sync_directory(target.parent().ok_or_else(|| invalid("missing parent"))?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(Error::io("remove portable file", error, target)),
    }
}

fn decode<T: serde::de::DeserializeOwned>(raw: &str) -> Result<T, Error> {
    serde_json::from_str(raw).map_err(|error| {
        Error::unavailable_with_source("portable migration", "invalid persisted JSON", error)
    })
}

pub(super) fn encode<T: Serialize>(value: &T, limit: usize) -> Result<Vec<u8>, Error> {
    struct BoundedJson {
        bytes: Vec<u8>,
        limit: usize,
    }
    impl std::io::Write for BoundedJson {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    "portable JSON exceeds size limit",
                ));
            }
            self.bytes.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut output = BoundedJson {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer(&mut output, value).map_err(|error| {
        Error::unavailable_with_source("portable migration", "cannot encode bounded JSON", error)
    })?;
    Ok(output.bytes)
}

/// Enumerate only bounded, typed UUID record paths. Never follow a symlink,
/// ignore an unreadable entry, or turn a malformed record into a lost object.
pub(super) fn record_files(
    registry: &RootRegistry,
    root: LibraryRootId,
    kind: RecordKind,
) -> Result<Vec<String>, Error> {
    let mut result = Vec::new();
    let directory = format!("echo/records/{}", kind.dir_name());
    collect(registry, root, &directory, 0, &mut result)?;
    result.sort();
    Ok(result)
}

fn collect(
    registry: &RootRegistry,
    root: LibraryRootId,
    relative: &str,
    depth: usize,
    result: &mut Vec<String>,
) -> Result<(), Error> {
    let directory = path(registry, root, relative)?;
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(Error::io("list portable records", error, directory)),
    };
    for entry in entries {
        let entry =
            entry.map_err(|error| Error::io("read portable record entry", error, &directory))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| invalid("non UTF-8 record path"))?;
        if name.contains(".tmp-") {
            continue;
        }
        let child = format!("{relative}/{name}");
        let target = path(registry, root, &child)?;
        let metadata = std::fs::symlink_metadata(&target)
            .map_err(|error| Error::io("inspect portable record", error, &target))?;
        if metadata.is_dir() {
            if depth >= 2 {
                return Err(invalid("record directory is too deep"));
            }
            collect(registry, root, &child, depth + 1, result)?;
        } else if std::path::Path::new(&name)
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
        {
            if result.len() >= 100_000 {
                return Err(invalid("too many portable records"));
            }
            result.push(child);
        }
    }
    Ok(())
}

fn validate(plan: &Plan) -> Result<(), Error> {
    let original: LibraryManifest = decode(&plan.original_manifest)?;
    if plan.version != 1
        || original.format_version != 1
        || plan.manifest.format_version != 2
        || original.library_id != plan.manifest.library_id
    {
        return Err(invalid(
            "plan has incompatible versions or library identity",
        ));
    }
    let mut targets = BTreeMap::new();
    for update in &plan.updates {
        update.validate()?;
        if update.sync_metadata().is_some_and(|sync| {
            sync.binding.library_id.as_uuid() != plan.manifest.library_id.as_uuid()
        }) {
            return Err(invalid("update belongs to a different library"));
        }
        encode(update, MAX_RECORD_BYTES)?;
        if targets
            .insert(record_relative(update.record())?, update)
            .is_some()
        {
            return Err(invalid("duplicate update target"));
        }
    }
    let mut sources = BTreeSet::new();
    for original in &plan.originals {
        if original.raw.len() > MAX_RECORD_BYTES {
            return Err(invalid("backup record is oversized"));
        }
        let document: PortableRecordDocument = decode(&original.raw)?;
        let record = document.record();
        if original.kind != record.kind() || original.uuid != record.object_uuid() {
            return Err(invalid("backup path disagrees with record identity"));
        }
        let source = original_relative(original.kind, original.uuid, original.tombstone_kind);
        if !sources.insert(source) {
            return Err(invalid("duplicate backup source"));
        }
        if !targets
            .get(&record_relative(record)?)
            .is_some_and(|update| update.record() == record)
        {
            return Err(invalid("an original record has no equivalent v2 update"));
        }
        if let Some(kind) = original.tombstone_kind {
            if !matches!(record, PortableRecord::Tombstone(tombstone) if tombstone.deleted_kind == kind)
            {
                return Err(invalid("backup tombstone path disagrees with deleted kind"));
            }
        }
    }
    Ok(())
}

pub(super) fn recover(registry: &RootRegistry, root: LibraryRootId) -> Result<(), Error> {
    let Some(raw) = read(registry, root, JOURNAL, 1024)? else {
        return Ok(());
    };
    let journal: Journal = decode(&raw)?;
    if journal.version != 1 {
        return Err(invalid("unsupported journal version"));
    }
    let backup = read(registry, root, BACKUP, MAX_PLAN_BYTES)?
        .ok_or_else(|| invalid("journal backup is missing"))?;
    if blake3::hash(backup.as_bytes()).to_hex().as_str() != journal.backup_digest {
        return Err(invalid("journal backup digest mismatch"));
    }
    let plan: Plan = decode(&backup)?;
    validate(&plan)?;
    replay(registry, root, &plan, &mut |_, _| Ok(()))
}

pub(super) fn apply(
    registry: &RootRegistry,
    root: LibraryRootId,
    updates: &[PortableRecordEnvelope],
    manifest: &LibraryManifest,
) -> Result<(), Error> {
    apply_with_hook(registry, root, updates, manifest, &mut |_, _| Ok(()))
}

fn apply_with_hook(
    registry: &RootRegistry,
    root: LibraryRootId,
    updates: &[PortableRecordEnvelope],
    manifest: &LibraryManifest,
    hook: &mut dyn FnMut(&str, usize) -> Result<(), Error>,
) -> Result<(), Error> {
    if read(registry, root, JOURNAL, 1024)?.is_some() {
        return recover(registry, root);
    }
    let original_manifest = read(registry, root, MANIFEST, MAX_RECORD_BYTES)?
        .ok_or_else(|| invalid("original manifest is missing"))?;
    let current: LibraryManifest = decode(&original_manifest)?;
    if current.format_version == 2 && current == *manifest {
        return Ok(());
    }
    if updates.len() > 100_000 {
        return Err(invalid("too many migration updates"));
    }
    let mut retained_bytes = 0usize;
    for update in updates {
        retained_bytes = retained_bytes.saturating_add(encode(update, MAX_RECORD_BYTES)?.len());
        if retained_bytes > MAX_PLAN_BYTES / 2 {
            return Err(invalid("migration plan exceeds its memory limit"));
        }
    }
    let mut plan = Plan {
        version: 1,
        original_manifest,
        manifest: manifest.clone(),
        originals: Vec::new(),
        updates: updates.to_vec(),
    };
    plan.updates
        .sort_by_cached_key(|update| record_relative(update.record()).unwrap_or_default());
    for &kind in RecordKind::ALL {
        for relative in record_files(registry, root, kind)? {
            let raw = read(registry, root, &relative, MAX_RECORD_BYTES)?
                .ok_or_else(|| invalid("record disappeared during preparation"))?;
            retained_bytes = retained_bytes.saturating_add(raw.len());
            if retained_bytes > MAX_PLAN_BYTES || plan.originals.len() >= 100_000 {
                return Err(invalid("migration backup exceeds its memory limit"));
            }
            let document: PortableRecordDocument = decode(&raw)?;
            let record = document.record();
            if record.kind() != kind {
                return Err(invalid("record stored under the wrong kind"));
            }
            let typed = record_relative(record)?;
            let legacy = original_relative(kind, record.object_uuid(), None);
            let tombstone_kind = if relative == typed && relative != legacy {
                match record {
                    PortableRecord::Tombstone(value) => Some(value.deleted_kind),
                    _ => None,
                }
            } else if relative == legacy {
                None
            } else {
                return Err(invalid("record stored under the wrong UUID path"));
            };
            plan.originals.push(Original {
                kind,
                uuid: record.object_uuid(),
                tombstone_kind,
                raw,
            });
        }
    }
    validate(&plan)?;
    let backup = encode(&plan, MAX_PLAN_BYTES)?;
    if let Some(existing) = read(registry, root, BACKUP, MAX_PLAN_BYTES)? {
        if existing.as_bytes() != backup {
            return Err(invalid(
                "an existing original backup must not be overwritten",
            ));
        }
    } else {
        write(registry, root, BACKUP, &backup)?;
    }
    let journal = Journal {
        version: 1,
        backup_digest: blake3::hash(&backup).to_hex().to_string(),
    };
    write(registry, root, JOURNAL, &encode(&journal, 1024)?)?;
    hook("journal", 0)?;
    replay(registry, root, &plan, hook)
}

fn replay(
    registry: &RootRegistry,
    root: LibraryRootId,
    plan: &Plan,
    hook: &mut dyn FnMut(&str, usize) -> Result<(), Error>,
) -> Result<(), Error> {
    let current: LibraryManifest = decode(
        &read(registry, root, MANIFEST, MAX_RECORD_BYTES)?
            .ok_or_else(|| invalid("manifest disappeared"))?,
    )?;
    if current.library_id != plan.manifest.library_id
        || ![1, 2, FENCE_VERSION].contains(&current.format_version)
    {
        return Err(invalid(
            "current manifest differs from the persisted migration",
        ));
    }
    let mut fence = plan.manifest.clone();
    fence.format_version = FENCE_VERSION;
    write(registry, root, MANIFEST, &encode(&fence, MAX_RECORD_BYTES)?)?;
    hook("fence", 0)?;
    for (index, update) in plan.updates.iter().enumerate() {
        hook("record", index)?;
        write(
            registry,
            root,
            &record_relative(update.record())?,
            &encode(update, MAX_RECORD_BYTES)?,
        )?;
    }
    for original in &plan.originals {
        if original.kind == RecordKind::Tombstone && original.tombstone_kind.is_none() {
            remove(
                registry,
                root,
                &original_relative(original.kind, original.uuid, None),
            )?;
        }
    }
    hook("manifest", plan.updates.len())?;
    write(
        registry,
        root,
        MANIFEST,
        &encode(&plan.manifest, MAX_RECORD_BYTES)?,
    )?;
    // The manifest is the commit point. A cleanup failure leaves a complete,
    // recoverable plan; it must not report an uncommitted migration.
    if let Err(error) = remove(registry, root, JOURNAL) {
        tracing::warn!(
            code = error.code(),
            "portable migration journal cleanup deferred"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::{ControlPlanePort, ManifestState};
    use crate::domain::ids::Revision;
    use crate::domain::library::{
        DeviceId, HybridLogicalClock, LibraryId, LibraryRelativePath, SongRecord, TombstoneRecord,
    };
    use crate::infrastructure::filesystem::RootControlPlane;

    fn fixture() -> (
        tempfile::TempDir,
        RootRegistry,
        LibraryRootId,
        LibraryManifest,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let registry = RootRegistry::new();
        let root = LibraryRootId::new();
        registry.register(root, dir.path());
        let target =
            LibraryManifest::new(LibraryId::from_uuid(uuid::Uuid::new_v4()), "upgrade-test");
        let mut original = target.clone();
        original.format_version = 1;
        write(
            &registry,
            root,
            MANIFEST,
            &serde_json::to_vec(&original).unwrap(),
        )
        .unwrap();
        (dir, registry, root, target)
    }

    fn song() -> PortableRecord {
        PortableRecord::Song(SongRecord {
            song_uuid: uuid::Uuid::new_v4(),
            revision: Revision::from_u64(7),
            updated_by_device_id: DeviceId::new(),
            hlc: HybridLogicalClock::new(1234, 9),
            media_path: LibraryRelativePath::new("media/历史/一首歌.flac").unwrap(),
            content_hash: "old-content-hash".to_owned(),
            added_at: 1_234_567_890,
            title: Some("历史标题".to_owned()),
            artist: Some("作者".to_owned()),
            album: None,
        })
    }

    fn seed(registry: &RootRegistry, root: LibraryRootId, record: &PortableRecord) -> String {
        // Unknown fields and noncanonical whitespace must survive in the backup
        // byte for byte, independently from typed deserialization.
        let mut value = serde_json::to_value(record).unwrap();
        value["historical_extension"] = serde_json::json!({"未知": [1, "原字段"]});
        let raw = format!("\n{}\n", serde_json::to_string_pretty(&value).unwrap());
        write(
            registry,
            root,
            &original_relative(record.kind(), record.object_uuid(), None),
            raw.as_bytes(),
        )
        .unwrap();
        raw
    }

    #[test]
    fn partial_upgrade_is_fenced_and_new_adapter_recovers_the_persisted_plan() {
        let (_dir, registry, root, target) = fixture();
        let records = [song(), song()];
        let originals: Vec<_> = records
            .iter()
            .map(|record| seed(&registry, root, record))
            .collect();
        let updates: Vec<_> = records
            .iter()
            .map(|record| PortableRecordEnvelope::new(record.clone(), None).unwrap())
            .collect();
        let error = apply_with_hook(&registry, root, &updates, &target, &mut |phase, index| {
            if phase == "record" && index == 1 {
                return Err(invalid("simulated disk failure"));
            }
            Ok(())
        })
        .unwrap_err();
        assert_eq!(error.code(), "conflict");
        let manifest: LibraryManifest = decode(
            &read(&registry, root, MANIFEST, MAX_RECORD_BYTES)
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(manifest.format_version, FENCE_VERSION);
        // This is exactly the version check performed by the v1 open path.
        assert!(manifest.format_version > 1);
        let plan: Plan = decode(
            &read(&registry, root, BACKUP, MAX_PLAN_BYTES)
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(plan.originals.len(), 2);
        for raw in &originals {
            assert!(plan.originals.iter().any(|original| &original.raw == raw));
        }
        let published: Vec<PortableRecordDocument> = records
            .iter()
            .map(|record| {
                decode(
                    &read(
                        &registry,
                        root,
                        &record_relative(record).unwrap(),
                        MAX_RECORD_BYTES,
                    )
                    .unwrap()
                    .unwrap(),
                )
                .unwrap()
            })
            .collect();
        assert_eq!(
            published
                .iter()
                .filter(|document| matches!(document, PortableRecordDocument::V2(_)))
                .count(),
            1
        );
        // A fresh adapter recovers before listing any mixed-version records.
        let restarted = RootControlPlane::new(registry.clone());
        let documents = restarted
            .list_record_documents(root, RecordKind::Song)
            .unwrap();
        assert_eq!(documents.len(), 2);
        assert!(documents
            .iter()
            .all(|document| matches!(document, PortableRecordDocument::V2(_))));
        for record in &records {
            assert!(documents.iter().any(|document| document.record() == record));
        }
        assert_eq!(restarted.read_manifest(root).unwrap(), Some(target));
        assert!(read(&registry, root, JOURNAL, 1024).unwrap().is_none());
        assert!(read(&registry, root, BACKUP, MAX_PLAN_BYTES)
            .unwrap()
            .is_some());
    }

    #[test]
    fn journal_before_fence_is_recoverable_and_formal_manifest_is_last() {
        let (_dir, registry, root, target) = fixture();
        let record = song();
        seed(&registry, root, &record);
        let envelope = PortableRecordEnvelope::new(record.clone(), None).unwrap();
        apply_with_hook(&registry, root, &[envelope], &target, &mut |phase, _| {
            if phase == "journal" {
                return Err(invalid("crash before fence"));
            }
            Ok(())
        })
        .unwrap_err();
        assert_eq!(
            decode::<LibraryManifest>(
                &read(&registry, root, MANIFEST, MAX_RECORD_BYTES)
                    .unwrap()
                    .unwrap()
            )
            .unwrap()
            .format_version,
            1
        );
        let backup: Plan = decode(
            &read(&registry, root, BACKUP, MAX_PLAN_BYTES)
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        replay(&registry, root, &backup, &mut |phase, _| {
            if phase == "manifest" {
                let manifest: LibraryManifest =
                    decode(&read(&registry, root, MANIFEST, MAX_RECORD_BYTES)?.unwrap())?;
                assert_eq!(manifest.format_version, FENCE_VERSION);
                assert!(matches!(
                    decode::<PortableRecordDocument>(
                        &read(
                            &registry,
                            root,
                            &record_relative(&record)?,
                            MAX_RECORD_BYTES
                        )?
                        .unwrap()
                    )?,
                    PortableRecordDocument::V2(_)
                ));
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(
            RootControlPlane::new(registry).read_manifest(root).unwrap(),
            Some(target)
        );
    }

    #[test]
    fn unmapped_original_aborts_before_any_journal_or_fence() {
        let (_dir, registry, root, target) = fixture();
        let record = song();
        let raw = seed(&registry, root, &record);
        assert!(apply(&registry, root, &[], &target).is_err());
        assert!(read(&registry, root, BACKUP, MAX_PLAN_BYTES)
            .unwrap()
            .is_none());
        assert!(read(&registry, root, JOURNAL, 1024).unwrap().is_none());
        assert_eq!(
            read(
                &registry,
                root,
                &record_relative(&record).unwrap(),
                MAX_RECORD_BYTES
            )
            .unwrap(),
            Some(raw)
        );
        assert_eq!(
            RootControlPlane::new(registry)
                .read_manifest(root)
                .unwrap()
                .unwrap()
                .format_version,
            1
        );
    }

    #[test]
    fn corrupt_backup_stops_reads_and_writes_without_clearing_the_fence() {
        let (_dir, registry, root, target) = fixture();
        let record = song();
        seed(&registry, root, &record);
        apply_with_hook(
            &registry,
            root,
            &[PortableRecordEnvelope::new(record.clone(), None).unwrap()],
            &target,
            &mut |phase, _| {
                if phase == "fence" {
                    return Err(invalid("crash"));
                }
                Ok(())
            },
        )
        .unwrap_err();
        write(&registry, root, BACKUP, b"{}").unwrap();
        let plane = RootControlPlane::new(registry.clone());
        assert!(plane.list_records(root, RecordKind::Song).is_err());
        assert!(plane.write_record(root, &record).is_err());
        assert!(plane
            .delete_record(root, RecordKind::Song, &record.object_uuid().to_string())
            .is_err());
        assert!(plane.write_manifest(root, &target).is_err());
        assert_eq!(
            decode::<LibraryManifest>(
                &read(&registry, root, MANIFEST, MAX_RECORD_BYTES)
                    .unwrap()
                    .unwrap()
            )
            .unwrap()
            .format_version,
            FENCE_VERSION
        );
    }

    #[test]
    fn typed_tombstones_with_the_same_uuid_coexist_and_delete_independently() {
        let (_dir, registry, root, target) = fixture();
        apply(&registry, root, &[], &target).unwrap();
        let plane = RootControlPlane::new(registry);
        let uuid = uuid::Uuid::new_v4();
        let tombstone = |kind| {
            PortableRecordEnvelope::new(
                PortableRecord::Tombstone(TombstoneRecord {
                    object_uuid: uuid,
                    deleted_kind: kind,
                    revision: Revision::INITIAL,
                    updated_by_device_id: DeviceId::new(),
                    hlc: HybridLogicalClock::default(),
                }),
                None,
            )
            .unwrap()
        };
        let song = tombstone(RecordKind::Song);
        let favorite = tombstone(RecordKind::Favorite);
        plane.write_record_envelope(root, &song).unwrap();
        plane.write_record_envelope(root, &favorite).unwrap();
        assert_eq!(
            plane
                .list_record_documents(root, RecordKind::Tombstone)
                .unwrap()
                .len(),
            2
        );
        assert_eq!(
            plane
                .read_tombstone_document(root, RecordKind::Song, &uuid.to_string())
                .unwrap(),
            Some(PortableRecordDocument::V2(song))
        );
        assert!(plane
            .read_record(root, RecordKind::Tombstone, &uuid.to_string())
            .is_err());
        assert!(plane
            .delete_record(root, RecordKind::Tombstone, &uuid.to_string())
            .is_err());
        plane
            .delete_tombstone(root, RecordKind::Song, &uuid.to_string())
            .unwrap();
        assert_eq!(
            plane
                .read_tombstone_document(root, RecordKind::Favorite, &uuid.to_string())
                .unwrap(),
            Some(PortableRecordDocument::V2(favorite))
        );
    }

    #[test]
    fn migration_moves_flat_tombstones_only_after_an_equivalent_typed_write() {
        let (_dir, registry, root, target) = fixture();
        let record = PortableRecord::Tombstone(TombstoneRecord {
            object_uuid: uuid::Uuid::new_v4(),
            deleted_kind: RecordKind::Favorite,
            revision: Revision::INITIAL,
            updated_by_device_id: DeviceId::new(),
            hlc: HybridLogicalClock::default(),
        });
        let raw = seed(&registry, root, &record);
        let flat = original_relative(record.kind(), record.object_uuid(), None);
        apply(
            &registry,
            root,
            &[PortableRecordEnvelope::new(record.clone(), None).unwrap()],
            &target,
        )
        .unwrap();
        assert!(read(&registry, root, &flat, MAX_RECORD_BYTES)
            .unwrap()
            .is_none());
        assert!(read(
            &registry,
            root,
            &record_relative(&record).unwrap(),
            MAX_RECORD_BYTES
        )
        .unwrap()
        .is_some());
        let backup: Plan = decode(
            &read(&registry, root, BACKUP, MAX_PLAN_BYTES)
                .unwrap()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(backup.originals[0].raw, raw);
    }

    #[test]
    fn future_format_and_uuid_paths_are_rejected_before_mutation() {
        let (_dir, registry, root, mut target) = fixture();
        target.format_version = FENCE_VERSION;
        write(
            &registry,
            root,
            MANIFEST,
            &serde_json::to_vec(&target).unwrap(),
        )
        .unwrap();
        let plane = RootControlPlane::new(registry);
        assert!(matches!(
            plane.manifest_state(root).unwrap(),
            ManifestState::Incompatible { .. }
        ));
        assert!(plane.write_record(root, &song()).is_err());
        assert!(plane.write_manifest(root, &target).is_err());
        assert!(plane
            .read_record(root, RecordKind::Song, "../../outside")
            .is_err());
        assert!(plane
            .delete_record(root, RecordKind::Song, "../../outside")
            .is_err());
    }

    #[cfg(unix)]
    #[test]
    fn record_symlink_escape_fails_before_a_backup_or_fence() {
        let (dir, registry, root, target) = fixture();
        let foreign = tempfile::tempdir().unwrap();
        let record = song();
        let relative = record_relative(&record).unwrap();
        let target_file = dir.path().join(&relative);
        std::fs::create_dir_all(target_file.parent().unwrap()).unwrap();
        let outside = foreign.path().join("record.json");
        std::fs::write(&outside, serde_json::to_vec(&record).unwrap()).unwrap();
        std::os::unix::fs::symlink(&outside, &target_file).unwrap();
        assert!(apply(
            &registry,
            root,
            &[PortableRecordEnvelope::new(record, None).unwrap()],
            &target
        )
        .is_err());
        assert!(read(&registry, root, BACKUP, MAX_PLAN_BYTES)
            .unwrap()
            .is_none());
        assert_eq!(
            RootControlPlane::new(registry)
                .read_manifest(root)
                .unwrap()
                .unwrap()
                .format_version,
            1
        );
    }
    #[test]
    fn actual_mid_write_filesystem_failure_recovers_after_the_blocker_is_removed() {
        let (_dir, registry, root, target) = fixture();
        let mut records = vec![song(), song()];
        records.sort_by_cached_key(|record| record_relative(record).unwrap());
        for record in &records {
            seed(&registry, root, record);
        }
        let updates: Vec<_> = records
            .iter()
            .map(|record| PortableRecordEnvelope::new(record.clone(), None).unwrap())
            .collect();
        let blocked = path(&registry, root, &record_relative(&records[1]).unwrap()).unwrap();
        let result = apply_with_hook(&registry, root, &updates, &target, &mut |phase, index| {
            if phase == "record" && index == 1 {
                std::fs::remove_file(&blocked).unwrap();
                std::fs::create_dir(&blocked).unwrap();
            }
            Ok(())
        });
        assert!(
            result.is_err(),
            "rename onto an occupied directory must fail"
        );
        assert_eq!(
            decode::<LibraryManifest>(
                &read(&registry, root, MANIFEST, MAX_RECORD_BYTES)
                    .unwrap()
                    .unwrap()
            )
            .unwrap()
            .format_version,
            FENCE_VERSION
        );
        std::fs::remove_dir(&blocked).unwrap();
        // Retry deliberately passes no updates: only the durable original plan
        // may decide what is replayed after a partial write.
        apply(&registry, root, &[], &target).unwrap();
        let plane = RootControlPlane::new(registry);
        assert_eq!(plane.read_manifest(root).unwrap(), Some(target));
        for record in &records {
            assert_eq!(
                plane
                    .read_record(root, record.kind(), &record.object_uuid().to_string())
                    .unwrap(),
                Some(record.clone())
            );
        }
    }

    #[test]
    fn oversized_serialized_records_are_rejected_before_a_plan_is_published() {
        let (_dir, registry, root, target) = fixture();
        let mut record = song();
        if let PortableRecord::Song(value) = &mut record {
            value.title = Some("x".repeat(MAX_RECORD_BYTES));
        }
        let envelope = PortableRecordEnvelope::new(record, None).unwrap();
        assert!(apply(&registry, root, &[envelope], &target).is_err());
        assert!(read(&registry, root, BACKUP, MAX_PLAN_BYTES)
            .unwrap()
            .is_none());
        assert!(read(&registry, root, JOURNAL, 1024).unwrap().is_none());
        assert_eq!(
            RootControlPlane::new(registry)
                .read_manifest(root)
                .unwrap()
                .unwrap()
                .format_version,
            1
        );
    }
}
