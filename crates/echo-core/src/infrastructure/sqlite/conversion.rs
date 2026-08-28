//! SQL row → domain mapping and enum ↔ database-string conversions.
//!
//! The `..._from_db` / `..._to_db` functions are the only place the storage
//! encoding of these values is decided; keep them in sync with the migration.

#![allow(
    clippy::doc_markdown,
    clippy::missing_errors_doc,
    clippy::needless_pass_by_value,
    clippy::redundant_pub_crate,
    clippy::uninlined_format_args
)]

use std::path::PathBuf;
use std::time::Duration;

use crate::application::ports::{OperationItem, OperationResourceKind};
use crate::domain::entities::{
    LibraryRoot, LyricsCandidate, LyricsLine, RootAvailability, Song, SongAvailability,
};
use crate::domain::ids::{PlayCount, RelativeMediaPath, Revision};
use crate::domain::media::AudioFormat;
use crate::domain::state::OperationState;
use crate::error::Error;

use super::support::{parse_id, to_sql_error};

pub(crate) fn root_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<LibraryRoot> {
    let id = parse_id(&row.get::<_, String>(0)?, "LibraryRootId").map_err(to_sql_error)?;
    let active = row.get::<_, i64>(2)? != 0;
    let write_capable = row.get::<_, i64>(3)? != 0;
    let mut root = LibraryRoot::new(
        id,
        PathBuf::from(row.get::<_, String>(1)?),
        active,
        write_capable,
    );
    root.set_availability(if row.get::<_, String>(4)? == "available" {
        RootAvailability::Available
    } else {
        RootAvailability::Unavailable
    });
    Ok(root)
}

pub(crate) fn song_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Song> {
    let id = parse_id(&row.get::<_, String>(0)?, "SongId").map_err(to_sql_error)?;
    let root = parse_id(&row.get::<_, String>(1)?, "LibraryRootId").map_err(to_sql_error)?;
    let path = RelativeMediaPath::new(&row.get::<_, String>(2)?).map_err(to_sql_error)?;
    let availability = availability_from_db(&row.get::<_, String>(3)?).map_err(to_sql_error)?;
    let duration = row
        .get::<_, Option<i64>>(11)?
        .and_then(|millis| u64::try_from(millis).ok())
        .map(Duration::from_millis);
    let format: Option<AudioFormat> = row
        .get::<_, Option<String>>(16)?
        .map(|value| audio_format_from_db(&value))
        .transpose()
        .map_err(to_sql_error)?;
    Ok(Song::from_storage(
        id,
        root,
        path,
        availability,
        row.get::<_, i64>(4)? != 0,
        PlayCount::from_u64(row.get::<_, u64>(5)?),
        Revision::from_u64(row.get::<_, u64>(6)?),
        row.get::<_, u64>(7)?,
        row.get(8)?,
        row.get(9)?,
        row.get(10)?,
        duration,
        row.get::<_, u64>(12)?,
        row.get(13)?,
        row.get(14)?,
        row.get(15)?,
        format,
    ))
}

/// The storage encoding of a scan state.
pub(crate) fn scan_state_from_db(value: &str) -> crate::domain::state::scan::ScanState {
    match value {
        "enumerating" => crate::domain::state::scan::ScanState::Enumerating,
        "parsing" => crate::domain::state::scan::ScanState::Parsing,
        "reconciling" => crate::domain::state::scan::ScanState::Reconciling,
        "completed" => crate::domain::state::scan::ScanState::Completed,
        "cancelled" => crate::domain::state::scan::ScanState::Cancelled,
        "failed" => crate::domain::state::scan::ScanState::Failed,
        _ => crate::domain::state::scan::ScanState::Queued,
    }
}

/// The storage encoding of an [`AudioFormat`] is its canonical extension.
pub(crate) const fn audio_format_to_db(value: AudioFormat) -> &'static str {
    value.extension()
}

pub(crate) fn audio_format_from_db(value: &str) -> Result<AudioFormat, Error> {
    match value {
        "mp3" => Ok(AudioFormat::Mpeg),
        "flac" => Ok(AudioFormat::Flac),
        "m4a" => Ok(AudioFormat::Mp4),
        "ogg" => Ok(AudioFormat::Ogg),
        "opus" => Ok(AudioFormat::Opus),
        "wav" => Ok(AudioFormat::Wav),
        "unknown" => Ok(AudioFormat::UnknownDamaged),
        _ => Err(Error::InvariantViolation {
            why: "unknown audio format in SQLite".to_owned(),
        }),
    }
}

/// The storage encoding of a [`LyricsCandidate`] row (`song_lyrics` table).
pub(crate) const fn text_kind_to_db(candidate: &LyricsCandidate) -> &'static str {
    if candidate.is_empty_override() {
        "empty"
    } else if candidate.is_plain_text() {
        "plain"
    } else {
        "timed"
    }
}

pub(crate) fn lyrics_source_from_db(
    value: &str,
) -> Result<crate::domain::entities::LyricsSource, Error> {
    match value {
        "override" => Ok(crate::domain::entities::LyricsSource::Override),
        "embedded" => Ok(crate::domain::entities::LyricsSource::Embedded),
        "sidecar" => Ok(crate::domain::entities::LyricsSource::Sidecar),
        _ => Err(Error::InvariantViolation {
            why: "unknown lyrics source in SQLite".to_owned(),
        }),
    }
}

