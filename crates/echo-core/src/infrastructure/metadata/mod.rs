//! Metadata infrastructure: probing, tags, lyrics parsing and cover caching
//! (tasks 4.3–4.6).
//!
//! Submodules:
//!
//! - [`probe`] — the Symphonia [`MediaProbe`](crate::application::ports::MediaProbe):
//!   container format, audio-track presence, duration and audio parameters,
//!   independent of tag reading.
//! - [`tags`] — the lofty [`MetadataReader`](crate::application::ports::MetadataReader):
//!   tags, embedded lyrics and cover art with the phase-4 input limits
//!   (tag 4 KiB / lyrics 2 MiB / cover 20 MiB).
//! - [`lrc`] — the [`LyricsParser`](crate::application::ports::LyricsParser):
//!   LRC timestamps (single, multiple, per-line), plain-text fallback.
//! - [`cover`] — the disk-backed [`CoverCache`](crate::application::ports::CoverCache):
//!   content-hash keys, list/detail thumbnails, strict key validation and a
//!   referenced-set-aware GC.
//!
//! None of these adapters define business rules; they implement the ports.

pub mod cover;
pub mod lrc;
pub mod probe;
pub mod tags;

pub use cover::DiskCoverCache;
pub use lrc::LrcLyricsParser;
pub use probe::SymphoniaMediaProbe;
pub use tags::{InputLimits, LoftyMetadataReader};

/// The phase-4 input limits (task 4.4), in bytes.
impl InputLimits {
    /// Default limits: tag field 4 KiB, lyrics candidate 2 MiB, cover 20 MiB.
    #[must_use]
    pub const fn defaults() -> Self {
        Self {
            tag_field: 4 * 1024,
            lyrics: 2 * 1024 * 1024,
            cover: 20 * 1024 * 1024,
        }
    }
}
