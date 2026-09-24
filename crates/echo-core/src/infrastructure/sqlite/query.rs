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

use crate::domain::catalog::{
    CatalogCounts, OpaqueCursor, Paged, SongSort, SongSortField, SortDirection,
};
use crate::domain::entities::Song;
use crate::domain::ids::{LibraryRootId, PlaylistId, Revision, SongId};
use crate::error::{Error, Subject};

use super::conversion::song_from_row;
use super::support::{parse_id, storage};

/// Column list shared by every read that produces a `Song`. The trailing four
/// columns are the scan bookkeeping facts (phase 4): hash, size, mtime, format.
pub(crate) const SONG_SELECT: &str = "SELECT s.uuid, s.library_root_uuid, s.relative_path, s.availability, s.is_favorite, s.play_count, s.revision, s.added_at, s.title, s.artist, s.album, s.duration_ms, s.updated_at, s.blake3_hash, s.file_size, s.file_mtime_ns, s.format, s.bitrate_bps, s.sample_rate_hz, s.channels, s.bits_per_sample FROM songs s";

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
    favorites: bool,
    playlist: Option<PlaylistId>,
    sort: SongSort,
    cursor: Option<&OpaqueCursor>,
    limit: usize,
) -> Result<Paged<Song>, Error> {
    if favorites && playlist.is_some() {
        return Err(Error::InvariantViolation {
            why: "search cannot restrict to favorites and a playlist at once".to_owned(),
        });
    }
    // These reads must share one WAL snapshot. A writer may commit on the
    // separate writer connection between statements; without this transaction
    // the total, cursor revision, and page contents could describe different
    // catalog states.
    let transaction = connection.unchecked_transaction().map_err(storage)?;
    let root = active_root_id(&transaction)?
        .ok_or_else(|| Error::unavailable("library", "no active root"))?;
    let revision: u64 = transaction
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
        if playlist.is_some() {
            "s.availability <> 'pending_delete'".to_owned()
        } else {
            "s.availability = 'available'".to_owned()
        },
    ];
    let mut values = vec![Value::Text(root.to_string())];
    if favorites {
        clauses.push("s.is_favorite = 1".to_owned());
    }
    if let Some(playlist) = playlist {
        clauses.push(
            "EXISTS (SELECT 1 FROM playlist_songs ps \
             WHERE ps.song_uuid = s.uuid AND ps.playlist_uuid = ?)"
                .to_owned(),
        );
        values.push(Value::Text(playlist.to_string()));
    }
    let (search_clauses, search_values) = query_clauses(query);
    clauses.extend(search_clauses);
    values.extend(search_values);
    // Count the full result set from the view and search predicates. Cursor
    // predicates are deliberately added only afterwards so pagination never
    // changes the displayed total.
    let count_sql = format!(
        "SELECT COUNT(*) FROM songs s WHERE {}",
        clauses.join(" AND ")
    );
    let total_count: i64 = transaction
        .query_row(&count_sql, params_from_iter(values.clone()), |row| {
            row.get(0)
        })
        .map_err(storage)?;
    if let Some(cursor) = cursor {
        let id = decode_cursor(cursor.keyset())?;
        let keys = cursor_keys(&transaction, id, sort, favorites)?;
        let (predicate, cursor_values) = keyset_predicate(sort, keys, favorites)?;
        clauses.push(predicate);
        values.extend(cursor_values);
    }
    let order = sort_sql(sort, favorites);
    let sql = format!(
        "{} WHERE {} ORDER BY {} LIMIT ?",
        SONG_SELECT,
        clauses.join(" AND "),
        order
    );
    values.push(Value::Integer(i64::try_from(limit + 1).unwrap_or(501)));
    let mut statement = transaction.prepare(&sql).map_err(storage)?;
    let mut songs = statement
        .query_map(params_from_iter(values), song_from_row)
        .map_err(storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage)?;
    drop(statement);
    let is_last = songs.len() <= limit;
    if !is_last {
        songs.pop();
    }
    let next_cursor = songs
        .last()
        .map(|song| OpaqueCursor::encode(Revision::from_u64(revision), encode_cursor(song.id())));
    let page = Paged::new(songs, if is_last { None } else { next_cursor }, is_last)
        .with_total_count(usize::try_from(total_count).unwrap_or(0));
    transaction.commit().map_err(storage)?;
    Ok(page)
}

/// Predicates a search term contributes to the active view query: a normalized
/// LIKE across title/artist/album for short terms (≤2 scalars), or an FTS5
/// `song_search` match for longer ones. An empty query contributes nothing, so
/// the caller restores the full underlying view.
fn query_clauses(query: &str) -> (Vec<String>, Vec<Value>) {
    if query.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let mut clauses = Vec::new();
    let mut values = Vec::new();
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
            "s.uuid IN (SELECT song_uuid FROM song_search WHERE song_search MATCH ?)".to_owned(),
        );
        values.push(Value::Text(escape_match(query)));
    }
    (clauses, values)
}

