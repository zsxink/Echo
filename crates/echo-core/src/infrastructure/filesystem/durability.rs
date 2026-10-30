//! Library-local redo protocol. Only writers materialize; read-only replay never
//! creates locks or mutates files. Historical operation evidence is ID-addressed.
mod assets;
mod checkpoint;
mod snapshot;
mod validation_cache;
mod verified_files;
use validation_cache::DocumentKind;
pub(super) use validation_cache::ValidationCache;
mod transaction;
pub(super) use assets::{read_asset, write_asset};
pub(super) use transaction::{commit_edit, commit_with_hook};
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
use std::collections::{BTreeMap, BTreeSet};

type Hook<'a> = dyn FnMut(&str, usize) -> Result<(), Error> + 'a;
const LIMIT: usize = 128 * 1024 * 1024;
const WINDOW: usize = 32;
const MAX_ACTIVE: usize = 512;
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    files_snapshot: Option<snapshot::Snapshot>,
}

fn materialization_checkpoint(
    registry: &RootRegistry,
    root: LibraryRootId,
) -> Result<Checkpoint, Error> {
    let Some(raw) = portable::read(registry, root, CHECKPOINT, LIMIT)? else {
        return Ok(Checkpoint::default());
    };
    let mut checkpoint: Checkpoint = decode(&raw)?;
    match (checkpoint.version, &checkpoint.files_snapshot) {
        (1, None) => {}
        (2, Some(snapshot)) if checkpoint.files.is_empty() => {
            checkpoint.files = snapshot::load(registry, root, snapshot)?;
        }
        _ => return Err(invalid("unsupported materialization checkpoint")),
    }
    Ok(checkpoint)
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
const fn outcome(c: &LibraryCommit, status: CommitStatus) -> CommitOutcome {
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

/// Fence legacy writers before an explicitly requested ordered edit. Record
/// envelopes retain their own versions, so upgrading this declaration never
/// rewrites legacy original bytes or their synchronization evidence.
pub(in crate::infrastructure::filesystem) fn enable_journal(
    registry: &RootRegistry,
    root: LibraryRootId,
    hook: &mut Hook<'_>,
) -> Result<(), Error> {
    let mut current = manifest(registry, root)?;
    if !current.is_compatible() {
        return Err(invalid("unsupported library manifest format"));
    }
    if current.format_version == crate::domain::library::CURRENT_FORMAT_VERSION {
        // After an interrupted manifest directory sync, a retry can see v3
        // without an additional barrier here. A new commit's prepared-file
        // publication syncs echo and root before its marker becomes durable.
        // If planning produces no new fact, a crash reverting only this version
        // declaration leaves the older library format safe to open again.
        return Ok(());
    }
    current.format_version = crate::domain::library::CURRENT_FORMAT_VERSION;
    io::publish(
        registry,
        root,
        "echo/manifest.json",
        &portable::encode(&current, portable::MAX_RECORD_BYTES)?,
        false,
        "journal_manifest",
        hook,
    )
}

pub(super) fn read_operation(
    registry: &RootRegistry,
    cache: &ValidationCache,
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
    cache.validate(DocumentKind::Commit, root, &raw, || commit.validate())?;
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
    } else if checkpoint::load(registry, cache, root)?
        .is_none_or(|c| c.library_commit_seq < commit.library_commit_seq)
    {
        return Err(invalid(
            "operation receipt is not covered by a durable anchor",
        ));
    }
    Ok(Some(commit))
}

/// Verify the fixed active suffix against the independently durable anchor.
/// Covered leftovers are checked against their immutable receipt after GC crashes.
pub(super) fn chain(
    registry: &RootRegistry,
    cache: &ValidationCache,
    root: LibraryRootId,
) -> Result<LibraryReplay, Error> {
    let checkpoint = checkpoint::load(registry, cache, root)?;
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
        cache.validate(DocumentKind::Commit, root, &raw, || commit.validate())?;
        if commit.batch.operation_id != id
            || marker.digest != commit.digest
            || marker.seq != commit.library_commit_seq
        {
            return Err(invalid("commit marker disagrees with redo payload"));
        }
        if commit.library_commit_seq <= anchor_seq {
            if read_operation(registry, cache, root, id)?.as_ref() != Some(&commit) {
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
    let replay = LibraryReplay {
        checkpoint,
        commits: commits.into_values().collect(),
    };
    replay.validate_state_history()?;
    Ok(replay)
}

pub(super) fn recover(
    registry: &RootRegistry,
    cache: &ValidationCache,
    root: LibraryRootId,
) -> Result<LibraryReplay, Error> {
    recover_with_hook(registry, cache, root, &mut |_, _| Ok(()))
}

fn recover_with_hook(
    registry: &RootRegistry,
    cache: &ValidationCache,
    root: LibraryRootId,
    hook: &mut Hook<'_>,
) -> Result<LibraryReplay, Error> {
    begin_edit(registry, cache, root, None, None, true, true, hook).map(|state| state.replay)
}

pub(super) fn catch_up_projection(
    registry: &RootRegistry,
    cache: &ValidationCache,
    root: LibraryRootId,
    hook: &mut Hook<'_>,
) -> Result<LibraryReplay, Error> {
    // The journal is authenticated and replayed in order. This path only
    // advances rebuildable files; strict recover still audits every existing
    // materialized byte before it can authorize startup or standalone writes.
    begin_edit(registry, cache, root, None, None, false, true, hook).map(|state| state.replay)
}

pub(super) fn projection_replay(
    registry: &RootRegistry,
    cache: &ValidationCache,
    root: LibraryRootId,
) -> Result<LibraryReplay, Error> {
    // Transaction markers publish only after the prepared body is durable, so
    // an authenticated chain read is safe without the directory writer lock.
    chain(registry, cache, root)
}

pub(super) fn materialization_files(
    registry: &RootRegistry,
    root: LibraryRootId,
) -> Result<BTreeMap<String, String>, Error> {
    Ok(materialization_checkpoint(registry, root)?.files)
}

// Retained only inside one control-plane writer guard. A commit may extend this
// verified state without re-reading all history and every materialized file.
// Every later writer call starts with a fresh recovery and fresh byte checks.
pub(in crate::infrastructure::filesystem) struct WriteSessionState {
    root: LibraryRootId,
    replay: LibraryReplay,
    files: BTreeMap<String, String>,
    durable_snapshot: Option<snapshot::Snapshot>,
    materialize_commits: bool,
    consumed: bool,
}

impl WriteSessionState {
    pub(in crate::infrastructure::filesystem) const fn replay(&self) -> &LibraryReplay {
        &self.replay
    }

    pub(in crate::infrastructure::filesystem) const fn files(&self) -> &BTreeMap<String, String> {
        &self.files
    }
}

/// The caller must retain the same root writer guard through `commit_edit`.
/// Releasing that guard invalidates the returned materialization proof.
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
pub(in crate::infrastructure::filesystem) fn begin_edit(
    registry: &RootRegistry,
    cache: &ValidationCache,
    root: LibraryRootId,
    cached_replay: Option<LibraryReplay>,
    cached_files: Option<BTreeMap<String, String>>,
    verify_materialized_files: bool,
    materialize_commits: bool,
    hook: &mut Hook<'_>,
) -> Result<WriteSessionState, Error> {
    let mut replay = match cached_replay {
        Some(replay) => replay,
        None => chain(registry, cache, root)?,
    };
    let force_materialize = replay.commits.len() >= MAX_ACTIVE - WINDOW;
    let materialize_commits = materialize_commits || force_materialize;
    let mut checkpoint = if cached_files.is_some() && !materialize_commits {
        Checkpoint::default()
    } else {
        materialization_checkpoint(registry, root)?
    };
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
        if expected != checkpoint.head.as_deref() {
            return Err(invalid("unverifiable materialization checkpoint"));
        }
    }
    // A fast-path file map also tracks journal after-images that have not yet
    // been written to the materialized tree. Never use that logical overlay as
    // a durability checkpoint when this pass is about to publish files.
    let mut files = if materialize_commits {
        checkpoint.files
    } else {
        cached_files.unwrap_or(checkpoint.files)
    };
    let mut durable_snapshot = if checkpoint.seq > 0 {
        checkpoint.files_snapshot
    } else {
        None
    };
    let newer_paths = if verify_materialized_files {
        checkpoint::changed_paths(&replay, checkpoint.seq)?
    } else {
        BTreeSet::new()
    };
    // Interactive writes trust the authenticated journal and the batch's
    // per-object preconditions. The materialized record tree is a rebuildable
    // projection; hashing every checkpointed file here made one favorite
    // operation scale with the full library size. Full verification remains
    // mandatory for recovery and standalone commits, and is run on startup.
    if verify_materialized_files {
        verified_files::verify(registry, root, &files, &newer_paths)?;
    }
    // The anchor is also a complete redo image if materialization lagged or was
    // interrupted before publishing its progress marker.
    if materialize_commits && checkpoint.seq < anchor_seq {
        if let Some(image) = &replay.checkpoint {
            io::flush_existing(registry, root, ANCHOR, "coverage_recovery", hook)?;
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
                &mut durable_snapshot,
                hook,
            )?;
        }
    }
    for commit in replay
        .commits
        .iter()
        .filter(|c| materialize_commits && c.library_commit_seq > checkpoint.seq.max(anchor_seq))
    {
        // A materialization checkpoint can only have been published after its
        // commit fence completed. Beyond that watermark, visibility alone is
        // insufficient: a prior marker rename may have failed directory sync.
        for (name, label) in [
            ("prepared.json", "prepared_recovery"),
            ("committed.json", "marker_recovery"),
        ] {
            io::flush_existing(
                registry,
                root,
                &relative(commit.batch.operation_id, name),
                label,
                hook,
            )?;
        }
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
            &mut durable_snapshot,
            hook,
        )?;
    }
    if materialize_commits && anchor_seq > 0 {
        checkpoint::cleanup_covered(registry, cache, root, anchor_seq, hook)?;
    }
    if force_materialize && replay.commits.len() >= MAX_ACTIVE - WINDOW {
        checkpoint::compact(registry, cache, root, &replay, hook)?;
        replay = chain(registry, cache, root)?;
        checkpoint = materialization_checkpoint(registry, root)?;
        files.clone_from(&checkpoint.files);
        durable_snapshot.clone_from(&checkpoint.files_snapshot);
    }
    Ok(WriteSessionState {
        root,
        replay,
        files,
        durable_snapshot,
        materialize_commits,
        consumed: false,
    })
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct TransactionsFingerprint {
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
    #[cfg(unix)]
    mtime: (i64, i64),
    #[cfg(unix)]
    ctime: (i64, i64),
    #[cfg(not(unix))]
    modified: Option<std::time::SystemTime>,
    #[cfg(not(unix))]
    len: u64,
}

pub(super) fn transactions_fingerprint(
    registry: &RootRegistry,
    root: LibraryRootId,
) -> Result<Option<TransactionsFingerprint>, Error> {
    let directory = registry.path_of(root)?.join("echo/transactions");
    let metadata = match std::fs::symlink_metadata(&directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(Error::io("inspect journal directory", error, directory)),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(invalid("journal transactions path is not a directory"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(Some(TransactionsFingerprint {
            dev: metadata.dev(),
            ino: metadata.ino(),
            mtime: (metadata.mtime(), metadata.mtime_nsec()),
            ctime: (metadata.ctime(), metadata.ctime_nsec()),
        }))
    }
    #[cfg(not(unix))]
    {
        Ok(Some(TransactionsFingerprint {
            modified: metadata.modified().ok(),
            len: metadata.len(),
        }))
    }
}
fn write_checkpoint(
    registry: &RootRegistry,
    root: LibraryRootId,
    seq: u64,
    head: Option<String>,
    files: &BTreeMap<String, String>,
    durable_snapshot: &mut Option<snapshot::Snapshot>,
    hook: &mut Hook<'_>,
) -> Result<(), Error> {
    let files_snapshot =
        snapshot::publish_reusing(registry, root, files, durable_snapshot.as_ref(), hook)?;
    let checkpoint = Checkpoint {
        version: 2,
        seq,
        head,
        files: BTreeMap::new(),
        files_snapshot: Some(files_snapshot.clone()),
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
    )?;
    *durable_snapshot = Some(files_snapshot);
    Ok(())
}

pub(super) fn commit(
    registry: &RootRegistry,
    cache: &ValidationCache,
    root: LibraryRootId,
    batch: &LibraryBatch,
) -> Result<CommitOutcome, Error> {
    commit_with_hook(registry, cache, root, batch, &mut |_, _| Ok(()))
}
pub(super) fn operation_status(
    registry: &RootRegistry,
    cache: &ValidationCache,
    root: LibraryRootId,
    id: uuid::Uuid,
) -> Result<Option<CommitOutcome>, Error> {
    let replay = chain(registry, cache, root)?;
    let Some(commit) = read_operation(registry, cache, root, id)? else {
        return portable::read(registry, root, &relative(id, "prepared.json"), LIMIT)?
            .map(|raw| {
                let c: LibraryCommit = decode(&raw)?;
                cache.validate(DocumentKind::Commit, root, &raw, || c.validate())?;
                if c.batch.operation_id != id
                    || c.batch.library_id != manifest(registry, root)?.library_id
                {
                    return Err(invalid("prepared operation identity mismatch"));
                }
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
