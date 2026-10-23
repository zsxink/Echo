//! Validate the complete chain and existing watermark before any SQL mutation.
use super::super::support::storage;
use crate::domain::{ids::LibraryRootId, library::durability::{ChangeOrigin, LibraryBatch, LibraryReplay}};
use crate::error::Error;
use rusqlite::{params, Connection, OptionalExtension};
use std::collections::BTreeSet;

type Watermark = (String, u64, Option<String>);
pub(super) fn validate(c: &Connection, root: LibraryRootId, replay: &LibraryReplay) -> Result<Option<Watermark>, Error> {
    let exists: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM library_roots WHERE uuid=?1)", [root.to_string()], |r| r.get(0)).map_err(storage)?;
    if !exists { return Err(Error::conflict("projection root is not registered")); }
    let current: Option<Watermark> = c.query_row("SELECT library_id,commit_seq,commit_digest FROM library_projection_watermarks WHERE library_root_uuid=?1", [root.to_string()], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?))).optional().map_err(storage)?;
    let mut library = replay.checkpoint.as_ref().map(|image| image.library_id);
    let mut seq = 0;
    let mut predecessor = None;
    if let Some(image) = &replay.checkpoint {
        if image.library_commit_seq == 0 || !crate::domain::library::durability::valid_digest(&image.head_digest) {
            return Err(Error::conflict("invalid coverage anchor"));
        }
        let mut batch = LibraryBatch::new(image.library_id.as_uuid(), image.library_id, ChangeOrigin::Recovery, image.records.clone());
        batch.state = image.state.clone(); batch.resources = image.resources.clone(); batch.validate()?;
        seq = image.library_commit_seq;
        predecessor = Some(image.head_digest.clone());
        if current.as_ref().is_some_and(|(_, n, digest)| *n == seq && digest != &predecessor) { return Err(Error::conflict("database watermark differs from coverage anchor")); }
    }
    let mut operations = BTreeSet::new();
    for commit in &replay.commits {
        commit.validate()?;
        if seq.checked_add(1) != Some(commit.library_commit_seq) || commit.predecessor_digest != predecessor || !operations.insert(commit.batch.operation_id) {
            return Err(Error::conflict("library projection chain is incomplete"));
        }
        if library.is_some_and(|id| id != commit.batch.library_id) { return Err(Error::conflict("library projection chain crosses libraries")); }
        library = Some(commit.batch.library_id); seq = commit.library_commit_seq; predecessor = Some(commit.digest.clone());
        if current.as_ref().is_some_and(|(_, n, _)| *n >= seq) {
            let applied: Option<(u64,String)> = c.query_row("SELECT commit_seq,commit_digest FROM library_applied_operations WHERE library_root_uuid=?1 AND operation_uuid=?2", params![root.to_string(),commit.batch.operation_id.to_string()], |r| Ok((r.get(0)?,r.get(1)?))).optional().map_err(storage)?;
            if applied.as_ref() != Some(&(seq,commit.digest.clone())) { return Err(Error::conflict("database projection differs from library chain")); }
            if current.as_ref().is_some_and(|(_, n, digest)| *n == seq && digest != &predecessor) { return Err(Error::conflict("database watermark digest differs")); }
        }
    }
    if let Some((id,n,_)) = &current {
        if *n > seq { return Err(Error::conflict("database projection is ahead of verified directory")); }
        if library.is_some_and(|library| id != &library.to_string()) { return Err(Error::conflict("database projection belongs to another library")); }
    }
    Ok(current)
}
