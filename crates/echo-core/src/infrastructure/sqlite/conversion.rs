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
use crate::domain::media::{AudioFormat, AudioParameters};
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
    root.set_write_safety_locked(row.get::<_, i64>(5)? != 0);
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
    let audio_parameters = AudioParameters {
        bitrate_bps: row.get::<_, Option<u64>>(17)?,
        sample_rate_hz: row.get::<_, Option<u32>>(18)?,
        channels: row.get::<_, Option<u16>>(19)?,
        bits_per_sample: row.get::<_, Option<u16>>(20)?,
    };
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
        audio_parameters,
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
        "ape" => Ok(AudioFormat::Ape),
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
        item_key: row.get(5)?,
        claim_key: row.get(6)?,
        source: row.get(7)?,
        staging_path: row
            .get::<_, Option<String>>(8)?
            .map(|value| RelativeMediaPath::new(&value))
            .transpose()
            .map_err(to_sql_error)?,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::entities::LyricsSource;
    use crate::domain::ids::{LibraryRootId, SongId};
    use crate::domain::state::scan::ScanState;
    use crate::domain::state::OperationState as DomainOperationState;

    /// Feed literal columns through a real `SELECT` and hand the first row to a
    /// `..._from_row` mapper — the same path real queries use (a `rusqlite::Row`
    /// cannot be constructed by value outside a statement).
    fn first_row<T>(
        values: &[rusqlite::types::Value],
        mapper: impl FnOnce(&rusqlite::Row<'_>) -> rusqlite::Result<T>,
    ) -> rusqlite::Result<T> {
        let connection = rusqlite::Connection::open_in_memory().expect("in-memory connection");
        let placeholders: Vec<&str> = std::iter::repeat_n("?", values.len()).collect();
        let mut statement = connection
            .prepare(&format!("SELECT {}", placeholders.join(", ")))
            .expect("literal select");
        let mut rows = statement
            .query(rusqlite::params_from_iter(values))
            .expect("query literal row");
        let row = rows.next()?.expect("one literal row");
        mapper(row)
    }

    #[test]
    fn root_from_row_maps_active_flags_and_availability() {
        let id = LibraryRootId::new();
        let values = [
            rusqlite::types::Value::Text(id.to_string()),
            rusqlite::types::Value::Text("/tmp/library".to_owned()),
            rusqlite::types::Value::Integer(1),
            rusqlite::types::Value::Integer(0),
            rusqlite::types::Value::Text("unavailable".to_owned()),
            rusqlite::types::Value::Integer(1),
        ];
        let root = first_row(&values, root_from_row).expect("root row");
        assert_eq!(root.id(), id);
        assert_eq!(root.absolute_path(), std::path::Path::new("/tmp/library"));
        assert!(root.is_active());
        assert!(!root.observed_write_capable());
        assert!(
            root.availability() == RootAvailability::Unavailable,
            "storage string mapped to unavailable"
        );
        assert!(
            root.write_safety_locked(),
            "write_safety_locked column maps"
        );
    }

    #[test]
    fn scan_state_database_strings_round_trip() {
        for (value, expected) in [
            ("enumerating", ScanState::Enumerating),
            ("parsing", ScanState::Parsing),
            ("reconciling", ScanState::Reconciling),
            ("completed", ScanState::Completed),
            ("cancelled", ScanState::Cancelled),
            ("failed", ScanState::Failed),
            ("unknown-junk", ScanState::Queued),
        ] {
            assert_eq!(scan_state_from_db(value), expected, "{value}");
        }
    }

    #[test]
    fn audio_format_round_trips_and_rejects_unknown() {
        for format in [
            AudioFormat::Mpeg,
            AudioFormat::Flac,
            AudioFormat::Ape,
            AudioFormat::Mp4,
            AudioFormat::Ogg,
            AudioFormat::Opus,
            AudioFormat::Wav,
            AudioFormat::UnknownDamaged,
        ] {
            let db = audio_format_to_db(format);
            assert_eq!(audio_format_from_db(db).unwrap(), format, "{db}");
        }
        let error = audio_format_from_db("not-a-format").expect_err("unsupported encoding");
        assert!(matches!(error, Error::InvariantViolation { .. }));
    }

    #[test]
    fn lyrics_source_and_text_kind_encodings_round_trip() {
        let lines = vec![LyricsLine {
            timestamp_ms: 1234,
            text: "第一行".to_owned(),
            original_index: 0,
        }];
        let timed = LyricsCandidate::with_raw_text(
            LyricsSource::Embedded,
            "raw".to_owned(),
            lines.clone(),
            None,
            Some("diagnostic".to_owned()),
        );
        assert_eq!(text_kind_to_db(&timed), "timed");
        let plain = LyricsCandidate::new(LyricsSource::Sidecar, lines.clone(), true);
        assert_eq!(text_kind_to_db(&plain), "plain");
        let mut empty = LyricsCandidate::new(LyricsSource::Override, lines, false);
        empty.mark_empty_override();
        assert_eq!(text_kind_to_db(&empty), "empty");

        assert_eq!(
            lyrics_source_from_db("override").unwrap(),
            LyricsSource::Override
        );
        assert_eq!(
            lyrics_source_from_db("embedded").unwrap(),
            LyricsSource::Embedded
        );
        assert_eq!(
            lyrics_source_from_db("sidecar").unwrap(),
            LyricsSource::Sidecar
        );
        assert!(matches!(
            lyrics_source_from_db("remote").unwrap_err(),
            Error::InvariantViolation { .. }
        ));
    }

    #[test]
    fn timed_lines_json_round_trips_and_rejects_malformed() {
        let candidate = LyricsCandidate::new(
            LyricsSource::Embedded,
            vec![
                LyricsLine {
                    timestamp_ms: 0,
                    text: "第一".to_owned(),
                    original_index: 2,
                },
                LyricsLine {
                    timestamp_ms: 5_000,
                    text: "第二".to_owned(),
                    original_index: 0,
                },
            ],
            true,
        );
        let json = timed_lines_to_json(&candidate);
        let round = timed_lines_from_json(&json).expect("valid round trip");
        assert_eq!(round.len(), 2);
        // The encoded order is the candidate's line order; `original_index`
        // survives so a consumer can re-sort.
        assert_eq!(round[1].original_index, 0);

        let mut corrupted = serde_json::json!([{ "text": "missing timestamp" }]).to_string();
        let partial = timed_lines_from_json(&corrupted).expect_err("missing timestamp_ms");
        assert!(matches!(partial, Error::InvariantViolation { .. }));
        corrupted = "not json".to_owned();
        assert!(matches!(
            timed_lines_from_json(&corrupted).unwrap_err(),
            Error::InvariantViolation { .. }
        ));
    }

    #[test]
    fn availability_encodings_round_trip_and_reject_unknown() {
        for availability in [
            SongAvailability::Available,
            SongAvailability::Missing,
            SongAvailability::PendingDelete,
        ] {
            assert_eq!(
                availability_from_db(availability_to_db(availability)).unwrap(),
                availability
            );
        }
        assert!(matches!(
            availability_from_db("gone").unwrap_err(),
            Error::InvariantViolation { .. }
        ));
    }

    #[test]
    fn operation_states_encode_and_decode_with_inverse() {
        for state in [
            DomainOperationState::Planned,
            DomainOperationState::CopyPending,
            DomainOperationState::CopyApplied,
            DomainOperationState::ValidatePending,
            DomainOperationState::Validated,
            DomainOperationState::PublishPending,
            DomainOperationState::PublishApplied,
            DomainOperationState::DatabaseCommitted,
            DomainOperationState::Completed,
            DomainOperationState::FailedRecoverable,
            DomainOperationState::RolledBack,
            DomainOperationState::StagePending,
            DomainOperationState::StageApplied,
            DomainOperationState::HiddenInDatabase,
            DomainOperationState::RestorePending,
            DomainOperationState::RestoreApplied,
            DomainOperationState::Restored,
            DomainOperationState::TrashPending,
            DomainOperationState::TrashApplied,
            DomainOperationState::DatabaseFinalized,
            DomainOperationState::TrashOutcomeUnknown,
        ] {
            let db = operation_state_to_db(state);
            assert_eq!(operation_state_from_db(db).unwrap(), state, "{db}");
        }
        assert!(matches!(
            operation_state_from_db("evaporated").unwrap_err(),
            Error::InvariantViolation { .. }
        ));
    }

    #[test]
    fn song_from_row_maps_nullable_audio_parameters_and_scan_facts() {
        let id = SongId::new();
        let root = LibraryRootId::new();
        let path = RelativeMediaPath::new("歌手/晴天.flac").unwrap();
        let values = [
            rusqlite::types::Value::Text(id.to_string()),
            rusqlite::types::Value::Text(root.to_string()),
            rusqlite::types::Value::Text(path.display().to_owned()),
            rusqlite::types::Value::Text("missing".to_owned()),
            rusqlite::types::Value::Integer(1),  // favorite
            rusqlite::types::Value::Integer(3),  // play_count
            rusqlite::types::Value::Integer(7),  // revision
            rusqlite::types::Value::Integer(5),  // added_at
            rusqlite::types::Value::Null,        // title
            rusqlite::types::Value::Null,        // artist
            rusqlite::types::Value::Null,        // album
            rusqlite::types::Value::Null,        // duration_ms
            rusqlite::types::Value::Integer(10), // updated_at
            rusqlite::types::Value::Text("cafe-".repeat(32)), // blake3
            rusqlite::types::Value::Integer(12_345), // file_size
            rusqlite::types::Value::Integer(1_111), // file_mtime_ns
            rusqlite::types::Value::Text("flac".to_owned()), // format
            rusqlite::types::Value::Integer(1411), // bitrate
            rusqlite::types::Value::Integer(44_100), // sample_rate
            rusqlite::types::Value::Integer(2),  // channels
            rusqlite::types::Value::Null,        // bits_per_sample
        ];
        let song = first_row(&values, song_from_row).expect("song row");
        assert_eq!(song.id(), id);
        assert_eq!(song.root(), root);
        assert_eq!(song.path(), &path);
        assert_eq!(song.availability(), SongAvailability::Missing);
        assert!(song.favorite());
        assert_eq!(song.play_count().as_u64(), 3);
        assert_eq!(song.revision().as_u64(), 7);
        assert_eq!(song.added_at(), 5);
        assert!(song.duration().is_none());
        assert_eq!(song.blake3_hash(), Some("cafe-".repeat(32).as_str()));
        assert_eq!(song.file_size(), Some(12_345));
        assert_eq!(song.file_mtime_ns(), Some(1_111));
        assert_eq!(song.format(), Some(AudioFormat::Flac));
        let parameters = song.audio_parameters();
        assert_eq!(parameters.bitrate_bps, Some(1411));
        assert_eq!(parameters.sample_rate_hz, Some(44_100));
        assert_eq!(parameters.channels, Some(2));
        assert_eq!(parameters.bits_per_sample, None);
    }
}
