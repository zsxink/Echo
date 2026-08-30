//! Read-only song detail (task 6.4, spec "查看歌曲详情").
//!
//! `GetSongDetail` aggregates the authoritative committed state of one song
//! into a single read-only DTO for the UI: effective metadata, format and
//! audio stream parameters, the library-relative path, play statistics and the
//! availability of cover art and lyrics.
//!
//! The DTO is deliberately path-safe: it never carries an absolute path. The
//! `relative_path` is a validated [`RelativeMediaPath`] (the only form that may
//! cross a trusted boundary), and the cover/lyrics are presence flags plus
//! their effective source, never file contents.

use crate::application::ports::{CoverRepository, LyricsRepository, SongRepository};
use crate::domain::entities::{LyricsSource, Song};
use crate::domain::ids::RelativeMediaPath;
use crate::domain::media::AudioParameters;
use crate::error::Error;

/// Effective source of lyrics actually shown (the strongest candidate that is
/// not corrupt or an empty override).
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum LyricsAvailability {
    None,
    Override,
    Embedded,
    Sidecar,
}

impl From<LyricsSource> for LyricsAvailability {
    fn from(source: LyricsSource) -> Self {
        match source {
            LyricsSource::Override => Self::Override,
            LyricsSource::Embedded => Self::Embedded,
            LyricsSource::Sidecar => Self::Sidecar,
        }
    }
}

/// Audio stream parameters in a serde-friendly shape (domain types do not
/// derive serde; the DTO is the IPC boundary).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SongAudioParams {
    pub bitrate_bps: Option<u64>,
    pub sample_rate_hz: Option<u32>,
    pub channels: Option<u16>,
    pub bits_per_sample: Option<u16>,
}

impl From<AudioParameters> for SongAudioParams {
    fn from(value: AudioParameters) -> Self {
        Self {
            bitrate_bps: value.bitrate_bps,
            sample_rate_hz: value.sample_rate_hz,
            channels: value.channels,
            bits_per_sample: value.bits_per_sample,
        }
    }
}

/// The read-only detail of one song, safe to serialize to the UI.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SongDetail {
    /// The stable identity every surface shares.
    pub song_id: String,
    /// Library-relative path — the only path form that crosses the boundary.
    pub relative_path: String,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    /// Duration in seconds.
    pub duration_s: Option<u64>,
    /// Format family from the probe, as its canonical extension.
    pub format: Option<String>,
    /// Audio stream parameters read at scan time (may be empty for older rows).
    pub audio: SongAudioParams,
    pub play_count: u64,
    pub favorite: bool,
    /// When the song was added (insertion order key).
    pub added_at: u64,
    /// Whether a usable cover asset exists.
    pub has_cover: bool,
    /// The effective lyrics source (or `None`).
    pub lyrics: LyricsAvailability,
    /// Human-readable availability of the file (available / missing /
    /// pending-delete), so the detail never lies about playability.
    pub availability: String,
}

/// Build the read-only detail of one song (task 6.4).
pub struct GetSongDetail<'a> {
    songs: &'a dyn SongRepository,
    lyrics: &'a dyn LyricsRepository,
    covers: &'a dyn CoverRepository,
}

impl<'a> GetSongDetail<'a> {
    #[must_use]
    pub const fn new(
        songs: &'a dyn SongRepository,
        lyrics: &'a dyn LyricsRepository,
        covers: &'a dyn CoverRepository,
    ) -> Self {
        Self {
            songs,
            lyrics,
            covers,
        }
    }

    /// Read the authoritative detail of `id`.
    ///
    /// # Errors
    ///
    /// `Unavailable` when the song is unknown; storage errors propagate.
    pub fn execute(&self, id: crate::domain::ids::SongId) -> Result<SongDetail, Error> {
        let song = self
            .songs
            .by_id(id)?
            .ok_or_else(|| Error::unavailable("song", "not in the library"))?;
        self.build(&song, id)
    }

