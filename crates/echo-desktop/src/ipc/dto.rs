//! IPC serde DTOs (task 7.2, design §10).
//!
//! These are the camelCase read models the frontend consumes. Core domain
//! entities are mapped *here*; entities never derive Tauri/TypeScript traits
//! directly. `RelativeMediaPath` is the only path form that crosses the
//! boundary — never an absolute path.

use serde::{Deserialize, Serialize};

use echo_core::application::scan::ScanSummary;
use echo_core::domain::catalog::{CatalogCounts, OpaqueCursor, Paged};
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

/// The outcome of choosing a library root (task 7.5 `choose_library_root`).
/// Reaches the UI as a path-free snapshot; the absolute directory was consumed
/// entirely desktop-side.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryRootStatusDto {
    /// Whether a library root is now configured.
    pub configured: bool,
    /// True when the chosen root activated read-only (writes disabled).
    pub read_only: bool,
    /// The active root id (never an absolute path).
    pub active_root: String,
}

/// The library's read-only availability + write capability (task 7.3
/// `library_status`). It surfaces whether reads/writes are safe and whether a
/// scan is in flight, without ever carrying an absolute path.
///
/// The four booleans are independent availability flags — collapsing them
/// would obscure which capability is missing, so the natural shape wins.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_excessive_bools)]
pub struct LibraryStatus {
    /// Whether a library root is configured.
    pub configured: bool,
    /// The active root is read-only (writes like import/delete are disabled).
    pub read_only: bool,
    /// The active root is currently unreachable/missing.
    pub unavailable: bool,
    /// A scan is in flight for the active root.
    pub scanning: bool,
    /// The active root id, if configured.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_root: Option<String>,
}

/// The terminal summary of one scan run (task 7.3 `start_scan`).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ScanSnapshot {
    pub generation: u64,
    pub cancelled: bool,
    pub state: String,
    pub discovered: u64,
    pub processed: u64,
    pub created: u64,
    pub updated: u64,
    pub missing: u64,
    pub skipped: u64,
    pub failed: u64,
}

impl From<&ScanSummary> for ScanSnapshot {
    fn from(summary: &ScanSummary) -> Self {
        let progress = summary.progress;
        Self {
            generation: summary.generation,
            cancelled: summary.cancelled,
            state: format!("{:?}", progress.state),
            discovered: progress.discovered,
            processed: progress.processed,
            created: progress.created,
            updated: progress.updated,
            missing: progress.missing,
            skipped: progress.skipped,
            failed: progress.failed,
        }
    }
}

/// A song's read-only detail view (task 7.3 `get_song_detail`).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SongDetailView {
    pub song_id: String,
    /// Library-relative path only.
    pub relative_path: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_s: Option<u64>,
    pub format: Option<String>,
    pub play_count: u64,
    pub favorite: bool,
    pub has_cover: bool,
    pub lyrics: String,
    pub availability: String,
}

impl From<&echo_core::application::detail::SongDetail> for SongDetailView {
    fn from(detail: &echo_core::application::detail::SongDetail) -> Self {
        Self {
            song_id: detail.song_id.clone(),
            relative_path: detail.relative_path.clone(),
            title: detail.title.clone(),
            artist: detail.artist.clone(),
            album: detail.album.clone(),
            duration_s: detail.duration_s,
            format: detail.format.clone(),
            play_count: detail.play_count,
            favorite: detail.favorite,
            has_cover: detail.has_cover,
            lyrics: format!("{:?}", detail.lyrics),
            availability: detail.availability.clone(),
        }
    }
}

/// Per-view song totals for the navigation sidebar.
///
/// The UI needs these before a view is ever opened, so they are a standalone
/// read rather than something derived from a paged query — see
/// `CatalogQueryRepository::counts`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LibraryCountsDto {
    pub all: u64,
    pub favorites: u64,
    /// Capped at the 最近添加 view's own ceiling (100): the count must not
    /// promise more songs than the view renders.
    pub recent: u64,
}

impl From<CatalogCounts> for LibraryCountsDto {
    fn from(counts: CatalogCounts) -> Self {
        Self {
            all: counts.all as u64,
            favorites: counts.favorites as u64,
            recent: counts.recent as u64,
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

/// One input's import result as the UI sees it (task 7.5). Carries only
/// relative paths; error variants carry user-safe codes + messages (Core
/// redacts absolute locations before they reach this layer).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "kind")]
pub enum ImportResultDto {
    /// Copied, verified, published and committed under the reserved identity.
    Imported {
        operation_id: String,
        song_id: String,
        /// Library-relative published path (never absolute).
        relative_path: String,
    },
    /// The content already belongs to a library record; no copy was made.
    Duplicate { existing_song_id: String },
    /// Not an importable audio type.
    Unsupported,
    /// The root could not accept writes; the whole batch was refused.
    LibraryUnavailable,
    /// This input failed; `code` is a stable machine code.
    Failed { code: String, message: String },
}

impl From<echo_core::application::import::ImportOutcome> for ImportResultDto {
    fn from(outcome: echo_core::application::import::ImportOutcome) -> Self {
        use echo_core::application::import::ImportOutcome as O;
        match outcome {
            O::Imported {
                operation,
                song,
                target,
                ..
            } => Self::Imported {
                operation_id: operation.to_string(),
                song_id: song.to_string(),
                relative_path: target.to_string(),
            },
            O::Duplicate { existing } => Self::Duplicate {
                existing_song_id: existing.to_string(),
            },
            O::Unsupported => Self::Unsupported,
            O::LibraryUnavailable => Self::LibraryUnavailable,
            O::Failed { code, message } => Self::Failed {
                code: code.to_owned(),
                message,
            },
        }
    }
}

/// The per-input results of one import dialog batch (task 7.5), index-aligned
/// with the chosen inputs. When the user cancelled the dialog the command
/// returns no batch at all — never an empty success.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ImportBatchDto {
    pub results: Vec<ImportResultDto>,
}

impl From<echo_core::application::import::ImportBatchReport> for ImportBatchDto {
    fn from(report: echo_core::application::import::ImportBatchReport) -> Self {
        Self {
            results: report
                .results
                .into_iter()
                .map(ImportResultDto::from)
                .collect(),
        }
    }
}

/// The outcome of a reveal-in-folder request (task 7.5). Only the library-
/// relative path reaches the UI; the reveal side effect happened desktop-side.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RevealResultDto {
    pub song_id: String,
    /// Library-relative path of the revealed song (never absolute).
    pub relative_path: String,
    /// Whether the OS could reveal the file (a soft failure — the UI may show
    /// the relative path instead).
    pub revealed: bool,
}

/// Theme preference. The three themes are accent-color themes only — they
/// never change the pure-white music workspace surface (coral/cobalt/turquoise
/// per `docs/interface-terminology.md`; coral is the design default).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum ThemeDto {
    Coral,
    Cobalt,
    Turquoise,
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
