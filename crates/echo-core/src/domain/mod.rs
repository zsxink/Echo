//! Domain layer: entities, value objects, state machines and invariants.
//!
//! This layer must not depend on the database, file system, network, UI or any
//! concrete framework. It is expanded by the domain tasks in the 0.1.0 change.

pub mod catalog;
pub mod entities;
pub mod ids;
pub mod media;
pub mod state;
pub mod text;
