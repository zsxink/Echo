//! Embedded loudness tags (`ReplayGain` 2.0 / R128), kept separate from the
//! general tag reader because the two encodings do not share a unit or a
//! reference loudness.
//!
//! The reader only produces *facts*: fixed-point gain/peak values plus the
//! tagging scheme. Converting them into an audible gain is the desktop rule
//! layer's job; the file is never modified and a bad tag never fails a song.

#[cfg(test)]
use lofty::tag::TagItem;
use lofty::tag::{ItemKey, Tag};

use crate::domain::media::{LoudnessSource, LoudnessTags, ParseWarning, ParseWarningKind};

/// Reject implausible tags rather than letting them become absurd gains.
const MAX_ABS_GAIN_DB: f64 = 60.0;
/// Some taggers write peaks slightly above 1.0; keep a bounded cushion.
const MAX_PEAK_AMPLITUDE: f64 = 4.0;
const R128_TRACK_KEY: &str = "R128_TRACK_GAIN";
const R128_ALBUM_KEY: &str = "R128_ALBUM_GAIN";

/// Read loudness tags from every tag container of one file.
///
/// `ReplayGain` 2.0 wins over R128 when both are present, because it also
/// carries peak information. Warnings are non-fatal diagnostics, never an
/// error: an undecodable loudness tag must not stop a scan or an import.
pub(super) fn read_all<'a>(
    tags: impl Iterator<Item = &'a Tag>,
    warnings: &mut Vec<ParseWarning>,
) -> Option<LoudnessTags> {
    let mut replaygain = None;
    let mut r128 = None;
    for tag in tags {
        if replaygain.is_none() {
            replaygain = replaygain_from_tag(tag, warnings);
        }
        if r128.is_none() {
            r128 = r128_from_tag(tag, warnings);
        }
        unsupported_from_tag(tag, warnings);
    }
    if replaygain.is_some() && r128.is_some() {
        warn(warnings, "tag:loudness:ignored_r128");
    }
    replaygain.or(r128)
}

fn replaygain_from_tag(tag: &Tag, warnings: &mut Vec<ParseWarning>) -> Option<LoudnessTags> {
    let track_gain = optional_gain(
        tag.get_string(&ItemKey::ReplayGainTrackGain),
        "tag:replaygain_track_gain",
        warnings,
    );
    let album_gain = optional_gain(
        tag.get_string(&ItemKey::ReplayGainAlbumGain),
        "tag:replaygain_album_gain",
        warnings,
    );
    let track_peak = optional_peak(
        tag.get_string(&ItemKey::ReplayGainTrackPeak),
        "tag:replaygain_track_peak",
        warnings,
    );
    let album_peak = optional_peak(
        tag.get_string(&ItemKey::ReplayGainAlbumPeak),
        "tag:replaygain_album_peak",
        warnings,
    );
    let tags = LoudnessTags::new(
        LoudnessSource::ReplayGain2,
        track_gain,
        album_gain,
        track_peak,
        album_peak,
    );
    if tags.has_gain() {
        Some(tags)
    } else {
        if track_peak.is_some() || album_peak.is_some() {
            warn(warnings, "tag:replaygain:peak_without_gain");
        }
        None
    }
}

fn r128_from_tag(tag: &Tag, warnings: &mut Vec<ParseWarning>) -> Option<LoudnessTags> {
    let mut track_gain = None;
    let mut album_gain = None;
    for item in tag.items() {
        let key = match item.key() {
            ItemKey::Unknown(key) => normalize_key(key),
            _ => continue,
        };
        let Some(text) = item.value().text() else {
            continue;
        };
        let parsed = parse_r128(text, warnings);
        match key.as_str() {
            R128_TRACK_KEY if parsed.is_some() => track_gain = parsed,
            R128_ALBUM_KEY if parsed.is_some() => album_gain = parsed,
            _ => {}
        }
    }
    let tags = LoudnessTags::new(LoudnessSource::R128, track_gain, album_gain, None, None);
    tags.has_gain().then_some(tags)
}

/// Private encodings are reported once, then treated as "no tags". Guessing a
/// conversion for `iTunNORM` (or the `OpusHead` output gain) would produce an
/// ungrounded gain value, so they are explicitly out of scope.
fn unsupported_from_tag(tag: &Tag, warnings: &mut Vec<ParseWarning>) {
    let has_itunnorm = tag.items().any(|item| {
        matches!(item.key(), ItemKey::Unknown(key) if normalize_key(key).contains("ITUNNORM"))
    });
    if has_itunnorm {
        warn(warnings, "tag:loudness:unsupported_itunnorm");
    }
}

