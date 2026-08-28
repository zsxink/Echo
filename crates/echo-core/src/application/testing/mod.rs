//! Test doubles for the application ports (task 2.7).
//!
//! These fakes are **test-only**: they live under `#[cfg(any(test, feature =
//! "testkit"))]` so use-case tests can simulate permission revocation, crash
//! points, trash failure and out-of-order watcher events without touching a
//! real user directory or real SQLite.
//!
//! The whole module is scaffolding, not shipped business code: it deliberately
//! uses casts for test data, procedural (not pure-functional) helper closures
//! and permissive docs. Enforcing pedantic on it would add noise without
//! protecting any production invariant, so the pedantic/nursery lints are
//! scoped out here while remaining `-D warnings` clean in `echo-core` proper.
//!
//! Submodules (each holds one family of doubles and its unit tests):
//!
//! - [`clock`] — [`FakeClock`], [`FakeIdGenerator`]: deterministic time & identity.
//! - [`repositories`] — [`MemorySongRepository`], [`MemoryPlaylistRepository`],
//!   [`MemoryOperationJournal`], [`MemoryLibraryRepository`]: in-memory stores.
//! - [`unit_of_work`] — [`MemoryUnitOfWork`]: atomic in-memory transactions.
//! - [`filesystem`] — [`FakeLibraryFileSystem`]: an in-temp-dir, root-constrained
//!   FS with scriptable permission/IO faults.
//! - [`small_fakes`] — [`FakeTrash`], [`ScriptedFileEvents`], [`FakeMediaProbe`],
//!   [`FakeMetadataReader`], [`FakeHasher`], [`MemoryCoverCache`],
//!   [`FakeLyricsParser`]: small deterministic fakes.
//!
//! The fakes never touch the user's home or an OS trash; temp dirs come from
//! the `tempfile` crate.

#![allow(
    clippy::missing_panics_doc,
    clippy::missing_errors_doc,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_lossless,
    clippy::too_many_lines,
    clippy::must_use_candidate,
    clippy::unnecessary_to_owned,
    clippy::redundant_clone,
    clippy::doc_markdown,
    clippy::let_and_return,
    clippy::needless_borrow,
    clippy::needless_pass_by_value,
    clippy::manual_let_else,
    clippy::unchecked_time_subtraction,
    clippy::wildcard_imports,
    clippy::bool_assert_comparison,
    clippy::type_complexity,
    clippy::missing_const_for_fn,
    clippy::significant_drop_in_scrutinee,
    clippy::significant_drop_tightening,
    clippy::manual_map,
    clippy::map_unwrap_or
)]

pub mod clock;
pub mod filesystem;
pub mod repositories;
pub mod small_fakes;
pub mod unit_of_work;

pub use clock::{FakeClock, FakeIdGenerator};
pub use filesystem::FakeLibraryFileSystem;
pub use repositories::{
    MemoryLibraryRepository, MemoryOperationJournal, MemoryPlaylistRepository, MemorySongRepository,
};
pub use small_fakes::{
    FakeHasher, FakeLyricsParser, FakeMediaProbe, FakeMetadataReader, FakeTrash, MemoryCoverCache,
    ScriptedFileEvents,
};
pub use unit_of_work::MemoryUnitOfWork;
