use crate::domain::ids::{
    LibraryRootId, PlaylistId, PlaylistItemId, RelativeMediaPath, Revision, SongId,
};

use super::*;

fn song() -> Song {
    Song::new(
        SongId::new(),
        LibraryRootId::new(),
        RelativeMediaPath::new("周杰伦/晴天.flac").unwrap(),
        Revision::INITIAL,
    )
}

#[test]
fn song_availability_invariants() {
    let mut s = song();
    assert!(s.availability().is_playable());
    assert!(s.availability().is_visible());
    assert!(!s.favorite());
    assert_eq!(s.play_count().as_u64(), 0);

    s.mark_missing();
    assert!(!s.availability().is_playable());
    assert!(s.availability().is_visible(), "missing stays visible");
    // Metadata and identity survive.
    assert_eq!(s.path().display(), "周杰伦/晴天.flac");

    s.begin_pending_delete();
    assert!(!s.availability().is_visible());
    s.cancel_pending_delete();
    assert!(s.availability().is_playable());
}

#[test]
fn song_relink_keeps_identity_and_stats() {
    let mut s = song();
    s.set_favorite(true);
    s.record_play();
    s.record_play();
    let id = s.id();
    let fav = s.favorite();
    let plays = s.play_count();

    s.relink(RelativeMediaPath::new("周杰伦/收藏/晴天.flac").unwrap());
    assert_eq!(s.id(), id);
    assert_eq!(s.favorite(), fav);
    assert_eq!(s.play_count(), plays);
    assert_eq!(s.path().display(), "周杰伦/收藏/晴天.flac");
}

#[test]
fn playlist_member_mirrors_but_never_drops_identity() {
    let song_id = SongId::new();
    let playlist = PlaylistId::new();
    let mut m = PlaylistMember::new(playlist, song_id, 0, SongAvailability::Available);
    assert_eq!(m.position(), 0);
    assert_eq!(m.song(), song_id);
    // Every member has a stable, independent identity.
    assert_ne!(m.id(), PlaylistItemId::new());
    m.mirror_song(SongAvailability::Missing);
    assert_eq!(m.song_availability(), SongAvailability::Missing);
    assert_eq!(m.song(), song_id, "mirroring must not touch identity");
}

#[test]
fn lyrics_priority_is_total_order() {
    assert!(LyricsSource::Override > LyricsSource::Embedded);
    assert!(LyricsSource::Embedded > LyricsSource::Sidecar);
    assert_eq!(LyricsSource::default(), LyricsSource::Embedded);
}

#[test]
fn lyrics_candidate_kinds() {
    let timed = LyricsCandidate::new(
        LyricsSource::Embedded,
        vec![LyricsLine {
            timestamp_ms: 1000,
            text: "故事的小黄花".into(),
            original_index: 0,
        }],
        false,
    );
    assert!(!timed.is_plain_text());
    assert_eq!(timed.source(), LyricsSource::Embedded);
    assert_eq!(timed.lines()[0].text, "故事的小黄花");

    let plain = LyricsCandidate::new(LyricsSource::Sidecar, vec![], true);
    assert!(plain.is_plain_text());

    let mut cleared = LyricsCandidate::new(LyricsSource::Override, vec![], false);
    cleared.mark_empty_override();
    assert!(cleared.is_empty_override());
}

#[test]
fn lyrics_candidate_retains_raw_plain_text_and_parse_error() {
    let candidate = LyricsCandidate::with_raw_text(
        LyricsSource::Sidecar,
        "first\nsecond".into(),
        vec![],
        Some("first\nsecond".into()),
        Some("invalid timestamp".into()),
    );
    assert_eq!(candidate.raw_text(), "first\nsecond");
    assert_eq!(candidate.plain_text(), Some("first\nsecond"));
    assert_eq!(candidate.parse_error(), Some("invalid timestamp"));
}

#[test]
fn library_root_write_capability_is_independent_of_active() {
    let mut root = LibraryRoot::new(
        LibraryRootId::new(),
        "/Users/xian/Music/我的音乐".into(),
        true,
        true,
    );
    assert!(root.write_capable());
    root.set_availability(RootAvailability::Unavailable);
    assert_eq!(root.availability(), RootAvailability::Unavailable);
    root.set_write_capable(false);
    assert!(!root.write_capable());
    assert!(root.is_active());
}

#[test]
fn song_revision_bumps_on_changes() {
    let mut s = song();
    let v0 = s.version();
    s.set_favorite(true);
    assert_ne!(s.version(), v0);
}

#[test]
fn song_added_at_is_stable_when_revision_changes() {
    let mut song = Song::with_added_at(
        SongId::new(),
        LibraryRootId::new(),
        RelativeMediaPath::new("album/song.flac").expect("valid path"),
        Revision::INITIAL,
        42,
    );
    song.set_favorite(true);
    assert_eq!(song.added_at(), 42);
}

#[test]
fn song_scan_facts_apply_without_touching_user_state() {
    let mut s = song();
    s.set_favorite(true);
    s.record_play();
    s.apply_scan_facts(
        "abc123".into(),
        4096,
        1_700_000_000_000_000_000,
        crate::domain::media::AudioFormat::Flac,
    );
    assert_eq!(s.blake3_hash(), Some("abc123"));
    assert_eq!(s.file_size(), Some(4096));
    assert_eq!(s.file_mtime_ns(), Some(1_700_000_000_000_000_000));
    assert_eq!(s.format(), Some(crate::domain::media::AudioFormat::Flac));
    assert!(s.favorite(), "scan facts never touch favorite");
    assert_eq!(s.play_count().as_u64(), 1);
    assert_eq!(s.availability(), SongAvailability::Available);
}

fn timed(source: LyricsSource, error: Option<&str>) -> LyricsCandidate {
    LyricsCandidate::with_raw_text(
        source,
        String::new(),
        vec![LyricsLine {
            timestamp_ms: 500,
            text: "line".into(),
            original_index: 0,
        }],
        None,
        error.map(str::to_owned),
    )
}

#[test]
fn lyrics_selection_prefers_valid_highest_priority() {
    let sidecar = timed(LyricsSource::Sidecar, None);
    let embedded = timed(LyricsSource::Embedded, None);
    let candidates = [sidecar, embedded];
    let picked = select_effective_lyrics(&candidates).unwrap();
    assert_eq!(picked.source(), LyricsSource::Embedded);
}

#[test]
fn lyrics_selection_falls_back_when_higher_priority_is_corrupt() {
    let broken_embedded = timed(LyricsSource::Embedded, Some("bad timestamp"));
    let valid_sidecar = timed(LyricsSource::Sidecar, None);
    let candidates = [broken_embedded, valid_sidecar.clone()];
    let picked = select_effective_lyrics(&candidates).unwrap();
    assert_eq!(picked.source(), LyricsSource::Sidecar);
    assert_eq!(picked, &valid_sidecar);
}

#[test]
fn lyrics_selection_empty_override_blocks_fallback() {
    let mut cleared = LyricsCandidate::new(LyricsSource::Override, vec![], false);
    cleared.mark_empty_override();
    let valid_sidecar = timed(LyricsSource::Sidecar, None);
    let candidates = [cleared, valid_sidecar];
    let picked = select_effective_lyrics(&candidates).unwrap();
    assert!(picked.is_empty_override(), "clearing must not fall through");
}

#[test]
fn lyrics_selection_all_corrupt_means_no_lyrics() {
    let broken = timed(LyricsSource::Embedded, Some("corrupt"));
    let candidates = [broken];
    assert!(select_effective_lyrics(&candidates).is_none());
    assert!(select_effective_lyrics(&[]).is_none());
}