fn optional_gain(raw: Option<&str>, field: &str, warnings: &mut Vec<ParseWarning>) -> Option<i32> {
    let raw = raw?;
    let parsed = parse_replaygain_gain(raw);
    if parsed.is_none() {
        warn(warnings, field);
    }
    parsed
}

fn optional_peak(raw: Option<&str>, field: &str, warnings: &mut Vec<ParseWarning>) -> Option<u32> {
    let raw = raw?;
    let parsed = parse_replaygain_peak(raw);
    if parsed.is_none() {
        warn(warnings, field);
    }
    parsed
}

/// `-6.54 dB` → -6540 milli-dB. The `dB` suffix is optional; a plain number
/// is accepted because several taggers omit it.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn parse_replaygain_gain(raw: &str) -> Option<i32> {
    let trimmed = raw.trim();
    let number = trimmed
        .strip_suffix("dB")
        .or_else(|| trimmed.strip_suffix("db"))
        .or_else(|| trimmed.strip_suffix("DB"))
        .unwrap_or(trimmed)
        .trim();
    let value = number.parse::<f64>().ok()?;
    if !value.is_finite() || value.abs() > MAX_ABS_GAIN_DB {
        return None;
    }
    Some((value * 1_000.0).round() as i32)
}

/// Peak amplitude → parts per million of full scale.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn parse_replaygain_peak(raw: &str) -> Option<u32> {
    let value = raw.trim().parse::<f64>().ok()?;
    if !value.is_finite() || value <= 0.0 || value > MAX_PEAK_AMPLITUDE {
        return None;
    }
    Some((value * 1_000_000.0).round() as u32)
}

/// Q7.8 fixed point → milli-dB (`raw / 256 dB`).
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn parse_r128(raw: &str, warnings: &mut Vec<ParseWarning>) -> Option<i32> {
    let value = raw.trim().parse::<i32>().ok().map(f64::from)?;
    if !value.is_finite() || value.abs() > MAX_ABS_GAIN_DB * 256.0 {
        warn(warnings, "tag:r128_gain");
        return None;
    }
    Some(((value / 256.0) * 1_000.0).round() as i32)
}

fn normalize_key(key: &str) -> String {
    key.chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '_')
        .flat_map(char::to_uppercase)
        .collect()
}

fn warn(warnings: &mut Vec<ParseWarning>, field: &str) {
    warnings.push(ParseWarning {
        kind: ParseWarningKind::LoudnessTag,
        field: field.to_owned(),
    });
}

/// A lofty tag with a `ReplayGain` value in one of its supported key maps.
#[cfg(test)]
fn tag_with(kind: lofty::tag::TagType, items: Vec<(ItemKey, &str)>) -> Tag {
    let mut tag = Tag::new(kind);
    for (key, value) in items {
        tag.insert_unchecked(TagItem::new(
            key,
            lofty::tag::ItemValue::Text(value.to_owned()),
        ));
    }
    tag
}

#[cfg(test)]
mod tests {
    use super::*;

    fn replaygain_tag() -> Tag {
        tag_with(
            lofty::tag::TagType::VorbisComments,
            vec![
                (ItemKey::ReplayGainTrackGain, "-6.54 dB"),
                (ItemKey::ReplayGainTrackPeak, "0.98765"),
                (ItemKey::ReplayGainAlbumGain, "-8.00 dB"),
                (ItemKey::ReplayGainAlbumPeak, "0.999"),
            ],
        )
    }

    #[test]
    fn replaygain_2_tags_round_trip_vorbis_mapped_keys() {
        let mut warnings = Vec::new();
        let tags = read_all(std::iter::once(&replaygain_tag()), &mut warnings)
            .expect("valid ReplayGain tags");
        assert_eq!(tags.source, LoudnessSource::ReplayGain2);
        assert_eq!(LoudnessTags::gain_db(tags.track_gain_mdb), Some(-6.54));
        assert_eq!(LoudnessTags::gain_db(tags.album_gain_mdb), Some(-8.0));
        assert_eq!(LoudnessTags::peak(tags.track_peak_ppm), Some(0.98765));
        assert_eq!(LoudnessTags::peak(tags.album_peak_ppm), Some(0.999));
        assert!(warnings.is_empty());
    }

