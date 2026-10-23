//! Coverage images bound recovery work to current state and an active suffix.
//! Operation receipts are addressed individually; resources are never deleted.
use super::{decode, invalid, io, manifest, portable, receipt, relative, Hook, ANCHOR, LIMIT};
use crate::domain::ids::LibraryRootId;
use crate::domain::library::durability::{
    DurableStateChange, LibraryBatch, LibraryCheckpoint, LibraryReplay,
};
use crate::domain::library::{PortableRecordEnvelope, PortableSerialize};
use crate::error::Error;
use crate::infrastructure::filesystem::RootRegistry;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Coverage {
    version: u32,
    image: LibraryCheckpoint,
    digest: String,
}

pub(super) fn load(
    registry: &RootRegistry,
    cache: &super::ValidationCache,
    root: LibraryRootId,
) -> Result<Option<LibraryCheckpoint>, Error> {
    let Some(raw) = portable::read(registry, root, ANCHOR, LIMIT)? else {
        return Ok(None);
    };
    let coverage: Coverage = decode(&raw)?;
    let image = coverage.image;
    // Ownership is contextual and must never be certified by a body cache.
    if image.library_id != manifest(registry, root)?.library_id {
        return Err(invalid("coverage checkpoint identity or checksum mismatch"));
    }
    cache.validate(super::DocumentKind::Coverage, root, &raw, || {
        if coverage.version != 1
            || image.library_commit_seq == 0
            || !crate::domain::library::durability::valid_digest(&image.head_digest)
            || image.content_hash()? != coverage.digest
        {
            return Err(invalid("coverage checkpoint identity or checksum mismatch"));
        }
        let mut validation = LibraryBatch::new(
            uuid::Uuid::from_u128(1),
            image.library_id,
            crate::domain::library::durability::ChangeOrigin::Recovery,
            image.records.clone(),
        );
        validation.state.clone_from(&image.state);
        validation.resources.clone_from(&image.resources);
        validation.validate()
    })?;
    Ok(Some(image))
}

pub(super) fn state_path(state: &DurableStateChange) -> String {
    format!("echo/state/{}/{}.json", state.kind.directory(), state.id)
}

pub(super) fn changed_paths(replay: &LibraryReplay, seq: u64) -> Result<BTreeSet<String>, Error> {
    let mut result = BTreeSet::new();
    if let Some(image) = replay
        .checkpoint
        .as_ref()
        .filter(|c| c.library_commit_seq > seq)
    {
        for record in &image.records {
            result.insert(portable::record_relative(record.record())?);
        }
        result.extend(image.state.iter().map(state_path));
    }
    for commit in replay.commits.iter().filter(|c| c.library_commit_seq > seq) {
        for record in &commit.batch.records {
            result.insert(portable::record_relative(record.record())?);
        }
        result.extend(commit.batch.state.iter().map(state_path));
    }
    Ok(result)
}

pub(super) fn materialize(
    registry: &RootRegistry,
    root: LibraryRootId,
    records: &[PortableRecordEnvelope],
    state: &[DurableStateChange],
    files: &mut BTreeMap<String, String>,
    hook: &mut Hook<'_>,
) -> Result<(), Error> {
    for (index, record) in records.iter().enumerate() {
        hook("record", index)?;
        let path = portable::record_relative(record.record())?;
        let bytes = portable::encode(record, portable::MAX_RECORD_BYTES)?;
        materialize_file(registry, root, path, &bytes, files, "record", hook)?;
    }
    for (index, state) in state.iter().enumerate() {
        hook("state", index)?;
        let path = state_path(state);
        let bytes = portable::encode(state, LIMIT)?;
        materialize_file(registry, root, path, &bytes, files, "state", hook)?;
    }
    Ok(())
}

// files contains only the previous durable checkpoint plus publications that
// completed all native sync boundaries in this recovery. Equal bytes on disk
// alone do not prove durability: a failed rename/directory-sync must be redone.
fn materialize_file(
    registry: &RootRegistry,
    root: LibraryRootId,
    path: String,
    bytes: &[u8],
    files: &mut BTreeMap<String, String>,
    label: &str,
    hook: &mut Hook<'_>,
) -> Result<(), Error> {
    let digest = blake3::hash(bytes).to_hex().to_string();
    if files.get(&path) == Some(&digest) {
        // Constrained reading also rechecks the path for symlinks. A durable
        // checkpoint is evidence to verify, never permission to repair tampering.
        let raw = portable::read(registry, root, &path, LIMIT)?
            .ok_or_else(|| invalid("checkpoint file missing"))?;
        if raw.as_bytes() != bytes {
            return Err(invalid("checkpoint file digest mismatch"));
        }
    } else {
        io::publish(registry, root, &path, bytes, false, label, hook)?;
        files.insert(path, digest);
    }
    Ok(())
}

