//! Parse IPC arguments without coupling the command adapters to string formats.
use echo_core::domain::catalog::{OpaqueCursor, SongSort, SongSortField, SortDirection};
use echo_desktop::ipc::IpcErrorDto;

pub(super) fn parse_sort(token: &str) -> Result<SongSort, IpcErrorDto> {
    let (field, direction) = token.split_once(':').ok_or_else(|| {
        IpcErrorDto::from(&echo_core::error::Error::validation(
            echo_core::error::Subject::Other,
            "sort",
            "sort must be field:direction".to_owned(),
        ))
    })?;
    let field = match field {
        "addedAt" => SongSortField::AddedAt,
        "title" => SongSortField::Title,
        "artist" => SongSortField::Artist,
        "album" => SongSortField::Album,
        "playCount" => SongSortField::PlayCount,
        _ => {
            return Err(IpcErrorDto::from(&echo_core::error::Error::validation(
                echo_core::error::Subject::Other,
                "sort",
                "unknown sort field".to_owned(),
            )))
        }
    };
    let direction = match direction {
        "asc" => SortDirection::Asc,
        "desc" => SortDirection::Desc,
        _ => {
            return Err(IpcErrorDto::from(&echo_core::error::Error::validation(
                echo_core::error::Subject::Other,
                "sort",
                "unknown sort direction".to_owned(),
            )))
        }
    };
    Ok(SongSort { field, direction })
}

pub(super) fn parse_collection_kind(
    token: &str,
) -> Result<echo_core::domain::catalog::CatalogCollectionKind, IpcErrorDto> {
    match token {
        "artist" => Ok(echo_core::domain::catalog::CatalogCollectionKind::Artist),
        "album" => Ok(echo_core::domain::catalog::CatalogCollectionKind::Album),
        _ => Err(IpcErrorDto::from(&echo_core::error::Error::validation(
            echo_core::error::Subject::Query,
            "kind",
            "unknown collection kind".to_owned(),
        ))),
    }
}

pub(super) fn parse_cursor(token: Option<String>) -> Result<Option<OpaqueCursor>, IpcErrorDto> {
    let Some(token) = token else { return Ok(None) };
    // The frontend forwards the opaque cursor token (a JSON string) verbatim.
    // `OpaqueCursor` deserializes from a String; wrap with quotes so the raw
    // token round-trips.
    let quoted = format!("\"{token}\"");
    serde_json::from_str::<OpaqueCursor>(&quoted)
        .map(Some)
        .map_err(|_| IpcErrorDto::from(&echo_core::error::Error::conflict("stale cursor")))
}

pub(super) fn parse_id<T: std::str::FromStr>(
    raw: &str,
    field: &'static str,
) -> Result<T, IpcErrorDto> {
    raw.parse::<T>().map_err(|_| {
        IpcErrorDto::from(&echo_core::error::Error::validation(
            echo_core::error::Subject::Other,
            field,
            String::from("invalid id"),
        ))
    })
}