    #[test]
    fn replaygain_2_tags_work_in_id3v2_ape_and_mp4_maps() {
        for kind in [
            lofty::tag::TagType::Id3v2,
            lofty::tag::TagType::Ape,
            lofty::tag::TagType::Mp4Ilst,
        ] {
            let tag = tag_with(kind, vec![(ItemKey::ReplayGainTrackGain, "+3.5 dB")]);
            let mut warnings = Vec::new();
            let tags = read_all(std::iter::once(&tag), &mut warnings)
                .unwrap_or_else(|| panic!("ReplayGain key not mapped for {kind:?}"));
            assert_eq!(LoudnessTags::gain_db(tags.track_gain_mdb), Some(3.5));
        }
    }

    #[test]
    fn r128_fixed_point_is_divided_by_256_and_has_no_peak() {
        let tag = tag_with(
            lofty::tag::TagType::VorbisComments,
            vec![
                (ItemKey::Unknown("R128_TRACK_GAIN".to_owned()), "-1408"),
                (ItemKey::Unknown("r128_album_gain".to_owned()), "-1792"),
            ],
        );
        let mut warnings = Vec::new();
        let tags = read_all(std::iter::once(&tag), &mut warnings).expect("valid R128 tags");
        assert_eq!(tags.source, LoudnessSource::R128);
        assert_eq!(LoudnessTags::gain_db(tags.track_gain_mdb), Some(-5.5));
        assert_eq!(LoudnessTags::gain_db(tags.album_gain_mdb), Some(-7.0));
        assert_eq!(tags.track_peak_ppm, None);
        assert_eq!(tags.album_peak_ppm, None);
        assert!(warnings.is_empty());
    }

    #[test]
    fn replaygain_wins_over_r128_and_reports_the_ignored_source() {
        let rg = replaygain_tag();
        let r128 = tag_with(
            lofty::tag::TagType::VorbisComments,
            vec![(ItemKey::Unknown("R128_TRACK_GAIN".to_owned()), "-256")],
        );
        let mut warnings = Vec::new();
        let tags = read_all([&rg, &r128].into_iter(), &mut warnings).expect("ReplayGain wins");
        assert_eq!(tags.source, LoudnessSource::ReplayGain2);
        assert!(warnings
            .iter()
            .any(|w| w.field == "tag:loudness:ignored_r128"));
    }

    #[test]
    fn malformed_values_are_dropped_without_failing_the_song() {
        let tag = tag_with(
            lofty::tag::TagType::VorbisComments,
            vec![
                (ItemKey::ReplayGainTrackGain, "not-a-number"),
                (ItemKey::ReplayGainAlbumGain, "-4.00 dB"),
                (ItemKey::ReplayGainTrackPeak, "NaN"),
            ],
        );
        let mut warnings = Vec::new();
        let tags = read_all(std::iter::once(&tag), &mut warnings).expect("album gain survives");
        assert_eq!(tags.track_gain_mdb, None);
        assert_eq!(LoudnessTags::gain_db(tags.album_gain_mdb), Some(-4.0));
        assert_eq!(tags.track_peak_ppm, None);
        assert_eq!(warnings.len(), 2);
        assert!(warnings
            .iter()
            .all(|w| w.kind == ParseWarningKind::LoudnessTag));
    }

    #[test]
    fn partial_tags_and_private_encodings_are_not_guessed() {
        let private = tag_with(
            lofty::tag::TagType::Mp4Ilst,
            vec![(
                ItemKey::Unknown("----:com.apple.iTunes:iTunNORM".to_owned()),
                "00000B2C",
            )],
        );
        let mut warnings = Vec::new();
        assert_eq!(read_all(std::iter::once(&private), &mut warnings), None);
        assert!(warnings
            .iter()
            .any(|w| w.field == "tag:loudness:unsupported_itunnorm"));

        let peak_only = tag_with(
            lofty::tag::TagType::VorbisComments,
            vec![(ItemKey::ReplayGainTrackPeak, "0.5")],
        );
        let mut warnings = Vec::new();
        assert_eq!(read_all(std::iter::once(&peak_only), &mut warnings), None);
        assert_eq!(read_all(std::iter::empty(), &mut Vec::new()), None);
    }

    #[test]
    fn out_of_range_values_are_rejected() {
        assert_eq!(parse_replaygain_gain("200 dB"), None);
        assert_eq!(parse_replaygain_gain("inf"), None);
        assert_eq!(parse_replaygain_peak("0"), None);
        assert_eq!(parse_replaygain_peak("-1"), None);
        assert_eq!(parse_r128("999999", &mut Vec::new()), None);
    }
}
