//! DTO serialization tests (split from `dto.rs` to keep both files under the
//! 1000-line scale gate; `CODE_STANDARDS` §2.1).

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
fn song_view_exposes_grouping_keys_only_for_real_metadata() {
    let view = SongView::from(&song());
    let json = serde_json::to_value(&view).expect("serialize");
    assert_eq!(
        json.get("artistKey").and_then(|value| value.as_str()),
        Some("周杰伦")
    );
    assert_eq!(
        json.get("albumKey").and_then(|value| value.as_str()),
        Some("叶惠美")
    );

    let untagged = Song::new(
        SongId::new(),
        LibraryRootId::new(),
        RelativeMediaPath::new("untagged.mp3").expect("path"),
        Revision::INITIAL,
    );
    let json = serde_json::to_value(SongView::from(&untagged)).expect("serialize");
    assert!(json.get("artistKey").is_none());
    assert!(json.get("albumKey").is_none());
}

#[test]
fn song_detail_view_carries_grouping_keys_and_omits_missing_ones() {
    use echo_core::application::detail::{LyricsAvailability, SongAudioParams, SongDetail};

    let detail = SongDetail {
        song_id: "song-1".to_owned(),
        relative_path: "周杰伦/晴天.flac".to_owned(),
        title: Some("晴天".to_owned()),
        artist: Some("周杰伦".to_owned()),
        album: Some("叶惠美".to_owned()),
        artist_key: Some("周杰伦".to_owned()),
        album_key: Some("叶惠美".to_owned()),
        duration_s: Some(239),
        format: Some("flac".to_owned()),
        audio: SongAudioParams {
            bitrate_bps: None,
            sample_rate_hz: None,
            channels: None,
            bits_per_sample: None,
        },
        loudness: None,
        play_count: 0,
        favorite: false,
        added_at: 0,
        has_cover: false,
        lyrics: LyricsAvailability::None,
        availability: "available".to_owned(),
    };
    let json = serde_json::to_value(SongDetailView::from(&detail)).expect("serialize");
    assert_eq!(
        json.get("artistKey").and_then(|value| value.as_str()),
        Some("周杰伦")
    );
    assert_eq!(
        json.get("albumKey").and_then(|value| value.as_str()),
        Some("叶惠美")
    );

    let untagged = SongDetail {
        artist_key: None,
        album_key: None,
        ..detail
    };
    let json = serde_json::to_value(SongDetailView::from(&untagged)).expect("serialize");
    assert!(json.get("artistKey").is_none());
    assert!(json.get("albumKey").is_none());
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

#[test]
fn imported_result_keeps_lyrics_failure_in_camel_case_ipc_shape() {
    use echo_core::application::import::{ImportOutcome, LyricsImportResult};
    use echo_core::domain::ids::{OperationId, SongId};

    let outcome = ImportOutcome::Imported {
        operation: OperationId::new(),
        song: SongId::new(),
        target: RelativeMediaPath::new("周杰伦/晴天.flac").expect("audio path"),
        renamed: false,
        lyrics: Box::new(LyricsImportResult::Failed {
            code: "lyrics_publish",
            message: "歌词目录不可写".to_owned(),
        }),
    };

    let json = serde_json::to_value(ImportResultDto::from(outcome)).expect("serialize DTO");
    assert_eq!(json["kind"], "imported");
    assert_eq!(json["relativePath"], "周杰伦/晴天.flac");
    assert_eq!(json["lyrics"]["kind"], "failed");
    assert_eq!(json["lyrics"]["code"], "lyrics_publish");
    assert_eq!(json["lyrics"]["message"], "歌词目录不可写");
}
