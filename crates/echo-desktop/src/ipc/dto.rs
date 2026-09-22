//! IPC serde DTOs (task 7.2, design §10).
//!
//! These are the camelCase read models the frontend consumes. Core domain
//! entities are mapped *here*; entities never derive Tauri/TypeScript traits
//! directly. `RelativeMediaPath` is the only path form that crosses the
//! boundary — never an absolute path.

use serde::{Deserialize, Serialize};

use echo_core::application::scan::ScanSummary;
use echo_core::domain::catalog::{CatalogCollection, CatalogCounts, OpaqueCursor, Paged};
use echo_core::domain::entities::{Song, SongAvailability};
use echo_core::domain::ids::{LibraryRootId, PlaylistId, SongId};
use echo_core::domain::media::{AudioFormat, AudioParameters};

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
    /// Derived sound-quality tier, when the persisted scan facts put the file
    /// in the SQ or HQ bucket. Derived at read time — never persisted, so old
    /// rows show a badge without a rescan.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quality: Option<QualityTierDto>,
}

/// One artist or album directory entry. Its keys are opaque stable identities;
/// the `WebView` may return them to the desktop but never interprets them. For
/// album entries, `album_key` identifies the merged album name and `artist_key`
/// is the representative artist of the newest member.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogCollectionView {
    pub kind: String,
    pub artist_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub album_key: Option<String>,
    pub artist: String,
    pub name: String,
    pub song_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover_key: Option<String>,
    pub has_custom_cover: bool,
}

impl CatalogCollectionView {
    pub(crate) fn from_collection(
        collection: CatalogCollection,
        cover_key: Option<String>,
        has_custom_cover: bool,
    ) -> Self {
        Self {
            kind: match collection.kind {
                echo_core::domain::catalog::CatalogCollectionKind::Artist => "artist",
                echo_core::domain::catalog::CatalogCollectionKind::Album => "album",
            }
            .to_owned(),
            artist_key: collection.artist_key,
            album_key: collection.album_key,
            artist: collection.artist,
            name: collection.name,
            song_count: collection.song_count,
            cover_key,
            has_custom_cover,
        }
    }
}

/// The badge tier shown after a song title in the library list.
///
/// Derived from the scan facts (`format` + audio parameters) the moment the
/// row is built; see [`derive_quality`]. Serialized as `"sq"` / `"hq"`.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum QualityTierDto {
    Sq,
    Hq,
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
            quality: derive_quality(song.format(), song.audio_parameters()),
        }
    }
}

