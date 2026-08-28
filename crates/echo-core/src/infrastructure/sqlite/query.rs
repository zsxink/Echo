//! Keyset-paginated catalog queries over the active root.

#![allow(
    clippy::doc_markdown,
    clippy::missing_errors_doc,
    clippy::needless_pass_by_value,
    clippy::redundant_pub_crate,
    clippy::uninlined_format_args
)]

use rusqlite::types::{Value, ValueRef};
use rusqlite::{params, params_from_iter, Connection, OptionalExtension};

use crate::domain::catalog::{OpaqueCursor, Paged, SongSort, SongSortField, SortDirection};
use crate::domain::entities::Song;
use crate::domain::ids::{LibraryRootId, Revision, SongId};
use crate::error::{Error, Subject};

use super::conversion::song_from_row;
use super::support::{parse_id, storage};

/// Column list shared by every read that produces a `Song`.
pub(crate) const SONG_SELECT: &str = "SELECT s.uuid, s.library_root_uuid, s.relative_path, s.availability, s.is_favorite, s.play_count, s.revision, s.added_at, s.title, s.artist, s.album, s.duration_ms, s.updated_at FROM songs s";

pub(crate) fn active_root_id(connection: &Connection) -> Result<Option<LibraryRootId>, Error> {
    connection
        .query_row(
            "SELECT uuid FROM library_roots WHERE is_active = 1",
            [],
            |row| row.get::<_, String>(0),
        )
        .optional()
        .map_err(storage)?
        .map(|value| parse_id(&value, "LibraryRootId"))
        .transpose()
}

pub(crate) fn query_active(
    connection: &Connection,
    query: &str,
    sort: SongSort,
    cursor: Option<&OpaqueCursor>,
    limit: usize,
) -> Result<Paged<Song>, Error> {
    let root = active_root_id(connection)?
        .ok_or_else(|| Error::unavailable("library", "no active root"))?;
    let revision: u64 = connection
        .query_row(
            "SELECT updated_at FROM library_roots WHERE uuid = ?1",
            params![root.to_string()],
            |row| row.get::<_, i64>(0),
        )
        .map_err(storage)?
        .try_into()
        .unwrap_or_default();
    if let Some(cursor) = cursor {
        if cursor.revision() != Revision::from_u64(revision) {
            return Err(Error::conflict("catalog changed; restart pagination"));
        }
    }
    let mut clauses = vec![
        "s.library_root_uuid = ?".to_owned(),
        "s.availability = 'available'".to_owned(),
    ];
    let mut values = vec![Value::Text(root.to_string())];
    if !query.is_empty() {
        if query.chars().count() < 3 {
            clauses.push("(s.title_sort LIKE ? ESCAPE '\\' OR s.artist_sort LIKE ? ESCAPE '\\' OR s.album_sort LIKE ? ESCAPE '\\')".to_owned());
            let like = format!("%{}%", escape_like(query));
            values.extend([
                Value::Text(like.clone()),
                Value::Text(like.clone()),
                Value::Text(like),
            ]);
        } else {
            clauses.push(
                "s.uuid IN (SELECT song_uuid FROM song_search WHERE song_search MATCH ?)"
                    .to_owned(),
            );
            values.push(Value::Text(escape_match(query)));
        }
    }
    if let Some(cursor) = cursor {
        let id = decode_cursor(cursor.keyset())?;
        let keys = cursor_keys(connection, id, sort)?;
        let (predicate, cursor_values) = keyset_predicate(sort, keys)?;
        clauses.push(predicate);
        values.extend(cursor_values);
    }
    let order = sort_sql(sort);
    let sql = format!(
        "{} WHERE {} ORDER BY {} LIMIT ?",
        SONG_SELECT,
        clauses.join(" AND "),
        order
    );
    values.push(Value::Integer(i64::try_from(limit + 1).unwrap_or(501)));
    let mut statement = connection.prepare(&sql).map_err(storage)?;
    let mut songs = statement
        .query_map(params_from_iter(values), song_from_row)
        .map_err(storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage)?;
    let is_last = songs.len() <= limit;
    if !is_last {
        songs.pop();
    }
    let next_cursor = songs
        .last()
        .map(|song| OpaqueCursor::encode(Revision::from_u64(revision), encode_cursor(song.id())));
    Ok(Paged::new(
        songs,
        if is_last { None } else { next_cursor },
        is_last,
    ))
}