pub(super) fn compact(
    registry: &RootRegistry,
    cache: &super::ValidationCache,
    root: LibraryRootId,
    replay: &LibraryReplay,
    hook: &mut Hook<'_>,
) -> Result<(), Error> {
    let Some((seq, head)) = replay.head() else {
        return Ok(());
    };
    let mut records = BTreeMap::new();
    let mut states = BTreeMap::new();
    let mut resources = BTreeMap::new();
    if let Some(image) = &replay.checkpoint {
        for record in &image.records {
            records.insert(
                serde_json::to_string(&record.record().object_key()?).map_err(|e| {
                    Error::unavailable_with_source("coverage key", "cannot encode typed object", e)
                })?,
                record.clone(),
            );
        }
        for state in &image.state {
            states.insert(state.storage_key()?, state.clone());
        }
        for resource in &image.resources {
            resources.insert(resource.relative_path.clone(), resource.clone());
        }
    }
    for commit in &replay.commits {
        for record in &commit.batch.records {
            records.insert(
                serde_json::to_string(&record.record().object_key()?).map_err(|e| {
                    Error::unavailable_with_source("coverage key", "cannot encode typed object", e)
                })?,
                record.clone(),
            );
        }
        for state in &commit.batch.state {
            states.insert(state.storage_key()?, state.clone());
        }
        for resource in &commit.batch.resources {
            resources.insert(resource.relative_path.clone(), resource.clone());
        }
        let path = receipt(commit.batch.operation_id);
        let bytes = portable::encode(commit, LIMIT)?;
        if let Some(raw) = portable::read(registry, root, &path, LIMIT)? {
            if raw.as_bytes() != bytes {
                return Err(invalid("immutable operation receipt collision"));
            }
            io::flush_existing(registry, root, &path, "receipt_retry", hook)?;
        } else {
            io::publish(registry, root, &path, &bytes, true, "receipt", hook)?;
        }
    }
    let image = LibraryCheckpoint {
        library_id: manifest(registry, root)?.library_id,
        library_commit_seq: seq,
        head_digest: head.to_owned(),
        records: records.into_values().collect(),
        state: states.into_values().collect(),
        resources: resources.into_values().collect(),
    };
    let coverage = Coverage {
        version: 1,
        digest: image.content_hash()?,
        image,
    };
    hook("coverage", 0)?;
    io::publish(
        registry,
        root,
        ANCHOR,
        &portable::encode(&coverage, LIMIT)?,
        false,
        "coverage",
        hook,
    )?;
    // No journal is removed until the full image, anchor and original-operation
    // receipts have each crossed their file and ancestor-directory sync boundary.
    cleanup_covered(registry, cache, root, seq, hook)
}

pub(super) fn cleanup_covered(
    registry: &RootRegistry,
    cache: &super::ValidationCache,
    root: LibraryRootId,
    seq: u64,
    hook: &mut Hook<'_>,
) -> Result<(), Error> {
    let directory = portable::path(registry, root, "echo/transactions")?;
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(Error::io("list covered transactions", e, &directory)),
    };
    let mut anchor_flushed = false;
    let mut covered = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| Error::io("read covered transaction", e, &directory))?;
        let id = entry
            .file_name()
            .to_str()
            .and_then(|raw| raw.parse::<uuid::Uuid>().ok())
            .ok_or_else(|| invalid("invalid covered transaction directory"))?;
        let Some(original) = super::read_operation(registry, cache, root, id)? else {
            continue;
        };
        if original.library_commit_seq > seq {
            continue;
        }
        if portable::read(registry, root, &receipt(id), LIMIT)?.is_none() {
            return Err(invalid("cannot collect transaction without receipt"));
        }
        // Recovery can observe an anchor or receipt whose rename completed but
        // whose ancestor sync failed. Before destroying its redo alternative,
        // establish both durable copies anew; no in-memory success is retained.
        if !anchor_flushed {
            if load(registry, cache, root)?.is_none_or(|image| image.library_commit_seq != seq) {
                return Err(invalid(
                    "cannot collect without matching coverage checkpoint",
                ));
            }
            io::flush_existing(registry, root, ANCHOR, "coverage_gc", hook)?;
            anchor_flushed = true;
        }
        io::flush_existing(registry, root, &receipt(id), "receipt_gc", hook)?;
        covered.push(id);
    }
    // Finish all proof syncs before the first destructive step. A subsequent
    // receipt failure therefore leaves the entire redo suffix untouched.
    for id in covered {
        for name in ["prepared.json", "committed.json"] {
            let path = portable::path(registry, root, &relative(id, name))?;
            match std::fs::remove_file(&path) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(Error::io("collect covered journal", e, &path)),
            }
            hook("gc:remove", usize::from(name == "committed.json"))?;
        }
        let path = portable::path(registry, root, &format!("echo/transactions/{id}"))?;
        // Only remove the known journal files and empty directory. Unknown
        // evidence is preserved and blocks writes instead of recursive deletion.
        std::fs::remove_dir(&path)
            .map_err(|e| Error::io("collect covered transaction directory", e, &path))?;
        io::sync_ancestors(registry, root, &directory, "gc", hook)?;
    }
    Ok(())
}