/// Serialize the timed lines of a candidate into the `timed_lines_json` column.
pub(crate) fn timed_lines_to_json(candidate: &LyricsCandidate) -> String {
    let lines: Vec<serde_json::Value> = candidate
        .lines()
        .iter()
        .map(|line| {
            serde_json::json!({
                "timestamp_ms": line.timestamp_ms,
                "text": line.text,
                "original_index": line.original_index,
            })
        })
        .collect();
    serde_json::to_string(&lines).unwrap_or_else(|_| "[]".to_owned())
}

pub(crate) fn timed_lines_from_json(json: &str) -> Result<Vec<LyricsLine>, Error> {
    let parsed: Vec<serde_json::Value> =
        serde_json::from_str(json).map_err(|e| Error::InvariantViolation {
            why: format!("corrupt timed_lines_json: {e}"),
        })?;
    parsed
        .into_iter()
        .map(|value| {
            Ok(LyricsLine {
                timestamp_ms: value
                    .get("timestamp_ms")
                    .and_then(serde_json::Value::as_i64)
                    .ok_or_else(|| Error::InvariantViolation {
                        why: "timed line missing timestamp_ms".to_owned(),
                    })?,
                text: value
                    .get("text")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                original_index: value
                    .get("original_index")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or_default()
                    .try_into()
                    .unwrap_or(usize::MAX),
            })
        })
        .collect()
}

pub(crate) fn operation_item_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<OperationItem> {
    Ok(OperationItem {
        kind: match row.get::<_, String>(0)?.as_str() {
            "audio" => OperationResourceKind::Audio,
            "lyrics" => OperationResourceKind::Lyrics,
            _ => {
                return Err(to_sql_error(Error::InvariantViolation {
                    why: "unknown operation resource kind".to_owned(),
                }))
            }
        },
        state: operation_state_from_db(&row.get::<_, String>(1)?).map_err(to_sql_error)?,
        song: row
            .get::<_, Option<String>>(2)?
            .map(|value| parse_id(&value, "SongId"))
            .transpose()
            .map_err(to_sql_error)?,
        target_path: RelativeMediaPath::new(&row.get::<_, String>(3)?).map_err(to_sql_error)?,
        expected_hash: row.get(4)?,
        claim_key: row.get(5)?,
    })
}

pub(crate) fn availability_from_db(value: &str) -> Result<SongAvailability, Error> {
    match value {
        "available" => Ok(SongAvailability::Available),
        "missing" => Ok(SongAvailability::Missing),
        "pending_delete" => Ok(SongAvailability::PendingDelete),
        _ => Err(Error::InvariantViolation {
            why: "unknown song availability in SQLite".to_owned(),
        }),
    }
}
pub(crate) const fn availability_to_db(value: SongAvailability) -> &'static str {
    match value {
        SongAvailability::Available => "available",
        SongAvailability::Missing => "missing",
        SongAvailability::PendingDelete => "pending_delete",
    }
}

pub(crate) const fn operation_state_to_db(state: OperationState) -> &'static str {
    match state {
        OperationState::Planned => "planned",
        OperationState::CopyPending => "copy_pending",
        OperationState::CopyApplied => "copy_applied",
        OperationState::ValidatePending => "validate_pending",
        OperationState::Validated => "validated",
        OperationState::PublishPending => "publish_pending",
        OperationState::PublishApplied => "publish_applied",
        OperationState::DatabaseCommitted => "database_committed",
        OperationState::Completed => "completed",
        OperationState::FailedRecoverable => "failed_recoverable",
        OperationState::RolledBack => "rolled_back",
        OperationState::StagePending => "stage_pending",
        OperationState::StageApplied => "stage_applied",
        OperationState::HiddenInDatabase => "hidden_in_database",
        OperationState::RestorePending => "restore_pending",
        OperationState::RestoreApplied => "restore_applied",
        OperationState::Restored => "restored",
        OperationState::TrashPending => "trash_pending",
        OperationState::TrashApplied => "trash_applied",
        OperationState::DatabaseFinalized => "database_finalized",
        OperationState::TrashOutcomeUnknown => "trash_outcome_unknown",
    }
}
pub(crate) fn operation_state_from_db(value: &str) -> Result<OperationState, Error> {
    match value {
        "planned" => Ok(OperationState::Planned),
        "copy_pending" => Ok(OperationState::CopyPending),
        "copy_applied" => Ok(OperationState::CopyApplied),
        "validate_pending" => Ok(OperationState::ValidatePending),
        "validated" => Ok(OperationState::Validated),
        "publish_pending" => Ok(OperationState::PublishPending),
        "publish_applied" => Ok(OperationState::PublishApplied),
        "database_committed" => Ok(OperationState::DatabaseCommitted),
        "completed" => Ok(OperationState::Completed),
        "failed_recoverable" => Ok(OperationState::FailedRecoverable),
        "rolled_back" => Ok(OperationState::RolledBack),
        "stage_pending" => Ok(OperationState::StagePending),
        "stage_applied" => Ok(OperationState::StageApplied),
        "hidden_in_database" => Ok(OperationState::HiddenInDatabase),
        "restore_pending" => Ok(OperationState::RestorePending),
        "restore_applied" => Ok(OperationState::RestoreApplied),
        "restored" => Ok(OperationState::Restored),
        "trash_pending" => Ok(OperationState::TrashPending),
        "trash_applied" => Ok(OperationState::TrashApplied),
        "database_finalized" => Ok(OperationState::DatabaseFinalized),
        "trash_outcome_unknown" => Ok(OperationState::TrashOutcomeUnknown),
        _ => Err(Error::InvariantViolation {
            why: "unknown operation state in SQLite".to_owned(),
        }),
    }
}