fn sort_sql(sort: SongSort) -> String {
    let direction = if sort.direction == SortDirection::Asc {
        "ASC"
    } else {
        "DESC"
    };
    let columns = match sort.field {
        SongSortField::AddedAt => "s.added_at, s.uuid",
        SongSortField::Title => "s.title_sort, s.artist_sort, s.uuid",
        SongSortField::Artist => "s.artist_sort, s.title_sort, s.uuid",
        SongSortField::PlayCount => "s.play_count, s.title_sort, s.artist_sort, s.uuid",
    };
    columns
        .split(", ")
        .map(|column| format!("{column} {direction}"))
        .collect::<Vec<_>>()
        .join(", ")
}
fn cursor_keys(connection: &Connection, id: SongId, sort: SongSort) -> Result<Vec<Value>, Error> {
    let columns = match sort.field {
        SongSortField::AddedAt => "added_at, uuid",
        SongSortField::Title => "title_sort, artist_sort, uuid",
        SongSortField::Artist => "artist_sort, title_sort, uuid",
        SongSortField::PlayCount => "play_count, title_sort, artist_sort, uuid",
    };
    let mut statement = connection
        .prepare(&format!("SELECT {columns} FROM songs WHERE uuid = ?1"))
        .map_err(storage)?;
    statement
        .query_row(params![id.to_string()], |row| {
            let mut values = Vec::new();
            for index in 0..row.as_ref().column_count() {
                values.push(match row.get_ref(index)? {
                    ValueRef::Integer(value) => Value::Integer(value),
                    ValueRef::Text(value) => {
                        Value::Text(String::from_utf8_lossy(value).into_owned())
                    }
                    other => {
                        return Err(rusqlite::Error::FromSqlConversionFailure(
                            index,
                            other.data_type(),
                            Box::new(std::io::Error::other("invalid cursor value")),
                        ))
                    }
                });
            }
            Ok(values)
        })
        .optional()
        .map_err(storage)?
        .ok_or_else(|| Error::conflict("cursor song no longer exists"))
}
fn keyset_predicate(sort: SongSort, keys: Vec<Value>) -> Result<(String, Vec<Value>), Error> {
    let columns: Vec<&str> = match sort.field {
        SongSortField::AddedAt => vec!["s.added_at", "s.uuid"],
        SongSortField::Title => vec!["s.title_sort", "s.artist_sort", "s.uuid"],
        SongSortField::Artist => vec!["s.artist_sort", "s.title_sort", "s.uuid"],
        SongSortField::PlayCount => vec!["s.play_count", "s.title_sort", "s.artist_sort", "s.uuid"],
    };
    if columns.len() != keys.len() {
        return Err(Error::InvariantViolation {
            why: "cursor key shape does not match sort".to_owned(),
        });
    }
    let op = if sort.direction == SortDirection::Asc {
        ">"
    } else {
        "<"
    };
    let mut branches = Vec::new();
    let mut values = Vec::new();
    for index in 0..columns.len() {
        let mut conditions = Vec::new();
        for previous in 0..index {
            conditions.push(format!("{} = ?", columns[previous]));
            values.push(keys[previous].clone());
        }
        conditions.push(format!("{} {op} ?", columns[index]));
        values.push(keys[index].clone());
        branches.push(format!("({})", conditions.join(" AND ")));
    }
    Ok((format!("({})", branches.join(" OR ")), values))
}
fn encode_cursor(id: SongId) -> String {
    id.to_string().replace('-', "")
}
fn decode_cursor(value: &str) -> Result<SongId, Error> {
    if value.len() != 32 || !value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(Error::validation(
            Subject::Query,
            "cursor",
            "malformed keyset",
        ));
    }
    let mut hyphenated = String::with_capacity(36);
    for (index, byte) in value.chars().enumerate() {
        if matches!(index, 8 | 12 | 16 | 20) {
            hyphenated.push('-');
        }
        hyphenated.push(byte);
    }
    parse_id(&hyphenated, "SongId")
}
fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}
fn escape_match(value: &str) -> String {
    format!("\"{}\"", value.replace('"', "\"\""))
}