    fn build(&self, song: &Song, id: crate::domain::ids::SongId) -> Result<SongDetail, Error> {
        let relative = song.path();
        Ok(SongDetail {
            song_id: id.to_string(),
            relative_path: relative.display().to_string(),
            title: song.title().map(ToOwned::to_owned),
            artist: song.artist().map(ToOwned::to_owned),
            album: song.album().map(ToOwned::to_owned),
            duration_s: song.duration().map(|d| d.as_secs()),
            format: song.format().map(|f| f.extension().to_owned()),
            audio: song.audio_parameters().into(),
            play_count: song.play_count().as_u64(),
            favorite: song.favorite(),
            added_at: song.added_at(),
            has_cover: self.covers.cover_of(id)?.is_some(),
            lyrics: self.effective_lyrics(id)?,
            availability: availability_label(song),
        })
    }

    /// The strongest lyrics candidate that is actually effective, or `None`.
    fn effective_lyrics(
        &self,
        id: crate::domain::ids::SongId,
    ) -> Result<LyricsAvailability, Error> {
        let candidates = self.lyrics.candidates(id)?;
        Ok(candidates
            .iter()
            .find(|c| c.is_effective())
            .map_or(LyricsAvailability::None, |c| c.source().into()))
    }
}

/// A stable, path-free, human-readable availability label.
fn availability_label(song: &Song) -> String {
    match song.availability() {
        crate::domain::entities::SongAvailability::Available => "available",
        crate::domain::entities::SongAvailability::Missing => "missing",
        crate::domain::entities::SongAvailability::PendingDelete => "pending-delete",
    }
    .to_owned()
}

