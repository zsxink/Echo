//! 队列条目与条目所引用的内容类型。
//!
//! 本模块只回答「队列里放的是什么」：一条队列条目（[`QueueEntry`]）、它引用的
//! 内容（[`QueueItem`]）、一次播放留下的历史痕迹（[`HistoryRecord`]），以及会话
//! 内的临时播放项（[`TemporaryItem`] 及其元数据与歌词）。
//!
//! 顺序、推进与移除规则不在这里，而在同级的 `ordering` / `mutation` 模块。

use echo_core::domain::ids::{QueueEntryId, SongId};

/// One entry in the queue. The identity is the [`QueueEntryId`]; the item is
/// either a library song (by `SongId`) or a session-only temporary file.
#[derive(Clone, Debug, PartialEq)]
pub struct QueueEntry {
    pub id: QueueEntryId,
    pub item: QueueItem,
}

/// One visit to a queue entry, recorded at the wall-clock instant playback
/// moved to it.  It deliberately keeps the queue-entry identity rather than a
/// song identity: repeated songs remain independently navigable.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HistoryRecord {
    pub entry_id: QueueEntryId,
    pub played_at_ms: u64,
}

/// What a queue entry refers to.
#[derive(Clone, Debug, PartialEq)]
pub enum QueueItem {
    /// A library song, resolved by stable `SongId` to its file (task 9.2).
    Library(SongId),
    /// A session-only temporary item with its validated absolute path and a
    /// snapshot of its metadata. Not persisted; filtered on save (8.9).
    ///
    /// Boxed on purpose: the queue holds a whole playback context in one
    /// `Vec<QueueEntry>`, so an inline `TemporaryItem` — path, six metadata
    /// fields and the parsed lyrics — would set the size of *every* entry,
    /// including the overwhelmingly common `Library(SongId)`, which is a
    /// 16-byte id. At most one entry is ever temporary.
    Temporary(Box<TemporaryItem>),
}

/// A temporary (session-only) playback item outside the active library.
#[derive(Clone, Debug, PartialEq)]
pub struct TemporaryItem {
    /// Display name (never a filesystem path).
    pub display_name: String,
    /// Validated absolute path to the local media file.
    pub path: std::path::PathBuf,
    /// Best-effort duration (seconds), `None` if unknown yet.
    pub duration: Option<f64>,
    /// Metadata parsed directly from the opened file. It is session-only and
    /// never becomes a library record until the user explicitly imports it.
    pub metadata: TemporaryMetadata,
    /// Whether this temporary item's file is on the *currently active* root
    /// (i.e. it was opened from a non-active root / outside the library).
    pub on_active_root: bool,
}

/// Presentation metadata for a temporary file-open playback item.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TemporaryMetadata {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration: Option<f64>,
    pub cover_key: Option<String>,
    pub lyrics: Option<TemporaryLyrics>,
}

/// Parsed lyrics carried by a temporary playback item.
#[derive(Clone, Debug, PartialEq)]
pub struct TemporaryLyrics {
    pub source: Option<String>,
    pub timed: bool,
    pub lines: Vec<TemporaryLyricLine>,
    pub plain_text: String,
    pub parse_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TemporaryLyricLine {
    pub seconds: f64,
    pub text: String,
}

impl QueueItem {
    /// The `SongId` when this item is a library song.
    #[must_use]
    pub const fn song_id(&self) -> Option<SongId> {
        match self {
            Self::Library(id) => Some(*id),
            Self::Temporary(_) => None,
        }
    }

    /// Whether two items denote the *same playable* for "play next"
    /// dedup/promote purposes.  Library songs match by stable `SongId`;
    /// session-only temporary items match by the path that identifies them.
    /// This deliberately does NOT collapse `QueueEntryId`s — two library
    /// entries for one song remain distinct ordinary queue members.
    #[must_use]
    pub fn matches_identity(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Library(a), Self::Library(b)) => a == b,
            (Self::Temporary(a), Self::Temporary(b)) => a.path == b.path,
            _ => false,
        }
    }
}
