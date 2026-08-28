//! Tiny shared low-level helpers for the SQLite adapter.
//!
//! These are glue (error mapping, epoch time, ID parsing) rather than one
//! cohesive concern; they live here so the other submodules share them.

#![allow(
    clippy::doc_markdown,
    clippy::missing_errors_doc,
    clippy::needless_pass_by_value,
    clippy::redundant_pub_crate,
    clippy::uninlined_format_args
)]

use std::str::FromStr;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::error::Error;

/// Map any error into the storage error variant (`what: "sqlite"`).
pub(crate) fn storage(source: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Error {
    Error::Storage {
        what: "sqlite".to_owned(),
        source: source.into(),
    }
}

/// Convert a domain [`Error`] into a `rusqlite` conversion failure so a row
/// mapper can surface it through `?` on a `rusqlite::Result`.
pub(crate) fn to_sql_error(error: Error) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}

/// Map a SQLite constraint violation to a conflict error, other failures to
/// storage errors.
pub(crate) fn map_constraint(error: rusqlite::Error) -> Error {
    if matches!(error, rusqlite::Error::SqliteFailure(ref code, _) if code.code == rusqlite::ErrorCode::ConstraintViolation)
    {
        Error::conflict("SQLite uniqueness constraint")
    } else {
        storage(error)
    }
}

/// Milliseconds since the epoch, as `i64` (the SQLite INTEGER timestamp).
pub(crate) fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(i64::MAX)
}

/// Parse a stored UUID string back into a typed ID.
pub(crate) fn parse_id<T: FromStr<Err = Error>>(value: &str, _kind: &str) -> Result<T, Error> {
    value.parse()
}
