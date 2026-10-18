//! Application-layer ports, grouped by the capability each adapter provides.
//!
//! This module is the stable public entry point. Splitting the declarations
//! below deliberately does not change `application::ports::*` imports for
//! callers or adapters.

#![allow(clippy::missing_errors_doc)]

pub(crate) mod filesystem;
mod filesystem_capabilities;
mod media;
mod remote_resources;
mod repository;
mod resource_retention;
mod state;
mod sync;
mod sync_recovery;
mod system;
mod transaction;

pub use filesystem::{
    FileMeta, ImportSource, ImportSourceInfo, ImportSourceReader, SidecarInfo, StagedCopy,
    StagedResource,
};
pub use filesystem_capabilities::*;
pub use media::*;
pub use remote_resources::*;
pub use repository::*;
pub use resource_retention::*;
pub use state::*;
pub use sync::*;
pub use sync_recovery::*;
pub use system::*;
pub use transaction::*;

/// Root-wide rescan sentinel path for [`FileEventKind::RescanNeeded`] events
/// that have no single triggering file (queue overflow, root replacement).
pub const RESCAN_SENTINEL: &str = ".echo-rescan";
