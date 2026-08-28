//! Tauri command/event DTO mapping (task 7.2).
//!
//! Defines serde camelCase DTOs and `IpcError` for the Tauri boundary and
//! generates read-only TypeScript types. Core domain types are mapped here;
//! domain entities never derive Tauri/TypeScript traits directly.
