//! Domain entities and value objects (task 2.2).
//!
//! The entities model the library's persistent state the way the business
//! describes it, separately from any SQL row or DTO:
//!
//! - [`Song`]: a media record with a stable [`SongId`], its root, its relative
//!   path, availability (available / missing / pending-delete), and all the
//!   derived presentation fields (title/artist/…, sort keys, favorite, play
//!   count).
//! - [`LibraryRoot`]: a chosen root directory with write capability and
//!   availability.
//! - [`PlaylistMember`]: a playlist membership row — a song identity plus its
//!   append position and an availability mirror for the playlist UI.
//! - [`LyricsCandidate`]: one parsed lyrics *source* (override / embedded /
//!   sidecar) whose relative priority is defined by its kind.
//! - [`MediaDiagnostic`]: a per-file diagnostic emitted by scan/import.
//!
//! Invariants that live here:
//!
//! - A `Song` is in exactly one of `available`, `missing`, `pendingDelete` —
//!   there is no "deleted" state that drops the identity; Echo deliberately
//!   keeps the record so references stay stable.
//! - A root's `write_capable` flag is derived from permissions + ownership
//!   marker; `active` is a separate concept (only one active root exists, but
//!   that invariant lives in the state machine / repository, not per-entity).
//! - A playlist member references a `SongId` and carries no path; the member's
//!   availability mirrors the song's but is allowed to lag one reconcile tick
//!   (a stale mirror does not corrupt identity).
//! - Lyrics priority is a total order: `Override > Embedded > Sidecar`.

mod diag;
mod lyrics;
mod member;
mod root;
mod song;

#[cfg(test)]
mod tests;

pub use diag::MediaDiagnostic;
pub use lyrics::{select_effective_lyrics, LyricsCandidate, LyricsLine, LyricsSource};
pub use member::PlaylistMember;
pub use root::{LibraryRoot, RootAvailability};
pub use song::{Song, SongAvailability};
