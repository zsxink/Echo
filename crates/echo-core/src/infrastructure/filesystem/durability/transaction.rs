//! Authoritative commit publication under the control plane's writer guard.
use super::assets::validate_resource;
use super::{
    begin_edit, checkpoint, decode, invalid, io, manifest, outcome, portable, read_operation,
    relative, write_checkpoint, DocumentKind, Hook, Marker, ValidationCache, WriteSessionState,
    LIMIT, MAX_ACTIVE, WINDOW,
};
use crate::domain::ids::LibraryRootId;
use crate::domain::library::durability::{
    CommitOutcome, CommitStatus, LibraryBatch, LibraryCommit,
};
use crate::error::Error;
use crate::infrastructure::filesystem::RootRegistry;

pub(in crate::infrastructure::filesystem) fn commit_with_hook(
    registry: &RootRegistry,
    cache: &ValidationCache,
    root: LibraryRootId,
    batch: &LibraryBatch,
    hook: &mut Hook<'_>,
) -> Result<CommitOutcome, Error> {
    validate_commit_manifest(registry, root, batch)?;
    let mut recovered = begin_edit(registry, cache, root, hook)?;
    commit_edit(registry, cache, root, &mut recovered, batch, hook)
}

pub(in crate::infrastructure::filesystem) fn commit_edit(
    registry: &RootRegistry,
    cache: &ValidationCache,
    root: LibraryRootId,
    recovered: &mut WriteSessionState,
    batch: &LibraryBatch,
    hook: &mut Hook<'_>,
) -> Result<CommitOutcome, Error> {
    if recovered.root != root || recovered.consumed {
        return Err(invalid(
            "write session has another root or is already consumed",
        ));
    }
    // A failed publication may leave visible evidence requiring cold recovery;
    // callers must acquire a new session before attempting another operation.
    recovered.consumed = true;
    validate_commit_manifest(registry, root, batch)?;
    let replay = &recovered.replay;
    if let Some(existing) = read_operation(registry, cache, root, batch.operation_id)? {
        if existing.batch != *batch {
            return Err(invalid(
                "operation identity reused with a different payload",
            ));
        }
        return Ok(outcome(&existing, CommitStatus::SavedPendingProjection));
    }
    replay.validate_state_extension(&batch.state)?;
    if replay.commits.len() >= MAX_ACTIVE {
        return Err(invalid("active journal requires compaction"));
    }
    // Refuse capacity exhaustion before publishing any commit marker. Otherwise
    // a mandatory coverage snapshot could fail only after the new fact is saved.
    checkpoint::preflight_capacity(replay, &recovered.files, batch)?;
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
    publish_prepared(registry, cache, root, &commit, hook)?;
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
        // The writer guard has remained held since recovery. This new commit's
        // prepared body and marker have both completed every native barrier, so
        // apply only its delta to the already verified materialization state.
        checkpoint::materialize(
            registry,
            root,
            &batch.records,
            &batch.state,
            &mut recovered.files,
            hook,
        )?;
        write_checkpoint(
            registry,
            root,
            seq,
            Some(commit.digest.clone()),
            &recovered.files,
            &mut recovered.durable_snapshot,
            hook,
        )?;
        recovered.replay.commits.push(commit.clone());
        if recovered.replay.commits.len() >= WINDOW {
            checkpoint::compact(registry, cache, root, &recovered.replay, hook)?;
        }
        Ok(())
    })() {
        return Err(saved_error(batch.operation_id, error));
    }
    Ok(outcome(&commit, CommitStatus::SavedPendingProjection))
}

fn validate_commit_manifest(
    registry: &RootRegistry,
    root: LibraryRootId,
    batch: &LibraryBatch,
) -> Result<(), Error> {
    batch.validate()?;
    let current_manifest = manifest(registry, root)?;
    if current_manifest.format_version != crate::domain::library::CURRENT_FORMAT_VERSION
        || current_manifest.library_id != batch.library_id
    {
        return Err(invalid("unsupported format or incorrect library identity"));
    }
    Ok(())
}

fn saved_error(id: uuid::Uuid, error: Error) -> Error {
    Error::saved_pending_recovery(crate::domain::ids::OperationId::from_uuid(id), error)
}

fn publish_prepared(
    registry: &RootRegistry,
    cache: &ValidationCache,
    root: LibraryRootId,
    commit: &LibraryCommit,
    hook: &mut Hook<'_>,
) -> Result<(), Error> {
    let prepared = relative(commit.batch.operation_id, "prepared.json");
    // An interrupted preparation freezes business payload, but does not reserve
    // a sequence: another completed operation may have advanced the head.
    if let Some(raw) = portable::read(registry, root, &prepared, LIMIT)? {
        let old: LibraryCommit = decode(&raw)?;
        cache.validate(DocumentKind::Commit, root, &raw, || old.validate())?;
        if old.batch != commit.batch {
            return Err(invalid("prepared operation has another payload"));
        }
        if old == *commit {
            io::flush_existing(registry, root, &prepared, "prepared_retry", hook)?;
        } else {
            io::publish(
                registry,
                root,
                &prepared,
                &portable::encode(commit, LIMIT)?,
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
            &portable::encode(commit, LIMIT)?,
            true,
            "prepared",
            hook,
        )?;
    }
    Ok(())
}