/// Derive the quality tier shown as a badge (SQ, then HQ) from the scan facts.
///
/// Pure read-time derivation: a plain comparison of persisted stream
/// parameters, no I/O and no DB writes. SQ wins over HQ when both match.
/// Formats outside the lossless set (e.g. `Ape`), below the code-rate
/// thresholds, or with missing parameters yield `None`.
///
/// - **SQ**: FLAC/WAV; or bits-per-sample ≥ 24; or sample rate ≥ 96 kHz.
/// - **HQ**: MP3 ≥ 320 kbps; MP4/AAC ≥ 256 kbps; Opus/Ogg ≥ 256 kbps.
///
/// Bitrates compare in bps (`320 kbps == 320_000`) to avoid unit mix-ups.
#[must_use]
fn derive_quality(format: Option<AudioFormat>, params: AudioParameters) -> Option<QualityTierDto> {
    let format = format?;
    // A damaged/unrecognized file never earns a badge.
    if format == AudioFormat::UnknownDamaged {
        return None;
    }

    let AudioParameters {
        bitrate_bps,
        sample_rate_hz,
        bits_per_sample,
        ..
    } = params;

    // SQ by container or by stream depth/rate — applies to any format family.
    let sq = matches!(format, AudioFormat::Flac | AudioFormat::Wav)
        || bits_per_sample.is_some_and(|bits| bits >= 24)
        || sample_rate_hz.is_some_and(|hz| hz >= 96_000);
    if sq {
        return Some(QualityTierDto::Sq);
    }

    // HQ by code rate, per format family. 320 kbps and 256 kbps in bps.
    let hq = match format {
        AudioFormat::Mpeg => bitrate_bps.is_some_and(|bps| bps >= 320_000),
        AudioFormat::Mp4 => bitrate_bps.is_some_and(|bps| bps >= 256_000),
        AudioFormat::Ogg | AudioFormat::Opus => bitrate_bps.is_some_and(|bps| bps >= 256_000),
        // The lossless arms already returned as SQ; damaged returned above;
        // APE never earns HQ on its own.
        AudioFormat::Flac | AudioFormat::Wav | AudioFormat::UnknownDamaged | AudioFormat::Ape => {
            false
        }
    };

    hq.then_some(QualityTierDto::Hq)
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
///
/// The booleans are independent availability flags (`read_only` = the root's
/// write capability, `manifest_healed` = this open rebuilt a missing manifest,
/// `control_plane_read_only` = the control surface refused continuation);
/// collapsing them would hide *which* capability is missing, so — as with
/// [`LibraryStatus`] — the natural shape wins.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(clippy::struct_excessive_bools)]
pub struct LibraryRootStatusDto {
    /// Whether a library root is now configured.
    pub configured: bool,
    /// True when the chosen root activated read-only (writes disabled).
    pub read_only: bool,
    /// The active root id (never an absolute path).
    pub active_root: String,
    /// Whether this open had to rebuild a missing `echo/manifest.json` from
    /// the surviving object records (never a re-scan with fresh identities).
    pub manifest_healed: bool,
    /// True when the control surface refused continuation (a manifest newer
    /// than this build): the library opened for reads only.
    pub control_plane_read_only: bool,
    /// Objects continued from `echo/records/` in this open, by kind.
    pub continued_songs: usize,
    pub continued_favorites: usize,
    pub continued_playlists: usize,
    /// Playlist memberships restored, in record order.
    pub continued_members: usize,
    /// Play counts restored from `play-stats` records.
    pub continued_play_stats: usize,
    /// Records that could not be placed into the effective view. They are kept
    /// on disk — never deleted — and reported so the loss is visible.
    pub unusable_records: usize,
}

impl LibraryRootStatusDto {
    /// A freshly configured root with nothing continued (no control surface).
    #[must_use]
    pub fn configured(active_root: LibraryRootId, read_only: bool) -> Self {
        Self {
            configured: true,
            read_only,
            active_root: active_root.to_string(),
            manifest_healed: false,
            control_plane_read_only: false,
            continued_songs: 0,
            continued_favorites: 0,
            continued_playlists: 0,
            continued_members: 0,
            continued_play_stats: 0,
            unusable_records: 0,
        }
    }
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
    /// Opaque `cover://` cache key. Missing for a new/empty playlist or when
    /// its newest member has no embedded artwork.
    pub cover_key: Option<String>,
    /// The effective automatic artwork (the newest member that has embedded
    /// artwork), available even while a manual cover is active.
    pub automatic_cover_key: Option<String>,
    /// Whether `cover_key` is a user choice rather than automatic member art.
    pub has_custom_cover: bool,
}

impl
    From<(
        PlaylistId,
        String,
        usize,
        Option<String>,
        Option<String>,
        bool,
    )> for PlaylistView
{
    fn from(
        (id, name, count, cover_key, automatic_cover_key, has_custom_cover): (
            PlaylistId,
            String,
            usize,
            Option<String>,
            Option<String>,
            bool,
        ),
    ) -> Self {
        Self {
            id: id.to_string(),
            name,
            member_count: count,
            cover_key,
            automatic_cover_key,
            has_custom_cover,
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
        /// The published name took the close-name ` (n)` numbering
        /// (spec: 重名后成功). Non-failure, presented as a rename.
        renamed: bool,
    },
    /// The content already belongs to a library record; no copy was made.
    Duplicate { existing_song_id: String },
    /// Not an importable audio file: a benign normal-skip (non-failure).
    Skipped,
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
                renamed,
                ..
            } => Self::Imported {
                operation_id: operation.to_string(),
                song_id: song.to_string(),
                relative_path: target.to_string(),
                renamed,
            },
            O::Duplicate { existing } => Self::Duplicate {
                existing_song_id: existing.to_string(),
            },
            O::Skipped => Self::Skipped,
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

    fn with_scan_facts(song: &mut Song, format: AudioFormat, params: AudioParameters) {
        song.apply_scan_facts_with_params("hash".to_owned(), 1_024, 0, format, params);
    }

    fn params(bitrate: Option<u64>, rate: Option<u32>, bits: Option<u16>) -> AudioParameters {
        AudioParameters {
            bitrate_bps: bitrate,
            sample_rate_hz: rate,
            channels: Some(2),
            bits_per_sample: bits,
        }
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
    fn song_view_serializes_quality_as_camel_case_and_omits_none() {
        let sq = SongView::from(&song());
        let json = serde_json::to_value(&sq).expect("serialize");
        // No scan facts yet → no quality at all, and the key is absent.
        assert!(json.get("quality").is_none());

        let mut damaged = song();
        with_scan_facts(
            &mut damaged,
            AudioFormat::Mpeg,
            params(Some(128_000), None, None),
        );
        let damaged_view = SongView::from(&damaged);
        assert!(serde_json::to_value(&damaged_view)
            .expect("serialize")
            .get("quality")
            .is_none());

        let mut hq = song();
        with_scan_facts(
            &mut hq,
            AudioFormat::Mpeg,
            params(Some(320_000), None, None),
        );
        let hq_view = SongView::from(&hq);
        let hq_json = serde_json::to_value(&hq_view).expect("serialize");
        assert_eq!(
            hq_json.get("quality").and_then(|v| v.as_str()),
            Some("hq"),
            "badge tier serializes as the lowercase camelCase field value"
        );
    }

    #[test]
    fn derive_quality_marks_lossless_formats_sq() {
        for format in [AudioFormat::Flac, AudioFormat::Wav] {
            // Even with no stream parameters at all, the container earns SQ.
            assert_eq!(
                derive_quality(Some(format), AudioParameters::default()),
                Some(QualityTierDto::Sq),
                "{format:?} should be SQ by format alone"
            );
        }
    }

    #[test]
    fn derive_quality_honors_bit_depth_and_sample_rate_for_sq() {
        // 24-bit at an otherwise "low" rate still lands in SQ.
        assert_eq!(
            derive_quality(
                Some(AudioFormat::Mpeg),
                params(Some(128_000), Some(44_100), Some(24)),
            ),
            Some(QualityTierDto::Sq)
        );
        // 96 kHz at 16-bit also earns SQ.
        assert_eq!(
            derive_quality(
                Some(AudioFormat::Mp4),
                params(Some(192_000), Some(96_000), Some(16)),
            ),
            Some(QualityTierDto::Sq)
        );
        // 23-bit is not SQ by depth, and 95_999 Hz is not SQ by rate.
        assert_eq!(
            derive_quality(
                Some(AudioFormat::Mpeg),
                params(Some(320_000), Some(44_100), Some(23)),
            ),
            Some(QualityTierDto::Hq),
            "23-bit keeps the MP3 in HQ, not SQ"
        );
        assert_eq!(
            derive_quality(
                Some(AudioFormat::Mpeg),
                params(Some(320_000), Some(95_999), Some(16)),
            ),
            Some(QualityTierDto::Hq),
            "95_999 Hz keeps the MP3 in HQ, not SQ"
        );
    }

    #[test]
    fn derive_quality_hq_thresholds_per_format() {
        // MP3: 319_999 bps is not HQ; 320_000 bps is.
        assert_eq!(
            derive_quality(Some(AudioFormat::Mpeg), params(Some(319_999), None, None)),
            None
        );
        assert_eq!(
            derive_quality(Some(AudioFormat::Mpeg), params(Some(320_000), None, None)),
            Some(QualityTierDto::Hq)
        );
        // MP4: 255_999 is not HQ; 256_000 is.
        assert_eq!(
            derive_quality(Some(AudioFormat::Mp4), params(Some(255_999), None, None)),
            None
        );
        assert_eq!(
            derive_quality(Some(AudioFormat::Mp4), params(Some(256_000), None, None)),
            Some(QualityTierDto::Hq)
        );
        // Opus/Ogg: 256_000 reaches HQ.
        assert_eq!(
            derive_quality(Some(AudioFormat::Opus), params(Some(256_000), None, None)),
            Some(QualityTierDto::Hq)
        );
        assert_eq!(
            derive_quality(Some(AudioFormat::Ogg), params(Some(320_000), None, None)),
            Some(QualityTierDto::Hq)
        );
    }

    #[test]
    fn derive_quality_returns_none_for_low_rate_missing_params_and_damaged() {
        // 128 kbps MP3 — the poster-child "no badge" row.
        assert_eq!(
            derive_quality(Some(AudioFormat::Mpeg), params(Some(128_000), None, None)),
            None
        );
        // Parameters entirely missing.
        assert_eq!(
            derive_quality(Some(AudioFormat::Mpeg), AudioParameters::default()),
            None
        );
        assert_eq!(derive_quality(None, AudioParameters::default()), None);
        // Damaged files never get a badge, even with a high bitrate.
        assert_eq!(
            derive_quality(
                Some(AudioFormat::UnknownDamaged),
                params(Some(320_000), None, Some(24)),
            ),
            None
        );
        // APE never earns HQ on its own.
        assert_eq!(
            derive_quality(Some(AudioFormat::Ape), params(Some(320_000), None, None)),
            None
        );
    }

    #[test]
    fn derive_quality_prefers_sq_over_hq_when_both_match() {
        // A 24-bit / 44.1 kHz FLAC satisfies both buckets; SQ wins.
        assert_eq!(
            derive_quality(
                Some(AudioFormat::Flac),
                params(Some(320_000), Some(44_100), Some(24)),
            ),
            Some(QualityTierDto::Sq)
        );
        // A high-rate MP3 would also meet the SQ-by-rate rule.
        assert_eq!(
            derive_quality(
                Some(AudioFormat::Mpeg),
                params(Some(320_000), Some(96_000), None)
            ),
            Some(QualityTierDto::Sq),
            "rate-based SQ beats the HQ code-rate check"
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
