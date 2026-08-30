//! IPC serde DTOs (task 7.2, design §10).
//!
//! These are the camelCase read models the frontend consumes. Core domain
//! entities are mapped *here*; entities never derive Tauri/TypeScript traits
//! directly. `RelativeMediaPath` is the only path form that crosses the
//! boundary — never an absolute path.

use serde::{Deserialize, Serialize};

use echo_core::domain::catalog::{OpaqueCursor, Paged};
use echo_core::domain::entities::{Song, SongAvailability};
use echo_core::domain::ids::{PlaylistId, SongId};

/// The bootstrap snapshot every session starts from (task 7.3 `get_bootstrap_state`).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BootstrapSnapshot {
    /// Whether the backend is ready to serve reads.
    pub ready: bool,
    /// Whether destructive operations are allowed (the recovery gate).
    pub writes_allowed: bool,
    /// The active library root id, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_root: Option<String>,
    /// How many journal operations recovery touched (informational).
    pub recovered_operations: u64,
}

/// One song as the UI sees it: presentation fields plus a relative path only.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SongView {
    pub id: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_s: Option<u64>,
    pub favorite: bool,
    pub play_count: u64,
    pub availability: String,
    /// The library-relative path (never absolute).
    pub relative_path: String,
}

impl From<&Song> for SongView {
    fn from(song: &Song) -> Self {
        Self {
            id: song.id().to_string(),
            title: song.title().map(ToOwned::to_owned),
            artist: song.artist().map(ToOwned::to_owned),
            album: song.album().map(ToOwned::to_owned),
            duration_s: song.duration().map(|d| d.as_secs()),
            favorite: song.favorite(),
            play_count: song.play_count().as_u64(),
            availability: availability_label(song.availability()),
            relative_path: song.path().display().to_string(),
        }
    }
}

/// A keyset-paginated page of song views.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PagedSongs {
    pub items: Vec<SongView>,
    /// Opaque next-page cursor, if not the last page.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    pub is_last: bool,
}

impl From<Paged<Song>> for PagedSongs {
    fn from(page: Paged<Song>) -> Self {
        Self {
            items: page.items.iter().map(SongView::from).collect(),
            next_cursor: page.next_cursor.map(|c| c.to_string()),
            is_last: page.is_last,
        }
    }
}

/// A playlist as the UI sees it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaylistView {
    pub id: String,
    pub name: String,
    pub member_count: usize,
}

impl From<(PlaylistId, String, usize)> for PlaylistView {
    fn from((id, name, count): (PlaylistId, String, usize)) -> Self {
        Self {
            id: id.to_string(),
            name,
            member_count: count,
        }
    }
}

/// Theme preference (coral is the design's default).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ThemeDto {
    Coral,
    Light,
    Dark,
}

/// Close behavior preference (what the window does on close).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum CloseBehaviorDto {
    Exit,
    Background,
}

fn availability_label(availability: SongAvailability) -> String {
    match availability {
        SongAvailability::Available => "available".to_owned(),
        SongAvailability::Missing => "missing".to_owned(),
        SongAvailability::PendingDelete => "pending-delete".to_owned(),
    }
}

/// A cursor value as an opaque string; the repository re-validates it.
#[must_use]
pub fn cursor_string(cursor: &OpaqueCursor) -> String {
    cursor.to_string()
}

/// A validated song id reference for commands.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SongIdRef {
    pub id: SongId,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use echo_core::domain::ids::{LibraryRootId, RelativeMediaPath, Revision};

    fn song() -> Song {
        let mut song = Song::new(
            SongId::new(),
            LibraryRootId::new(),
            RelativeMediaPath::new("周杰伦/晴天.flac").expect("path"),
            Revision::INITIAL,
        );
        song.set_favorite(true);
        song.record_play();
        song.apply_metadata(
            Some("晴天".to_owned()),
            Some("周杰伦".to_owned()),
            Some("叶惠美".to_owned()),
            Some(Duration::from_secs(239)),
        );
        song
    }

    #[test]
    fn song_view_is_camel_case_relative_path_only() {
        let view = SongView::from(&song());
        let json = serde_json::to_value(&view).expect("serialize");
        assert_eq!(view.relative_path, "周杰伦/晴天.flac");
        assert!(
            !view.relative_path.starts_with('/'),
            "no absolute path crosses the boundary"
        );
        // camelCase field names.
        assert!(json.get("relativePath").is_some());
        assert!(json.get("relative_path").is_none());
        assert!(json.get("playCount").is_some());
        assert_eq!(
            json.get("availability").and_then(|v| v.as_str()),
            Some("available")
        );
    }

    #[test]
    fn paged_songs_map_item_views_and_cursor() {
        let page = Paged::new(vec![song()], None, true);
        let view: PagedSongs = page.into();
        assert!(view.is_last);
        assert_eq!(view.items.len(), 1);
        assert_eq!(view.next_cursor, None);
    }
}