/// Per-view song totals for the navigation sidebar.
///
/// Two `COUNT(*)`s, not three: `recent` is not an independent population — the
/// "最近添加" view is *defined* as the newest [`RECENT_VIEW_LIMIT`] available
/// songs, so its count is `min(all, 100)` and is clamped in
/// [`CatalogCounts::new`]. Counting it separately would mean re-stating the
/// ceiling in SQL, and the two would drift the day the view's definition moves.
///
/// Both counts reuse the view-membership predicates of [`query_active`]
pub(crate) fn catalog_counts(connection: &Connection) -> Result<CatalogCounts, Error> {
    let root = active_root_id(connection)?
        .ok_or_else(|| Error::unavailable("library", "no active root"))?;
    let available = count_available_songs(connection, root, false)?;
    let favorites = count_available_songs(connection, root, true)?;
    let (artists, albums) = connection
        .query_row(
            "SELECT COUNT(DISTINCT CASE WHEN artist_sort = '' THEN '未知艺人' ELSE artist_sort END), COUNT(DISTINCT CASE WHEN album_sort = '' THEN '未知专辑' ELSE album_sort END) FROM songs WHERE library_root_uuid = ?1 AND availability = 'available'",
            params![root.to_string()],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .map_err(storage)?;
    Ok(CatalogCounts::new(
        available,
        favorites,
        usize::try_from(artists).unwrap_or(0),
        usize::try_from(albums).unwrap_or(0),
    ))
}

fn count_available_songs(
    connection: &Connection,
    root: LibraryRootId,
    favorites: bool,
) -> Result<usize, Error> {
    let sql = if favorites {
        "SELECT COUNT(*) FROM songs WHERE library_root_uuid = ?1 AND availability = 'available' AND is_favorite = 1"
    } else {
        "SELECT COUNT(*) FROM songs WHERE library_root_uuid = ?1 AND availability = 'available'"
    };
    let count: i64 = connection
        .query_row(sql, params![root.to_string()], |row| row.get(0))
        .map_err(storage)?;
    Ok(usize::try_from(count).unwrap_or(0))
}

fn sort_sql(sort: SongSort, favorites: bool) -> String {
    if favorites && sort.field == SongSortField::AddedAt {
        let direction = if sort.direction == SortDirection::Asc {
            "ASC"
        } else {
            "DESC"
        };
        return format!("COALESCE(s.favorited_at, s.added_at) {direction}, s.uuid {direction}");
    }
    let direction = if sort.direction == SortDirection::Asc {
        "ASC"
    } else {
        "DESC"
    };
    let columns = match sort.field {
        SongSortField::AddedAt => "s.added_at, s.uuid",
        SongSortField::Title => "s.title_sort, s.artist_sort, s.uuid",
        SongSortField::Artist => "s.artist_sort, s.title_sort, s.uuid",
        SongSortField::Album => "CASE WHEN s.album_sort = '' THEN '未知专辑' ELSE s.album_sort END, s.title_sort, s.artist_sort, s.uuid",
        SongSortField::PlayCount => "s.play_count, s.title_sort, s.artist_sort, s.uuid",
    };
    columns
        .split(", ")
        .map(|column| format!("{column} {direction}"))
        .collect::<Vec<_>>()
        .join(", ")
}
fn cursor_keys(
    connection: &Connection,
    id: SongId,
    sort: SongSort,
    favorites: bool,
) -> Result<Vec<Value>, Error> {
    let columns = if favorites && sort.field == SongSortField::AddedAt {
        "COALESCE(favorited_at, added_at), uuid"
    } else {
        match sort.field {
            SongSortField::AddedAt => "added_at, uuid",
            SongSortField::Title => "title_sort, artist_sort, uuid",
            SongSortField::Artist => "artist_sort, title_sort, uuid",
            SongSortField::Album => "CASE WHEN album_sort = '' THEN '未知专辑' ELSE album_sort END, title_sort, artist_sort, uuid",
            SongSortField::PlayCount => "play_count, title_sort, artist_sort, uuid",
        }
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
fn keyset_predicate(
    sort: SongSort,
    keys: Vec<Value>,
    favorites: bool,
) -> Result<(String, Vec<Value>), Error> {
    let columns: Vec<&str> = if favorites && sort.field == SongSortField::AddedAt {
        vec!["COALESCE(s.favorited_at, s.added_at)", "s.uuid"]
    } else {
        match sort.field {
            SongSortField::AddedAt => vec!["s.added_at", "s.uuid"],
            SongSortField::Title => vec!["s.title_sort", "s.artist_sort", "s.uuid"],
            SongSortField::Artist => vec!["s.artist_sort", "s.title_sort", "s.uuid"],
            SongSortField::Album => vec![
                "CASE WHEN s.album_sort = '' THEN '未知专辑' ELSE s.album_sort END",
                "s.title_sort",
                "s.artist_sort",
                "s.uuid",
            ],
            SongSortField::PlayCount => {
                vec!["s.play_count", "s.title_sort", "s.artist_sort", "s.uuid"]
            }
        }
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

/// One playlist's song rows over the **active root**, ordered by member
/// position (stable) with a UUID tie-break. Available and externally-missing
/// members are shown (so a blocked row can display), pending-delete members
/// are hidden (task 6.1 "歌单" view; the missing/blocked display refinement
/// is task 6.7).
pub(crate) fn playlist_songs_query(
    connection: &Connection,
    playlist: PlaylistId,
) -> Result<Vec<Song>, Error> {
    let root = active_root_id(connection)?
        .ok_or_else(|| Error::unavailable("library", "no active root"))?;
    let mut statement = connection
        .prepare(&format!(
            "{} JOIN playlist_songs ps ON ps.song_uuid = s.uuid \
             WHERE ps.playlist_uuid = ?1 AND s.library_root_uuid = ?2 \
             AND s.availability <> 'pending_delete' \
             ORDER BY ps.position, s.uuid",
            SONG_SELECT
        ))
        .map_err(storage)?;
    let songs = statement
        .query_map(
            params![playlist.to_string(), root.to_string()],
            song_from_row,
        )
        .map_err(storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage)?;
    Ok(songs)
}
