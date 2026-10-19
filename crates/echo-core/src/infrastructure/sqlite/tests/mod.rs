use super::*;
use crate::application::catalog::CatalogQuery;
use crate::application::ports::{
    DeviceIdProvider, LibraryRepository, OperationResourceKind, PlaylistRepository, SongRepository,
    SyncStateReader, TxAccess, UnitOfWork,
};
use crate::domain::catalog::{CatalogCollectionKind, SongSort, SongSortField, SortDirection};
use crate::domain::entities::{LyricsLine, RootAvailability, SongAvailability};
use crate::domain::ids::Revision;
use crate::domain::state::OperationState;
use crate::error::Error;
use rusqlite::params;
use std::sync::Arc;
use std::time::Duration;

include!("schema.rs");
include!("sync_payloads.rs");
include!("repositories.rs");
include!("operations.rs");
include!("songs.rs");
include!("performance.rs");
mod sync_baseline;
mod sync_configuration;
mod sync_manifest_validation;
mod sync_peer_lifecycle;
mod sync_publication;
mod sync_resource_publication;
mod sync_root_contexts;
mod sync_snapshot;
mod sync_snapshot_apply;
mod sync_song_projection;

mod resource_retention;
