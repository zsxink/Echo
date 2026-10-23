//! Shared ingestion boundary: the library owns identity before the index does.

use super::{ParsedOutcome, ScanDeps};
use crate::application::library_commit::commit_then_project;
use crate::application::portable::EnsureControlPlane;
use crate::domain::entities::Song;
use crate::domain::ids::{LibraryRootId, OperationId, RelativeMediaPath, Revision};
use crate::domain::library::durability::{
    ChangeOrigin, CommitStatus, DurableResource, DurableStateChange, LibraryBatch, StateKind,
};
use crate::domain::library::{
    next_hlc, HybridLogicalClock, PortableRecord, PortableRecordEnvelope, RecordKind,
};
use crate::domain::media::AudioFormat;
use crate::error::Error;
use std::time::UNIX_EPOCH;

pub fn library_id(
    deps: &ScanDeps,
    root: LibraryRootId,
) -> Result<crate::domain::library::LibraryId, Error> {
    EnsureControlPlane::new(deps.control.as_ref())
        .run(root)?
        .library_id()
        .ok_or_else(|| Error::unavailable("library", "authoritative library is not writable"))
}

/// Restore identities saved by a previous invocation whose SQLite projection
/// failed. A fresh scan must never allocate a replacement identity for them.
pub fn song_snapshot(deps: &ScanDeps, root: LibraryRootId) -> Result<Vec<Song>, Error> {
    let commits = deps
        .control
        .durability()
        .ok_or_else(|| {
            Error::unavailable(
                "library durability",
                "directory commit adapter is not configured",
            )
        })?
        .recover(root)?;
    let mut songs = deps.songs.all_in_root(root)?;
    for raw in deps.control.list_records(root, RecordKind::Song)? {
        let record = serde_json::from_str::<crate::domain::library::PortableRecordDocument>(&raw)
            .map_err(|error| {
            Error::unavailable_with_source("library song", "invalid authoritative song", error)
        })?;
        let PortableRecord::Song(record) = record.into_record() else {
            return Err(Error::conflict("unexpected authoritative song type"));
        };
        let id = crate::domain::ids::SongId::from_uuid(record.song_uuid);
        if songs.iter().any(|song| song.id() == id) {
            continue;
        }
        let path = RelativeMediaPath::new(record.media_path.as_str())?;
        if songs.iter().any(|song| song.path() == &path) {
            return Err(Error::conflict(
                "library and index song identities need reconciliation",
            ));
        }
        let mut song = Song::with_added_at(id, root, path, record.revision, record.added_at);
        song.apply_metadata(record.title, record.artist, record.album, None);
        song.apply_scan_facts(record.content_hash, 0, 0, AudioFormat::Flac);
        song.mark_missing();
        songs.push(song);
    }
    let mut claims = std::collections::BTreeMap::new();
    for state in commits.states() {
        if state.kind == StateKind::Claim && state.payload["kind"] == "ingestion" {
            claims.insert(state.id, state.payload.clone());
        }
    }
    for claim in claims.into_values().filter(|claim| claim["active"] == true) {
        let Some(target) = claim["target"].as_str() else {
            continue;
        };
        let path = RelativeMediaPath::new(target)?;
        let Some(hash) = claim["content_hash"].as_str() else {
            continue;
        };
        // A claim alone never makes a partial or replaced file into a song.
        if deps.hasher.hash(root, &path).ok().as_deref() != Some(hash) {
            continue;
        }
        let id = claim["song_uuid"]
            .as_str()
            .and_then(|raw| raw.parse::<crate::domain::ids::SongId>().ok())
            .ok_or_else(|| Error::conflict("invalid durable import identity"))?;
        if songs
            .iter()
            .any(|song| song.id() == id || song.path() == &path)
        {
            continue;
        }
        let mut song = Song::with_added_at(
            id,
            root,
            path,
            Revision::INITIAL,
            claim["added_at"]
                .as_u64()
                .ok_or_else(|| Error::conflict("durable import has no addition time"))?,
        );
        song.apply_scan_facts(
            hash.to_owned(),
            claim["byte_length"].as_u64().unwrap_or(0),
            0,
            AudioFormat::Flac,
        );
        song.mark_missing();
        songs.push(song);
    }
    Ok(songs)
}

