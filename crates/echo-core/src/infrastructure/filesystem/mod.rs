//! Root-constrained file-system adapter (task 4.2).
//!
//! Everything the use cases see crosses the boundary as a [`LibraryRootId`]
//! plus a [`RelativeMediaPath`]; the absolute path is resolved here, inside
//! the adapter, from the [`RootRegistry`] the desktop runtime keeps current.
//!
//! Submodules:
//!
//! - [`registry`] — the root-id → absolute-path map shared by every adapter.
//! - [`staging`] — the random, marker-verified controlled staging directory.
//! - [`adapter`] — the [`LibraryFileSystem`](crate::application::ports::LibraryFileSystem)
//!   implementation (enumeration, metadata, head reads, publish).
//! - [`hasher`] — the BLAKE3 [`ContentHasher`](crate::application::ports::ContentHasher).
//! - [`watcher`] — the `notify`-backed [`FileEventSource`](crate::application::ports::FileEventSource)
//!   with per-path debounce and file-stability double sampling.
//!
//! Boundary rules implemented here (design §5/§6/§8):
//!
//! - Enumeration never follows directory symlinks; a candidate file's
//!   canonical path must stay inside the canonical root, which rejects both
//!   escaping symlinks and symlink cycles.
//! - The staging directory is exclusive-created under a random name and owned
//!   via an application-magic marker file; a same-named user directory, an
//!   invalid marker or a symlinked directory makes Echo pick another random
//!   name — never take over, ignore or clean up a directory it does not own.
//! - The scan walker ignores a `.echo-staging-*` directory **only** when its
//!   marker fully matches (`magic` + `LibraryRootId` + format version).

pub mod adapter;
pub mod control_plane;
pub mod hasher;
pub mod registry;
pub mod staging;
pub mod walker;
pub mod watcher;

pub use adapter::RootConstrainedFileSystem;
pub use control_plane::RootControlPlane;
pub use hasher::Blake3ContentHasher;
pub use registry::RootRegistry;
pub use staging::{
    StagingCheck, StagingDecision, StagingManager, MARKER_FILE_NAME, STAGING_DIR_PREFIX,
};
pub use watcher::{NotifyFileEventSource, WatcherTimings};
