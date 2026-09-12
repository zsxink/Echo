//! Infrastructure layer: Adapters implementing the application ports.
//!
//! `SQLite`, the root-constrained file system, metadata parsing/hashing and
//! cover caching live here. This layer may not define business rules; it
//! implements the ports declared in [`crate::application`].

pub mod filesystem;
pub mod metadata;
pub mod sqlite;

/// Production [`crate::application::ports::Clock`] and
/// [`crate::application::ports::IdGenerator`] adapters for the composition root.
pub mod core;
