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
mod tests;
