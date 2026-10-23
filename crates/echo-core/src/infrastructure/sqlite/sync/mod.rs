//! SQLite adapters for synchronization persistence.
//!
//! The parent [`super`] module continues to own `SqliteDatabase`, its writer
//! actor, reader pool, schema and transaction coordination.

#![allow(clippy::redundant_pub_crate)]

// Preserve the established sibling path while compiling the implementation once.
#[path = "sync.rs"]
pub(crate) mod enqueue;
pub(crate) use enqueue::*;
#[path = "sync_context.rs"]
pub(crate) mod sync_context;
#[path = "sync_manifests.rs"]
pub(crate) mod sync_manifests;
#[path = "sync_projection.rs"]
pub(crate) mod sync_projection;
#[path = "sync_song_aliases.rs"]
pub(crate) mod sync_song_aliases;
pub(crate) use sync_context as context;
pub(crate) use sync_manifests as manifests;
pub(crate) use sync_projection as projection;
pub(crate) use sync_song_aliases as song_aliases;

#[path = "sync_apply.rs"]
pub(crate) mod apply;
#[path = "sync_baseline.rs"]
mod baseline;
#[path = "cache_cleanup.rs"]
mod cache_cleanup;
#[path = "sync_configuration.rs"]
pub(crate) mod configuration;
#[path = "sync_conflicts.rs"]
mod conflicts;
#[path = "sync_identity_repair.rs"]
pub mod identity_repair;
#[path = "sync_ledger.rs"]
pub(crate) mod ledger;
#[path = "sync_queue.rs"]
pub(crate) mod queue;
#[path = "sync_recovery.rs"]
mod recovery;
#[path = "remote_resources.rs"]
mod remote_resources;
#[path = "resource_retention.rs"]
mod resource_retention;
#[path = "sync_snapshot.rs"]
mod snapshot;
#[path = "sync_snapshot_apply.rs"]
mod snapshot_apply;

pub(crate) mod durability;
