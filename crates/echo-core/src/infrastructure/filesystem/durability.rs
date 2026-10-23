//! Library-local redo protocol. Only writers materialize; read-only replay never
//! creates locks or mutates files. Historical operation evidence is ID-addressed.
mod assets;
mod checkpoint;
use assets::validate_resource;
pub(super) use assets::{read_asset, write_asset};
mod io;
#[cfg(test)]
mod tests;

use super::portable_migration as portable;
use super::registry::RootRegistry;
use crate::domain::ids::LibraryRootId;
use crate::domain::library::durability::{
    CommitOutcome, CommitStatus, LibraryBatch, LibraryCommit, LibraryReplay,
};
use crate::domain::library::LibraryManifest;
use crate::error::Error;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

type Hook<'a> = dyn FnMut(&str, usize) -> Result<(), Error> + 'a;
const LIMIT: usize = 128 * 1024 * 1024;
const WINDOW: usize = 32;
const MAX_ACTIVE: usize = 256;
const CHECKPOINT: &str = "echo/state/materialized-checkpoint.json";
const ANCHOR: &str = "echo/state/coverage-checkpoint.json";
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Marker {
    version: u32,
    digest: String,
    seq: u64,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Checkpoint {
    version: u32,
    seq: u64,
    head: Option<String>,
    files: BTreeMap<String, String>,
}

fn invalid(reason: &str) -> Error {
    Error::conflict(format!("library durability: {reason}"))
}
fn decode<T: serde::de::DeserializeOwned>(raw: &str) -> Result<T, Error> {
    serde_json::from_str(raw).map_err(|e| {
        Error::unavailable_with_source("library durability", "invalid persisted transaction", e)
    })
}
fn relative(id: uuid::Uuid, file: &str) -> String {
    format!("echo/transactions/{id}/{file}")
}
fn receipt(id: uuid::Uuid) -> String {
    let id = id.to_string();
    format!("echo/state/operations/{}/{id}.json", &id[..2])
}
fn outcome(c: &LibraryCommit, status: CommitStatus) -> CommitOutcome {
    CommitOutcome {
        operation_id: c.batch.operation_id,
        library_commit_seq: c.library_commit_seq,
        status,
        reason: None,
    }
}
fn manifest(registry: &RootRegistry, root: LibraryRootId) -> Result<LibraryManifest, Error> {
    decode(
        &portable::read(
            registry,
            root,
            "echo/manifest.json",
            portable::MAX_RECORD_BYTES,
        )?
        .ok_or_else(|| invalid("initialize the library manifest before committing"))?,
    )
}

pub(super) fn read_operation(
    registry: &RootRegistry,
    root: LibraryRootId,
    id: uuid::Uuid,
) -> Result<Option<LibraryCommit>, Error> {
    let raw_marker = portable::read(registry, root, &relative(id, "committed.json"), 4096)?;
    let raw = if let Some(raw) = portable::read(registry, root, &receipt(id), LIMIT)? {
        Some(raw)
    } else if raw_marker.is_some() {
        portable::read(registry, root, &relative(id, "prepared.json"), LIMIT)?
    } else {
        None
    };
    let Some(raw) = raw else {
        return Ok(None);
    };
    let commit: LibraryCommit = decode(&raw)?;
    commit.validate()?;
    if commit.batch.operation_id != id
        || commit.batch.library_id != manifest(registry, root)?.library_id
    {
        return Err(invalid("operation receipt identity mismatch"));
    }
    if let Some(raw) = raw_marker {
        let marker: Marker = decode(&raw)?;
        if marker.version != 1
            || marker.seq != commit.library_commit_seq
            || marker.digest != commit.digest
        {
            return Err(invalid("operation marker disagrees with receipt"));
        }
    } else if !checkpoint::load(registry, root)?
        .is_some_and(|c| c.library_commit_seq >= commit.library_commit_seq)
    {
        return Err(invalid(
            "operation receipt is not covered by a durable anchor",
        ));
    }
    Ok(Some(commit))
}

/// Verify the fixed active suffix against the independently durable anchor.
/// Covered leftovers are checked against their immutable receipt after GC crashes.
pub(super) fn chain(registry: &RootRegistry, root: LibraryRootId) -> Result<LibraryReplay, Error> {
    let checkpoint = checkpoint::load(registry, root)?;
    let anchor_seq = checkpoint.as_ref().map_or(0, |c| c.library_commit_seq);
    let mut previous = checkpoint.as_ref().map(|c| c.head_digest.clone());
    let folder = portable::path(registry, root, "echo/transactions")?;
    let entries = match std::fs::read_dir(&folder) {
        Ok(entries) => Some(entries),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(Error::io("list library transactions", e, &folder)),
    };
    let mut commits = BTreeMap::new();
    let mut count = 0;
    for entry in entries.into_iter().flatten() {
        count += 1;
        if count > MAX_ACTIVE {
            return Err(invalid("active transaction safety limit exceeded"));
        }
        let entry = entry.map_err(|e| Error::io("read library transaction", e, &folder))?;
        let name = entry
            .file_name()
            .into_string()
            .map_err(|_| invalid("non UTF-8 transaction name"))?;
        let id =
            uuid::Uuid::parse_str(&name).map_err(|_| invalid("invalid transaction directory"))?;
        portable::path(registry, root, &relative(id, "committed.json"))?;
        let Some(raw_marker) =
            portable::read(registry, root, &relative(id, "committed.json"), 4096)?
        else {
            continue;
        };
        let marker: Marker = decode(&raw_marker)?;
        if marker.version != 1 {
            return Err(invalid("unsupported transaction version"));
        }
        // GC removes marker last. Once an anchor covers it, a receipt supplies
        // the frozen original if the prepared file was already removed.
        let raw = portable::read(registry, root, &relative(id, "prepared.json"), LIMIT)?
            .or(portable::read(registry, root, &receipt(id), LIMIT)?)
            .ok_or_else(|| invalid("committed transaction has no redo payload"))?;
        let commit: LibraryCommit = decode(&raw)?;
        commit.validate()?;
        if commit.batch.operation_id != id
            || marker.digest != commit.digest
            || marker.seq != commit.library_commit_seq
        {
            return Err(invalid("commit marker disagrees with redo payload"));
        }
        if commit.library_commit_seq <= anchor_seq {
            if read_operation(registry, root, id)?.as_ref() != Some(&commit) {
                return Err(invalid("covered journal differs from receipt"));
            }
            continue;
        }
        if commits.insert(commit.library_commit_seq, commit).is_some() {
            return Err(invalid("duplicate local commit sequence"));
        }
    }
    let library = if checkpoint.is_some() || !commits.is_empty() {
        Some(manifest(registry, root)?.library_id)
    } else {
        None
    };
    for (index, commit) in commits.values().enumerate() {
        if commit.library_commit_seq != anchor_seq + index as u64 + 1
            || commit.predecessor_digest != previous
        {
            return Err(invalid("broken local transaction chain"));
        }
        if Some(commit.batch.library_id) != library {
            return Err(invalid("cross-library transaction chain"));
        }
        previous = Some(commit.digest.clone());
    }
    Ok(LibraryReplay {
        checkpoint,
        commits: commits.into_values().collect(),
    })
}

pub(super) fn recover(
    registry: &RootRegistry,
    root: LibraryRootId,
) -> Result<LibraryReplay, Error> {
    recover_with_hook(registry, root, &mut |_, _| Ok(()))
}
fn recover_with_hook(
    registry: &RootRegistry,
    root: LibraryRootId,
    hook: &mut Hook<'_>,
) -> Result<LibraryReplay, Error> {
    let replay = chain(registry, root)?;
    let checkpoint = portable::read(registry, root, CHECKPOINT, LIMIT)?
        .map(|raw| decode::<Checkpoint>(&raw))
        .transpose()?
        .unwrap_or_default();
    let anchor_seq = replay
        .checkpoint
        .as_ref()
        .map_or(0, |c| c.library_commit_seq);
    if checkpoint.seq > 0 {
        let expected = replay
            .commits
            .iter()
            .find(|c| c.library_commit_seq == checkpoint.seq)
            .map(|c| c.digest.as_str())
            .or_else(|| {
                replay
                    .checkpoint
                    .as_ref()
                    .filter(|c| c.library_commit_seq == checkpoint.seq)
                    .map(|c| c.head_digest.as_str())
            });
        if checkpoint.version != 1 || expected != checkpoint.head.as_deref() {
            return Err(invalid("unverifiable materialization checkpoint"));
        }
    }
    let mut files = checkpoint.files;
    let newer_paths = checkpoint::changed_paths(&replay, checkpoint.seq)?;
    for (path, digest) in &files {
        if newer_paths.contains(path) {
            continue;
        }
        let raw = portable::read(registry, root, path, LIMIT)?
            .ok_or_else(|| invalid("checkpoint file missing"))?;
        if blake3::hash(raw.as_bytes()).to_hex().as_str() != digest {
            return Err(invalid("checkpoint file digest mismatch"));
        }
    }
    // The anchor is also a complete redo image if materialization lagged or was
    // interrupted before publishing its progress marker.
    if checkpoint.seq < anchor_seq {
        if let Some(image) = &replay.checkpoint {
            checkpoint::materialize(
                registry,
                root,
                &image.records,
                &image.state,
                &mut files,
                hook,
            )?;
            write_checkpoint(
                registry,
                root,
                anchor_seq,
                Some(image.head_digest.clone()),
                &files,
                hook,
            )?;
        }
    }
    for commit in replay
        .commits
        .iter()
        .filter(|c| c.library_commit_seq > checkpoint.seq.max(anchor_seq))
    {
        checkpoint::materialize(
            registry,
            root,
            &commit.batch.records,
            &commit.batch.state,
            &mut files,
            hook,
        )?;
        write_checkpoint(
            registry,
            root,
            commit.library_commit_seq,
            Some(commit.digest.clone()),
            &files,
            hook,
        )?;
    }
    if anchor_seq > 0 {
        checkpoint::cleanup_covered(registry, root, anchor_seq, hook)?;
    }
    Ok(replay)
}
fn write_checkpoint(
    registry: &RootRegistry,
    root: LibraryRootId,
    seq: u64,
    head: Option<String>,
    files: &BTreeMap<String, String>,
    hook: &mut Hook<'_>,
) -> Result<(), Error> {
    let checkpoint = Checkpoint {
        version: 1,
        seq,
        head,
        files: files.clone(),
    };
    hook("checkpoint", 0)?;
    io::publish(
        registry,
        root,
        CHECKPOINT,
        &portable::encode(&checkpoint, LIMIT)?,
        false,
        "checkpoint",
        hook,
    )
}

pub(super) fn commit(
    registry: &RootRegistry,
    root: LibraryRootId,
    batch: &LibraryBatch,
) -> Result<CommitOutcome, Error> {
    commit_with_hook(registry, root, batch, &mut |_, _| Ok(()))
}
fn commit_with_hook(
    registry: &RootRegistry,
    root: LibraryRootId,
    batch: &LibraryBatch,
    hook: &mut Hook<'_>,
) -> Result<CommitOutcome, Error> {
    batch.validate()?;
    let replay = recover_with_hook(registry, root, hook)?;
    let current_manifest = manifest(registry, root)?;
    if !current_manifest.is_compatible() || current_manifest.library_id != batch.library_id {
        return Err(invalid("unsupported format or incorrect library identity"));
    }
    if let Some(existing) = read_operation(registry, root, batch.operation_id)? {
        if existing.batch != *batch {
            return Err(invalid(
                "operation identity reused with a different payload",
            ));
        }
        return Ok(outcome(&existing, CommitStatus::SavedPendingProjection));
    }
    if replay.commits.len() >= MAX_ACTIVE {
        return Err(invalid("active journal requires compaction"));
    }
    for expected in &batch.expected {
        let path = portable::original_relative(expected.kind, expected.object_uuid, None);
        let actual = portable::read(registry, root, &path, portable::MAX_RECORD_BYTES)?;
        if actual.map(|raw| blake3::hash(raw.as_bytes()).to_hex().to_string())
            != expected.content_hash
        {
            return Err(invalid("record precondition changed during preparation"));
        }
    }
    for resource in &batch.resources {
        validate_resource(registry, root, resource, hook)?;
    }
    let (seq, predecessor_digest) = replay.head().map_or((1, None), |(seq, digest)| {
        (seq + 1, Some(digest.to_owned()))
    });
    let digest = LibraryCommit::calculate_digest(batch, seq, predecessor_digest.as_deref())?;
    let commit = LibraryCommit {
        batch: batch.clone(),
        library_commit_seq: seq,
        predecessor_digest,
        digest,
    };
    let prepared = relative(batch.operation_id, "prepared.json");
    // An interrupted preparation freezes business payload, but does not reserve
    // a sequence: another completed operation may have advanced the head.
    if let Some(raw) = portable::read(registry, root, &prepared, LIMIT)? {
        let old: LibraryCommit = decode(&raw)?;
        old.validate()?;
        if old.batch != *batch {
            return Err(invalid("prepared operation has another payload"));
        }
        if old != commit {
            io::publish(
                registry,
                root,
                &prepared,
                &portable::encode(&commit, LIMIT)?,
                false,
                "prepared",
                hook,
            )?;
        }
    } else {
        io::publish(
            registry,
            root,
            &prepared,
            &portable::encode(&commit, LIMIT)?,
            true,
            "prepared",
            hook,
        )?;
    }
    hook("prepared", 0)?;
    let marker = Marker {
        version: 1,
        digest: commit.digest.clone(),
        seq,
    };
    let publish = io::publish(
        registry,
        root,
        &relative(batch.operation_id, "committed.json"),
        &portable::encode(&marker, 4096)?,
        true,
        "marker",
        hook,
    );
    if let Err(error) = publish {
        if !matches!(
            portable::read(
                registry,
                root,
                &relative(batch.operation_id, "committed.json"),
                4096,
            ),
            Ok(None)
        ) {
            return Err(saved_error(batch.operation_id, error));
        }
        return Err(error);
    }
    if let Err(error) = (|| {
        hook("committed", 0)?;
        // Finish materialization before publishing a complete coverage image.
        let replay = recover_with_hook(registry, root, hook)?;
        if replay.commits.len() >= WINDOW {
            checkpoint::compact(registry, root, &replay, hook)?;
        }
        Ok(())
    })() {
        return Err(saved_error(batch.operation_id, error));
    }
    Ok(outcome(&commit, CommitStatus::SavedPendingProjection))
}
fn saved_error(id: uuid::Uuid, error: Error) -> Error {
    Error::saved_pending_recovery(crate::domain::ids::OperationId::from_uuid(id), error)
}

pub(super) fn operation_status(
    registry: &RootRegistry,
    root: LibraryRootId,
    id: uuid::Uuid,
) -> Result<Option<CommitOutcome>, Error> {
    let replay = chain(registry, root)?;
    let Some(commit) = read_operation(registry, root, id)? else {
        return portable::read(registry, root, &relative(id, "prepared.json"), LIMIT)?
            .map(|raw| {
                let c: LibraryCommit = decode(&raw)?;
                c.validate()?;
                Ok(outcome(&c, CommitStatus::NotSaved))
            })
            .transpose();
    };
    let checkpoint: Option<Checkpoint> = portable::read(registry, root, CHECKPOINT, LIMIT)?
        .map(|raw| decode(&raw))
        .transpose()?;
    let covered = replay
        .checkpoint
        .as_ref()
        .is_some_and(|c| c.library_commit_seq >= commit.library_commit_seq)
        || checkpoint.is_some_and(|c| c.seq >= commit.library_commit_seq);
    Ok(Some(outcome(
        &commit,
        if covered {
            CommitStatus::SavedPendingProjection
        } else {
            CommitStatus::SavedPendingRecovery
        },
    )))
}
pub(super) fn sync_directory(path: &std::path::Path) -> Result<(), Error> {
    #[cfg(not(windows))]
    let file = std::fs::File::open(path);
    #[cfg(windows)]
    let file = {
        use std::os::windows::fs::OpenOptionsExt as _;
        // FILE_FLAG_BACKUP_SEMANTICS permits opening a directory for flush.
        std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(0x0200_0000)
            .open(path)
    };
    file.and_then(|file| file.sync_all())
        .map_err(|e| Error::io("flush library directory", e, path))
}
