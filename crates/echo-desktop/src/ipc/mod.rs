//! Tauri command/event DTO mapping (task 7.2).
//!
//! Defines serde camelCase DTOs and [`IpcErrorDto`] for the Tauri boundary and
//! generates read-only TypeScript types. Core domain types are mapped here;
//! domain entities never derive Tauri/TypeScript traits directly.
//!
//! `unsafe` is confined to the `player` subtree (see crate README); this module
//! forbids it.
#![forbid(unsafe_code)]

pub mod dto;
pub mod error;
pub mod events;
pub mod generate;

pub use dto::*;
pub use error::IpcErrorDto;
pub use events::*;
pub use generate::generated_typescript;
