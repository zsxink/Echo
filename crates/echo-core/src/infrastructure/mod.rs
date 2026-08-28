//! Infrastructure layer: Adapters implementing the application ports.
//!
//! `SQLite`, `file-system`, metadata parsing, hashing and lyrics/cover parsing
//! live here. This layer may not define business rules; it implements the
//! ports declared in [`crate::application`].

pub mod sqlite;
