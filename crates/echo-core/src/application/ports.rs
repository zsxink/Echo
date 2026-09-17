//! Application-layer ports, grouped by the capability each adapter provides.
//!
//! This module is the stable public entry point. Splitting the declarations
//! below deliberately does not change `application::ports::*` imports for
//! callers or adapters.

#![allow(clippy::missing_errors_doc)]

use std::io::Read;
use std::time::Duration;

use crate::domain::catalog::{CatalogCounts, OpaqueCursor, Paged, SongSort};
use crate::domain::entities::{
    LibraryRoot, LyricsCandidate, LyricsSource, MediaDiagnostic, PlaylistMember, Song,
    SongAvailability,
};
use crate::domain::ids::{
    LibraryRootId, OperationId, PlaylistId, RelativeMediaPath, Revision, SongId,
};
use crate::domain::library::{
    DeviceId, HybridLogicalClock, LibraryManifest, PortableRecord, RecordKind,
};
use crate::domain::media::{AudioFormat, ParsedMetadata};
use crate::domain::state::scan::{ScanProgress, ScanState};
use crate::error::Error;

pub(crate) mod filesystem;
mod filesystem_capabilities;
mod media;
mod repository;
mod state;
mod system;
mod transaction;

pub use filesystem::{
    FileMeta, ImportSource, ImportSourceInfo, ImportSourceReader, SidecarInfo, StagedCopy,
    StagedResource,
};
pub use filesystem_capabilities::*;
pub use media::*;
pub use repository::*;
pub use state::*;
pub use system::*;
pub use transaction::*;

/// Root-wide rescan sentinel path for [`FileEventKind::RescanNeeded`] events
/// that have no single triggering file (queue overflow, root replacement).
pub const RESCAN_SENTINEL: &str = ".echo-rescan";
