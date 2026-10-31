//! Directory order is authoritative; unjournaled snapshots retain legacy rules.

use super::ProjectionReport;
use crate::application::ports::{ControlPlanePort, LibraryProjectionPort};
use crate::domain::ids::LibraryRootId;
use crate::domain::library::durability::{
    LibraryReplay, ProjectionTarget, ProjectionUnresolvedReason,
};
use crate::domain::library::{
    PortableRecord, PortableRecordDocument, PortableRecordEnvelope, RecordKind,
};
use crate::domain::sync::ObjectKey;
use crate::error::Error;
use std::collections::{HashMap, HashSet};

pub(super) fn project(
    control: &dyn ControlPlanePort,
    projection: &dyn LibraryProjectionPort,
    root: LibraryRootId,
) -> Result<ProjectionReport, Error> {
    let manifest = control
        .read_manifest(root)?
        .ok_or_else(|| Error::conflict("continuation lost its compatible manifest"))?;
    let journaled = manifest.format_version >= 3;
    let replay = if journaled {
        control
            .durability()
            .ok_or_else(|| {
                Error::unavailable(
                    "library continuation",
                    "directory recovery is not configured",
                )
            })?
            .recover(root)?
    } else {
        LibraryReplay::default()
    };
    let snapshots = uncovered_snapshots(control, root, &replay)?;
    if !snapshots.is_empty() {
        projection.project_legacy_records(root, &snapshots)?;
    }
    if journaled {
        // Never synthesize a commit for a legacy snapshot. Real commits alone
        // establish the projection watermark and permanent tombstone semantics.
        projection.project_library_commits(root, &replay)?;
    }
    report(projection, root)
}

/// Select the directory originals that are not represented by journal facts.
/// Candidate rebuilds use the same selection as ordinary continuation.
pub fn uncovered_snapshots(
    control: &dyn ControlPlanePort,
    root: LibraryRootId,
    replay: &LibraryReplay,
) -> Result<Vec<PortableRecordEnvelope>, Error> {
    let covered: HashSet<_> = replay
        .checkpoint
        .iter()
        .flat_map(|image| &image.records)
        .chain(
            replay
                .commits
                .iter()
                .flat_map(|commit| &commit.batch.records),
        )
        .map(|envelope| envelope.record().object_key())
        .collect::<Result<_, _>>()?;
    let mut snapshots = HashMap::new();
    for kind in [
        RecordKind::Song,
        RecordKind::Favorite,
        RecordKind::Playlist,
        RecordKind::PlaylistItem,
        RecordKind::PlayStats,
        RecordKind::Override,
        RecordKind::Tombstone,
    ] {
        for document in control.list_record_documents(root, kind)? {
            let envelope = match document {
                PortableRecordDocument::V1(record) => PortableRecordEnvelope::new(record, None)?,
                PortableRecordDocument::V2(envelope) => envelope,
            };
            let key = envelope.record().object_key()?;
            if !covered.contains(&key) {
                select_snapshot(&mut snapshots, key, envelope)?;
            }
        }
    }
    let mut snapshots: Vec<_> = snapshots.into_values().collect();
    snapshots.sort_by_cached_key(|envelope| format!("{:?}", envelope.record().object_key()));
    Ok(snapshots)
}

fn select_snapshot(
    selected: &mut HashMap<ObjectKey, PortableRecordEnvelope>,
    key: ObjectKey,
    envelope: PortableRecordEnvelope,
) -> Result<(), Error> {
    let version = |record: &PortableRecord| {
        (
            record.hlc(),
            record.revision().as_u64(),
            record.is_tombstone(),
        )
    };
    if let Some(previous) = selected.get(&key) {
        match version(envelope.record()).cmp(&version(previous.record())) {
            std::cmp::Ordering::Less => return Ok(()),
            std::cmp::Ordering::Equal if previous != &envelope => {
                return Err(Error::conflict(
                    "legacy snapshots disagree at the same typed version",
                ));
            }
            _ => {}
        }
    }
    selected.insert(key, envelope);
    Ok(())
}

fn report(
    projection: &dyn LibraryProjectionPort,
    root: LibraryRootId,
) -> Result<ProjectionReport, Error> {
    let status = projection.projection_status(root)?;
    let mut unresolved = HashSet::new();
    let mut report = ProjectionReport::default();
    for issue in status.unresolved {
        if let ProjectionTarget::Record { object } = issue.target {
            unresolved.insert(object);
            if issue.reason == ProjectionUnresolvedReason::IdentityConflict {
                report.superseded += 1;
            } else {
                report.invalid += 1;
            }
        }
    }
    for envelope in projection.read_recovery_records(root)?.records {
        if unresolved.contains(&envelope.record().object_key()?) {
            continue;
        }
        match envelope.record() {
            PortableRecord::Song(_) => report.songs += 1,
            PortableRecord::Favorite(_) => report.favorites += 1,
            PortableRecord::Playlist(_) => report.playlists += 1,
            PortableRecord::PlaylistItem(_) => report.members += 1,
            PortableRecord::PlayStats(_) => report.play_stats += 1,
            PortableRecord::Override(_) => report.overrides_seen += 1,
            PortableRecord::Tombstone(_) => report.tombstoned += 1,
        }
    }
    Ok(report)
}