/// Prove the DTO never leaks an absolute path: the only path field is the
/// validated relative one.
#[must_use]
pub fn detail_path_is_relative(detail: &SongDetail) -> bool {
    RelativeMediaPath::new(&detail.relative_path).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    use crate::application::ports::{LibraryRepository, SongRepository, TxAccess, UnitOfWork};
    use crate::application::testing::memory_database::MemoryDatabase;
    use crate::domain::entities::{LibraryRoot, SongAvailability};
    use crate::domain::ids::{LibraryRootId, Revision, SongId};

    fn seed(db: &MemoryDatabase, root: LibraryRootId) -> Song {
        let mut song = Song::new(
            SongId::new(),
            root,
            RelativeMediaPath::new("周杰伦/七里香.flac").expect("path"),
            Revision::INITIAL,
        );
        song.apply_metadata(
            Some("七里香".to_owned()),
            Some("周杰伦".to_owned()),
            Some("七里香".to_owned()),
            Some(Duration::from_secs(254)),
        );
        song.set_favorite(true);
        song.record_play();
        song.record_play();
        song.apply_scan_facts_with_params(
            "a".repeat(64),
            8_000_000,
            1,
            crate::domain::media::AudioFormat::Flac,
            crate::domain::media::AudioParameters {
                bitrate_bps: Some(900_000),
                sample_rate_hz: Some(44_100),
                channels: Some(2),
                bits_per_sample: Some(16),
            },
        );
        SongRepository::upsert(db, &song).expect("seed");
        song
    }

    #[test]
    fn detail_is_read_only_and_carries_no_absolute_path() {
        let db = MemoryDatabase::new();
        let root = LibraryRootId::new();
        LibraryRepository::upsert(&db, &LibraryRoot::new(root, ".".into(), true, true))
            .expect("active root");
        let song = seed(&db, root);
        let use_case = GetSongDetail::new(&db, &db, &db);

        let detail = use_case.execute(song.id()).expect("detail");
        assert_eq!(detail.title.as_deref(), Some("七里香"));
        assert_eq!(detail.artist.as_deref(), Some("周杰伦"));
        assert_eq!(detail.duration_s, Some(254));
        assert_eq!(detail.format.as_deref(), Some("flac"));
        assert_eq!(detail.play_count, 2);
        assert!(detail.favorite);
        assert!(!detail.relative_path.starts_with('/'), "no absolute path");
        assert!(detail_path_is_relative(&detail), "path stays relative");
        assert_eq!(detail.availability, "available");
        assert!(detail.audio.sample_rate_hz == Some(44_100));
        assert_eq!(detail.lyrics, LyricsAvailability::None, "no lyrics stored");
    }

    #[test]
    fn detail_reflects_cover_and_lyrics_availability() {
        let db = MemoryDatabase::new();
        let root = LibraryRootId::new();
        LibraryRepository::upsert(&db, &LibraryRoot::new(root, ".".into(), true, true))
            .expect("active root");
        let song = seed(&db, root);

        // Attach a lyrics candidate through the transaction surface.
        db.with_tx({
            let id = song.id();
            Box::new(move |tx: &mut dyn TxAccess| {
                tx.set_lyrics_candidate(
                    id,
                    &crate::domain::entities::LyricsCandidate::with_raw_text(
                        crate::domain::entities::LyricsSource::Embedded,
                        "\n".to_owned(),
                        vec![crate::domain::entities::LyricsLine {
                            timestamp_ms: 0,
                            text: "歌词".to_owned(),
                            original_index: 0,
                        }],
                        Some("歌词".to_owned()),
                        None,
                    ),
                )
            })
        })
        .expect("lyrics");
        // A cover reference (memory writer stores the hash-only row).
        db.with_tx({
            let id = song.id();
            Box::new(move |tx: &mut dyn TxAccess| {
                tx.attach_cover(
                    id,
                    &crate::application::ports::CoverAssetRef {
                        content_hash: "deadbeef".to_owned(),
                        mime: "image/jpeg".to_owned(),
                        asset_key: "cover-1".to_owned(),
                    },
                )
            })
        })
        .expect("cover");

        let detail = GetSongDetail::new(&db, &db, &db)
            .execute(song.id())
            .expect("detail");
        assert_eq!(detail.lyrics, LyricsAvailability::Embedded);
        assert!(detail.has_cover);
    }

    fn golden_detail() -> SongDetail {
        SongDetail {
            song_id: "01234567-89ab-cdef-0123-456789abcdef".to_owned(),
            relative_path: "周杰伦/七里香.flac".to_owned(),
            title: Some("七里香".to_owned()),
            artist: Some("周杰伦".to_owned()),
            album: Some("七里香".to_owned()),
            duration_s: Some(254),
            format: Some("flac".to_owned()),
            audio: SongAudioParams {
                bitrate_bps: Some(900_000),
                sample_rate_hz: Some(44_100),
                channels: Some(2),
                bits_per_sample: Some(16),
            },
            play_count: 12,
            favorite: true,
            added_at: 1_704_000_000,
            has_cover: true,
            lyrics: LyricsAvailability::Embedded,
            availability: "available".to_owned(),
        }
    }

    /// 序列化 golden test：DTO 输出必须稳定、路径字段只含相对路径、绝不含
    /// 绝对路径（设计 §10：完整绝对路径不得进入 `WebView`）。
    #[test]
    fn detail_serialization_golden_match() {
        let json = serde_json::to_string_pretty(&golden_detail()).expect("serialize");
        let expected = r#"{
  "song_id": "01234567-89ab-cdef-0123-456789abcdef",
  "relative_path": "周杰伦/七里香.flac",
  "title": "七里香",
  "artist": "周杰伦",
  "album": "七里香",
  "duration_s": 254,
  "format": "flac",
  "audio": {
    "bitrate_bps": 900000,
    "sample_rate_hz": 44100,
    "channels": 2,
    "bits_per_sample": 16
  },
  "play_count": 12,
  "favorite": true,
  "added_at": 1704000000,
  "has_cover": true,
  "lyrics": "Embedded",
  "availability": "available"
}"#;
        assert_eq!(json, expected, "song detail serialization must be stable");
        // The serialized relative_path is the validated relative spelling — it
        // may contain separators but must never start with an absolute root.
        let detail = golden_detail();
        assert!(
            !detail.relative_path.starts_with('/'),
            "no absolute path may serialize"
        );
        // The only path is the validated relative one.
        assert!(detail_path_is_relative(&detail));
    }

    #[test]
    fn detail_of_unknown_song_is_unavailable_and_missing_is_visible() {
        let db = MemoryDatabase::new();
        let root = LibraryRootId::new();
        LibraryRepository::upsert(&db, &LibraryRoot::new(root, ".".into(), true, true))
            .expect("active root");
        assert!(
            GetSongDetail::new(&db, &db, &db)
                .execute(SongId::new())
                .is_err(),
            "unknown song is unavailable"
        );

        let mut song = seed(&db, root);
        song.mark_missing();
        SongRepository::set_availability(&db, song.id(), SongAvailability::Missing)
            .expect("missing");
        let detail = GetSongDetail::new(&db, &db, &db)
            .execute(song.id())
            .expect("detail");
        assert_eq!(
            detail.availability, "missing",
            "missing is visible and honest"
        );
    }
}
