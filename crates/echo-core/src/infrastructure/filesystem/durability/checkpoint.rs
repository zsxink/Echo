//! Coverage images bound recovery work to current state and an active suffix.
//! Operation receipts are addressed individually; resources are never deleted.
use super::{
    decode, invalid, io, manifest, portable, receipt, relative, snapshot, Hook, ANCHOR, LIMIT,
};
use crate::domain::ids::LibraryRootId;
use crate::domain::library::durability::{
    DurableResource, DurableStateChange, LibraryBatch, LibraryCheckpoint, LibraryReplay,
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    record_snapshot: Option<snapshot::Snapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    state_snapshot: Option<snapshot::Snapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    resource_snapshot: Option<snapshot::Snapshot>,
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
    let mut image = coverage.image;
    match (
        coverage.version,
        &coverage.record_snapshot,
        &coverage.state_snapshot,
        &coverage.resource_snapshot,
    ) {
        (1, None, None, None) => {}
        (2, Some(records), Some(states), Some(resources))
            if image.records.is_empty() && image.state.is_empty() && image.resources.is_empty() =>
        {
            let records: BTreeMap<String, PortableRecordEnvelope> =
                snapshot::load(registry, root, records)?;
            for (key, record) in &records {
                if serde_json::to_string(&record.record().object_key()?).map_err(|error| {
                    Error::unavailable_with_source(
                        "snapshot record",
                        "cannot encode typed key",
                        error,
                    )
                })? != *key
                {
                    return Err(invalid("snapshot record key mismatch"));
                }
            }
            let states: BTreeMap<String, DurableStateChange> =
                snapshot::load(registry, root, states)?;
            for (key, state) in &states {
                if state.storage_key()? != *key {
                    return Err(invalid("snapshot state key mismatch"));
                }
            }
            let resources: BTreeMap<String, DurableResource> =
                snapshot::load(registry, root, resources)?;
            if resources
                .iter()
                .any(|(key, resource)| key != &resource.relative_path)
            {
                return Err(invalid("snapshot resource key mismatch"));
            }
            image.records = records.into_values().collect();
            image.state = states.into_values().collect();
            image.resources = resources.into_values().collect();
        }
        _ => return Err(invalid("unsupported coverage snapshot")),
    }
    // Ownership is contextual and must never be certified by a body cache.
    if image.library_id != manifest(registry, root)?.library_id {
        return Err(invalid("coverage checkpoint identity or checksum mismatch"));
    }
    cache.validate(super::DocumentKind::Coverage, root, &raw, || {
        if image.library_commit_seq == 0
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
    let mut directories = io::DirectorySyncSet::default();
    for (index, record) in records.iter().enumerate() {
        hook("record", index)?;
        let path = portable::record_relative(record.record())?;
        let bytes = portable::encode(record, portable::MAX_RECORD_BYTES)?;
        materialize_file(
            registry,
            root,
            path,
            &bytes,
            files,
            "record",
            hook,
            &mut directories,
        )?;
    }
    for (index, state) in state.iter().enumerate() {
        hook("state", index)?;
        let path = state_path(state);
        let bytes = portable::encode(state, LIMIT)?;
        materialize_file(
            registry,
            root,
            path,
            &bytes,
            files,
            "state",
            hook,
            &mut directories,
        )?;
    }
    // The subsequent checkpoint is the visibility boundary for the entire
    // materialization. Sync each shared ancestor once before publishing it.
    directories.sync(registry, root, "materialization", hook)
}

/// Advance the in-memory materialization index for a journal-only commit.
/// These hashes are capacity metadata until the background materializer writes
/// the actual files and publishes a checkpoint.
pub(super) fn track(
    records: &[PortableRecordEnvelope],
    state: &[DurableStateChange],
    files: &mut BTreeMap<String, String>,
) -> Result<(), Error> {
    for record in records {
        let bytes = portable::encode(record, portable::MAX_RECORD_BYTES)?;
        files.insert(
            portable::record_relative(record.record())?,
            blake3::hash(&bytes).to_hex().to_string(),
        );
    }
    for state in state {
        let bytes = portable::encode(state, LIMIT)?;
        files.insert(state_path(state), blake3::hash(&bytes).to_hex().to_string());
    }
    Ok(())
}

// files contains only the previous durable checkpoint plus publications that
// completed all native sync boundaries in this recovery. Equal bytes on disk
// alone do not prove durability: a failed rename/directory-sync must be redone.
#[allow(clippy::too_many_arguments)]
fn materialize_file(
    registry: &RootRegistry,
    root: LibraryRootId,
    path: String,
    bytes: &[u8],
    files: &mut BTreeMap<String, String>,
    label: &str,
    hook: &mut Hook<'_>,
    directories: &mut io::DirectorySyncSet,
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
        io::publish_batched(
            registry,
            root,
            &path,
            bytes,
            false,
            label,
            hook,
            directories,
        )?;
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
    let contents = Contents::merge(replay, None)?;
    let mut image = LibraryCheckpoint {
        library_id: manifest(registry, root)?.library_id,
        library_commit_seq: seq,
        head_digest: head.to_owned(),
        records: contents.records.values().cloned().collect(),
        state: contents.states.values().cloned().collect(),
        resources: contents.resources.values().cloned().collect(),
    };
    let digest = image.content_hash()?;
    let record_snapshot = snapshot::publish(registry, root, &contents.records, hook)?;
    let state_snapshot = snapshot::publish(registry, root, &contents.states, hook)?;
    let resource_snapshot = snapshot::publish(registry, root, &contents.resources, hook)?;
    image.records.clear();
    image.state.clear();
    image.resources.clear();
    for commit in &replay.commits {
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
    let coverage = Coverage {
        version: 2,
        digest,
        image,
        record_snapshot: Some(record_snapshot),
        state_snapshot: Some(state_snapshot),
        resource_snapshot: Some(resource_snapshot),
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
    // The full graph (both roots and every referenced leaf) is re-flushed before
    // destructive GC. A visible root alone is not proof after an interrupted sync.
    let materialized = super::materialization_checkpoint(registry, root)?;
    let coverage_roots: Vec<_> = coverage
        .record_snapshot
        .iter()
        .chain(coverage.state_snapshot.iter())
        .chain(coverage.resource_snapshot.iter())
        .collect();
    let materialized_roots: Vec<_> = materialized.files_snapshot.iter().collect();
    snapshot::collect(
        registry,
        root,
        &[
            (ANCHOR, &coverage_roots),
            (super::CHECKPOINT, &materialized_roots),
        ],
        hook,
    )?;
    // Receipts and the coverage image retain every user resource declaration;
    // media, candidate/conflict originals and unfinished publications are held.
    cleanup_covered(registry, cache, root, seq, hook)
}

#[derive(Default)]
struct Contents {
    records: BTreeMap<String, PortableRecordEnvelope>,
    states: BTreeMap<String, DurableStateChange>,
    resources: BTreeMap<String, DurableResource>,
}
impl Contents {
    fn merge(replay: &LibraryReplay, extra: Option<&LibraryBatch>) -> Result<Self, Error> {
        let mut result = Self::default();
        if let Some(image) = &replay.checkpoint {
            for record in &image.records {
                result.records.insert(
                    serde_json::to_string(&record.record().object_key()?).map_err(|e| {
                        Error::unavailable_with_source(
                            "coverage key",
                            "cannot encode typed object",
                            e,
                        )
                    })?,
                    record.clone(),
                );
            }
            for state in &image.state {
                result.states.insert(state.storage_key()?, state.clone());
            }
            for resource in &image.resources {
                result
                    .resources
                    .insert(resource.relative_path.clone(), resource.clone());
            }
        }
        for batch in replay
            .commits
            .iter()
            .map(|commit| &commit.batch)
            .chain(extra)
        {
            for record in &batch.records {
                result.records.insert(
                    serde_json::to_string(&record.record().object_key()?).map_err(|e| {
                        Error::unavailable_with_source(
                            "coverage key",
                            "cannot encode typed object",
                            e,
                        )
                    })?,
                    record.clone(),
                );
            }
            for state in &batch.state {
                result.states.insert(state.storage_key()?, state.clone());
            }
            for resource in &batch.resources {
                result
                    .resources
                    .insert(resource.relative_path.clone(), resource.clone());
            }
        }
        Ok(result)
    }
}

pub(super) fn preflight_capacity(
    replay: &LibraryReplay,
    materialized_files: &BTreeMap<String, String>,
    batch: &LibraryBatch,
) -> Result<(), Error> {
    let contents = Contents::merge(replay, Some(batch))?;
    portable::encode(
        &(&contents.records, &contents.states, &contents.resources),
        LIMIT,
    )
    .map_err(|_| invalid("coverage aggregate safety limit exceeded"))?;
    snapshot::preflight(&contents.states)?;
    snapshot::preflight(&contents.records)?;
    snapshot::preflight(&contents.resources)?;
    let mut files = materialized_files.clone();
    for record in &batch.records {
        files.insert(portable::record_relative(record.record())?, "0".repeat(64));
    }
    for state in &batch.state {
        files.insert(state_path(state), "0".repeat(64));
    }
    snapshot::preflight(&files)
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
    let mut proof_directories = io::DirectorySyncSet::default();
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
            let path = io::flush_file(registry, root, ANCHOR, "coverage_gc", hook)?;
            proof_directories.include_ancestors(
                registry,
                root,
                path.parent()
                    .ok_or_else(|| invalid("missing coverage parent"))?,
            )?;
            anchor_flushed = true;
        }
        let path = io::flush_file(registry, root, &receipt(id), "receipt_gc", hook)?;
        proof_directories.include_ancestors(
            registry,
            root,
            path.parent()
                .ok_or_else(|| invalid("missing receipt parent"))?,
        )?;
        covered.push(id);
    }
    // Finish all proof syncs before the first destructive step. A subsequent
    // receipt failure therefore leaves the entire redo suffix untouched.
    proof_directories.sync(registry, root, "gc_proof", hook)?;
    let had_covered = !covered.is_empty();
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
    }
    if had_covered {
        io::sync_ancestors(registry, root, &directory, "gc", hook)?;
    }
    Ok(())
}
