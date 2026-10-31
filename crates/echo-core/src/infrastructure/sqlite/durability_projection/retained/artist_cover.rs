//! Project local artwork preferences without creating shared mutation events.
use super::{storage, Reason};
use crate::domain::ids::LibraryRootId;
use crate::domain::library::durability::{
    ArtistCoverState, DurableStateChange, ProjectionTarget, StateKind,
};
use crate::domain::library::{LibraryId, PortableAssetReference};
use crate::error::Error;
use rusqlite::{params, Connection, OptionalExtension};

pub(super) fn read(
    connection: &Connection,
    root: LibraryRootId,
    artist_key: &str,
) -> Result<Option<PortableAssetReference>, Error> {
    let library: Option<String> = connection
        .query_row(
            "SELECT library_id FROM library_projection_watermarks WHERE library_root_uuid=?1",
            [root.to_string()],
            |row| row.get(0),
        )
        .optional()
        .map_err(storage)?;
    let Some(library) = library else {
        return Ok(None);
    };
    let library = library.parse::<LibraryId>()?;
    let artwork = ArtistCoverState::new(library, artist_key.to_owned(), None)?;
    let key = serde_json::to_string(&ProjectionTarget::State {
        state_kind: StateKind::ArtistCover,
        id: artwork.id(),
    })
    .map_err(storage)?;
    let json: Option<String> = connection.query_row(
        "SELECT envelope_json FROM library_recovery_records WHERE library_root_uuid=?1 AND object_key=?2 AND row_kind='state' AND unresolved_reason IS NULL",
        params![root.to_string(), key],
        |row| row.get(0),
    ).optional().map_err(storage)?;
    json.map(|json| {
        let change = serde_json::from_str(&json).map_err(storage)?;
        ArtistCoverState::from_change(&change).map(|state| state.asset)
    })
    .transpose()
    .map(Option::flatten)
}

pub(super) fn project(
    connection: &Connection,
    root: LibraryRootId,
    library: Option<LibraryId>,
    state: &DurableStateChange,
) -> Result<Option<Reason>, Error> {
    let artwork = ArtistCoverState::from_change(state)?;
    if library != Some(artwork.library_id) {
        return Ok(Some(Reason::IdentityConflict));
    }
    if let Some(asset) = artwork.asset {
        connection.execute(
            "INSERT INTO artist_covers(library_root_uuid,artist_key,cover_asset_key,updated_at) VALUES(?1,?2,?3,0) ON CONFLICT(library_root_uuid,artist_key) DO UPDATE SET cover_asset_key=excluded.cover_asset_key",
            params![root.to_string(), artwork.artist_key, crate::domain::media::cover_asset_key(&asset.content_hash)],
        ).map_err(storage)?;
    } else {
        connection
            .execute(
                "DELETE FROM artist_covers WHERE library_root_uuid=?1 AND artist_key=?2",
                params![root.to_string(), artwork.artist_key],
            )
            .map_err(storage)?;
    }
    Ok(None)
}