/// Re-parsing unchanged published bytes only refreshes local derived facts.
/// The planner has already folded embedded tags into its snapshot, so use the
/// original row to preserve the host's shared values and current local path.
pub(super) fn published_index_refresh(
    deps: &ScanDeps,
    song: &Song,
    parsed: &ParsedOutcome,
) -> Result<Option<Song>, Error> {
    if deps
        .songs
        .published_logical_path(song.root(), song.path())?
        .is_none()
    {
        return Ok(None);
    }
    let Some(mut original) = deps.songs.by_id(song.id())? else {
        return Ok(None);
    };
    if original.root() != song.root()
        || original.path() != song.path()
        || original.blake3_hash() != Some(parsed.file.hash.as_str())
        || original.file_size() != Some(parsed.file.size)
    {
        return Ok(None);
    }
    crate::application::scan_resources::verify_parsed_audio(deps, song.root(), parsed)?;
    original.apply_scan_facts_with_params(
        parsed.file.hash.clone(),
        parsed.file.size,
        parsed.file.mtime_ns,
        parsed.file.meta.format,
        parsed.file.meta.parameters,
    );
    Ok(Some(original))
}

/// Persist a complete song after-image and verified resource declarations,
/// then run its SQLite projection. A failed projection returns a typed saved
/// result whose operation remains recoverable from the directory journal.
pub fn commit_song(
    deps: &ScanDeps,
    song: &Song,
    operation: OperationId,
    origin: ChangeOrigin,
    resources: Vec<DurableResource>,
    project: impl FnOnce() -> Result<(), Error>,
) -> Result<(), Error> {
    let root = song.root();
    let library = library_id(deps, root)?;
    let previous = deps
        .control
        .read_record(root, RecordKind::Song, &song.id().to_string())?;
    let previous = match previous {
        Some(PortableRecord::Song(record)) => Some(record),
        Some(_) => return Err(Error::conflict("unexpected authoritative song type")),
        None => None,
    };
    let now = deps
        .clock
        .now_wall()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let hlc = next_hlc(
        previous
            .as_ref()
            .map_or_else(HybridLogicalClock::default, |r| r.hlc),
        now.as_secs(),
    );
    let prior_revision = previous
        .as_ref()
        .map_or(0, |record| record.revision.as_u64());
    let indexed_revision = deps
        .sync
        .outbox_revision("song", &song.id().to_string())?
        .as_u64();
    let revision = Revision::from_u64(
        prior_revision
            .max(indexed_revision)
            .checked_add(1)
            .ok_or_else(|| Error::conflict("song revision exhausted"))?,
    );
    // A completed publication associates the physical version with its shared
    // media path. Only the portable after-image uses that logical path; resource
    // verification and the SQLite projection keep the actual local location.
    let mut portable_song = song.clone();
    if let Some(logical) = deps.songs.published_logical_path(root, song.path())? {
        portable_song.relink(logical);
    }
    let record = crate::application::portable_materialize::song_record(
        deps.device_id.current_device_id(),
        hlc,
        revision,
        &portable_song,
    )?;
    // Repeated watcher notifications and timestamp-only rescans do not create
    // another intent. File availability and parse caches remain local facts.
    if previous.as_ref().is_some_and(|old| {
        old.song_uuid == record.song_uuid
            && old.media_path == record.media_path
            && old.content_hash == record.content_hash
            && old.added_at == record.added_at
            && old.title == record.title
            && old.artist == record.artist
            && old.album == record.album
    }) {
        return project();
    }
    let envelope = PortableRecordEnvelope::new(PortableRecord::Song(record), None)?;
    let mut batch = LibraryBatch::new(operation.as_uuid(), library, origin, vec![envelope]);
    batch.resources = resources;
    // This is a local intent, not an already-frozen network mutation. The
    // separate sync capture boundary owns predecessor/receipt semantics.
    batch.state.push(DurableStateChange::new(
        StateKind::Claim,
        operation.as_uuid(),
        serde_json::json!({
            "version": 1, "kind": "ingestion", "active": false,
            "song_uuid": song.id().as_uuid(), "target": song.path().display(),
            "content_hash": song.blake3_hash(), "added_at": song.added_at(),
        }),
    ));
    let (outcome, projected) =
        commit_then_project(deps.control.as_ref(), root, &batch, |_| project())?;
    if projected.is_some() {
        return Ok(());
    }
    let cause = Error::unavailable(
        "library projection",
        outcome
            .reason
            .unwrap_or_else(|| "retry required".to_owned()),
    );
    if outcome.status == CommitStatus::SavedPendingRecovery {
        Err(Error::saved_pending_recovery(operation, cause))
    } else {
        Err(Error::saved_pending_projection(operation, cause))
    }
}
