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

use std::time::Duration;

use crate::domain::ids::{
    LibraryRootId, PlayCount, PlaylistId, RelativeMediaPath, Revision, SongId,
};

// ---------------------------------------------------------------------------
// Song
// ---------------------------------------------------------------------------

/// Availability of a song. Echo never drops a song identity on external
/// deletion — it collapses to `Missing` and is restored by a later scan.
#[derive(Clone, Copy, Debug, Default, Eq, Ord, PartialEq, PartialOrd)]
pub enum SongAvailability {
    /// The file is present and playable.
    #[default]
    Available,
    /// Externally removed (or the root is unavailable); UUID/associations kept.
    Missing,
    /// Echo's own delete is in progress or its undo window is open; hidden
    /// from default catalog/queue views.
    PendingDelete,
}

impl SongAvailability {
    /// Whether the song may be played right now.
    #[must_use]
    pub const fn is_playable(self) -> bool {
        matches!(self, Self::Available)
    }

    /// Whether the song is visible in the default library views.
    #[must_use]
    pub const fn is_visible(self) -> bool {
        !matches!(self, Self::PendingDelete)
    }
}

/// A library media record — the central domain entity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Song {
    id: SongId,
    root: LibraryRootId,
    /// Relative (root-relative) path. Never absolute; the playable identity
    /// inside the root.
    path: RelativeMediaPath,
    availability: SongAvailability,
    /// Fields that survive external changes and re-scans.
    favorite: bool,
    play_count: PlayCount,
    /// Monotone revision for optimistic concurrency and event ordering.
    /// Assigned by the persistence layer — the entity never mutates it after
    /// construction (`Song::bump` only advances the in-memory change tag).
    revision: Revision,
    /// Stable insertion ordering key. Unlike `revision`, this never changes
    /// when metadata, favorites, or playback statistics are updated.
    added_at: u64,
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    /// Seconds. `None` until the file has been probed.
    duration: Option<Duration>,
    /// Hash of the parsed metadata for diagnostics provenance.
    version: u64,
}

impl Song {
    /// Create a new, available song.
    #[must_use]
    pub fn new(
        id: SongId,
        root: LibraryRootId,
        path: RelativeMediaPath,
        revision: Revision,
    ) -> Self {
        Self::with_added_at(id, root, path, revision, revision.as_u64())
    }

    /// Create a new song with an explicit insertion ordering key.
    #[must_use]
    pub fn with_added_at(
        id: SongId,
        root: LibraryRootId,
        path: RelativeMediaPath,
        revision: Revision,
        added_at: u64,
    ) -> Self {
        Self {
            id,
            root,
            path,
            availability: SongAvailability::Available,
            favorite: false,
            play_count: PlayCount::default(),
            revision,
            added_at,
            title: None,
            artist: None,
            album: None,
            duration: None,
            version: 0,
        }
    }

    /// The stable identity.
    #[must_use]
    pub const fn id(&self) -> SongId {
        self.id
    }
    /// The root this song lives in.
    #[must_use]
    pub const fn root(&self) -> LibraryRootId {
        self.root
    }
    #[must_use]
    pub const fn path(&self) -> &RelativeMediaPath {
        &self.path
    }
    #[must_use]
    pub const fn availability(&self) -> SongAvailability {
        self.availability
    }
    #[must_use]
    pub const fn favorite(&self) -> bool {
        self.favorite
    }
    #[must_use]
    pub const fn play_count(&self) -> PlayCount {
        self.play_count
    }
    #[must_use]
    pub const fn revision(&self) -> Revision {
        self.revision
    }
    #[must_use]
    pub const fn added_at(&self) -> u64 {
        self.added_at
    }
    #[must_use]
    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }
    #[must_use]
    pub fn artist(&self) -> Option<&str> {
        self.artist.as_deref()
    }
    #[must_use]
    pub fn album(&self) -> Option<&str> {
        self.album.as_deref()
    }
    #[must_use]
    pub const fn duration(&self) -> Option<Duration> {
        self.duration
    }

    /// Change the relative path, keeping the identity and associations.
    ///
    /// # Invariant
    ///
    /// Re-linking (a hash-identical file reappearing at a new path) must not
    /// reset favorite, play count, or revision ordering — only the path and
    /// the availability change.
    pub fn relink(&mut self, path: RelativeMediaPath) {
        if self.path != path {
            self.path = path;
            self.bump();
        }
    }

    /// Mark the song externally missing (never drops identity or metadata).
    pub fn mark_missing(&mut self) {
        if self.availability != SongAvailability::Missing {
            self.availability = SongAvailability::Missing;
            self.bump();
        }
    }

    /// Restore an externally-missing song (file reappeared / hash matched).
    pub fn restore_available(&mut self) {
        if self.availability != SongAvailability::Available {
            self.availability = SongAvailability::Available;
            self.bump();
        }
    }

    /// Enter Echo's own delete flow (hidden but not forgotten).
    pub fn begin_pending_delete(&mut self) {
        if self.availability != SongAvailability::PendingDelete {
            self.availability = SongAvailability::PendingDelete;
            self.bump();
        }
    }

    /// Exit Echo's delete flow, e.g. undo.
    pub fn cancel_pending_delete(&mut self) {
        if self.availability != SongAvailability::Available {
            self.availability = SongAvailability::Available;
            self.bump();
        }
    }

    /// Toggle favorite, bumping the revision.
    pub fn set_favorite(&mut self, favorite: bool) {
        if self.favorite != favorite {
            self.favorite = favorite;
            self.bump();
        }
    }

    /// Apply parsed metadata. Does not clear existing values.
    pub fn apply_metadata(
        &mut self,
        title: Option<String>,
        artist: Option<String>,
        album: Option<String>,
        duration: Option<Duration>,
    ) {
        self.title = title.or_else(|| self.title.take());
        self.artist = artist.or_else(|| self.artist.take());
        self.album = album.or_else(|| self.album.take());
        if duration.is_some() {
            self.duration = duration;
        }
        self.bump();
    }

    /// Record one completed play (idempotence is enforced upstream by the
    /// `PlaybackSessionId`; the entity only increments).
    pub fn record_play(&mut self) {
        self.play_count = PlayCount::from_u64(self.play_count.as_u64() + 1);
        self.bump();
    }

    /// Rehydrate a song from the trusted local storage adapter.
    ///
    /// This is crate-visible on purpose: SQL rows must be converted back into
    /// the domain entity without exposing persistence fields to UI/application
    /// callers or simulating mutations (which would incorrectly bump version).
    #[allow(clippy::too_many_arguments)]
    #[must_use]
    pub(crate) const fn from_storage(
        id: SongId,
        root: LibraryRootId,
        path: RelativeMediaPath,
        availability: SongAvailability,
        favorite: bool,
        play_count: PlayCount,
        revision: Revision,
        added_at: u64,
        title: Option<String>,
        artist: Option<String>,
        album: Option<String>,
        duration: Option<Duration>,
        version: u64,
    ) -> Self {
        Self {
            id,
            root,
            path,
            availability,
            favorite,
            play_count,
            revision,
            added_at,
            title,
            artist,
            album,
            duration,
            version,
        }
    }

    /// Internal: advance the revision on any meaningful change.
    fn bump(&mut self) {
        self.version = self.version.wrapping_add(1);
        // Optimization: sorting/events use the version as a cheap change tag.
        // The revision proper is assigned by persistence.
    }

    /// The change-generation tag (used by sorting/event caches).
    #[must_use]
    pub const fn version(&self) -> u64 {
        self.version
    }
}

// ---------------------------------------------------------------------------
// LibraryRoot
// ---------------------------------------------------------------------------

/// A chosen library root directory.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LibraryRoot {
    id: LibraryRootId,
    /// Absolute path on this machine — kept in the entity as a raw field but
    /// **never** logged or serialized outward. Cross-team contracts use
    /// `RelativeMediaPath` only.
    absolute_path: std::path::PathBuf,
    is_active: bool,
    write_capable: bool,
    availability: RootAvailability,
}

/// Whether the root directory is currently reachable & readable.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RootAvailability {
    #[default]
    Available,
    /// The root is missing, unmounted, or unreadable. Records are kept.
    Unavailable,
}

impl LibraryRoot {
    #[must_use]
    pub const fn new(
        id: LibraryRootId,
        absolute_path: std::path::PathBuf,
        is_active: bool,
        write_capable: bool,
    ) -> Self {
        Self {
            id,
            absolute_path,
            is_active,
            write_capable,
            availability: RootAvailability::Available,
        }
    }

    #[must_use]
    pub const fn id(&self) -> LibraryRootId {
        self.id
    }
    #[must_use]
    pub const fn is_active(&self) -> bool {
        self.is_active
    }
    #[must_use]
    pub const fn write_capable(&self) -> bool {
        self.write_capable
    }
    #[must_use]
    pub const fn availability(&self) -> RootAvailability {
        self.availability
    }
    /// Absolute path accessor — for the desktop boundary only; never into
    /// logs, IPC DTOs or sync payloads.
    #[must_use]
    pub fn absolute_path(&self) -> &std::path::Path {
        &self.absolute_path
    }

    /// Set write capability (derived from permissions + ownership marker).
    pub fn set_write_capable(&mut self, capable: bool) {
        self.write_capable = capable;
    }

    /// Mark the root available/unavailable, keeping records intact.
    pub fn set_availability(&mut self, availability: RootAvailability) {
        self.availability = availability;
    }
}

// ---------------------------------------------------------------------------
// PlaylistMember
// ---------------------------------------------------------------------------

/// A playlist membership row.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct PlaylistMember {
    playlist: PlaylistId,
    song: SongId,
    position: u64,
    song_availability: SongAvailability,
}

impl PlaylistMember {
    #[must_use]
    pub const fn new(
        playlist: PlaylistId,
        song: SongId,
        position: u64,
        song_availability: SongAvailability,
    ) -> Self {
        Self {
            playlist,
            song,
            position,
            song_availability,
        }
    }

    #[must_use]
    pub const fn playlist(&self) -> PlaylistId {
        self.playlist
    }
    #[must_use]
    pub const fn song(&self) -> SongId {
        self.song
    }
    #[must_use]
    pub const fn position(&self) -> u64 {
        self.position
    }
    #[must_use]
    pub const fn song_availability(&self) -> SongAvailability {
        self.song_availability
    }

    /// Mirror the song's availability (stale by at most one reconcile tick).
    pub fn mirror_song(&mut self, availability: SongAvailability) {
        self.song_availability = availability;
    }
}

// ---------------------------------------------------------------------------
// Lyrics
// ---------------------------------------------------------------------------

/// Priority of a lyrics source. Total order: `Override > Embedded > Sidecar`.
///
/// The override semantics are subtle: an override that exists but is empty is
/// a *user clearing*, which must NOT fall through to a lower-priority source.
///
/// `Ord` is implemented manually because the derived variant order would be
/// the declaration order (Sidecar first), the opposite of the priority meaning.
impl Ord for LyricsSource {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.priority().cmp(&other.priority())
    }
}

impl PartialOrd for LyricsSource {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl LyricsSource {
    /// Numeric priority: `Override` = 3, `Embedded` = 2, `Sidecar` = 1.
    #[must_use]
    const fn priority(self) -> u8 {
        match self {
            Self::Override => 3,
            Self::Embedded => 2,
            Self::Sidecar => 1,
        }
    }
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum LyricsSource {
    /// The user-edited Echo override layer.
    Override,
    /// Embedded in the audio file (e.g. USLT/lyrics tag).
    #[default]
    Embedded,
    /// A same-basename `.lrc` sidecar in the file's directory.
    Sidecar,
}

/// A parsed lyrics candidate from one source.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LyricsCandidate {
    source: LyricsSource,
    /// Timestamped lines, sorted by time. Empty for plain-text lyrics.
    lines: Vec<LyricsLine>,
    /// Original source text, retained for diagnostics and plain-text display.
    raw_text: String,
    /// Plain text content when no usable timestamps were found.
    plain_text: Option<String>,
    /// A non-fatal parse diagnostic. A malformed candidate must not prevent
    /// the audio record from being imported.
    parse_error: Option<String>,
    /// Whether the text is deliberately empty (a user cleared it). When set,
    /// lower-priority sources must NOT be considered.
    empty_override: bool,
}

/// One timestamped lyrics line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LyricsLine {
    /// Time in milliseconds.
    pub timestamp_ms: i64,
    pub text: String,
    /// Position in the source before timestamp sorting.
    pub original_index: usize,
}

impl LyricsCandidate {
    #[must_use]
    pub fn new(source: LyricsSource, lines: Vec<LyricsLine>, plain_text: bool) -> Self {
        let plain_text = plain_text.then(|| {
            lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>()
                .join("\n")
        });
        Self::with_raw_text(source, String::new(), lines, plain_text, None)
    }

    /// Construct a candidate while retaining the parser input and any
    /// recoverable parse diagnostic.
    #[must_use]
    pub const fn with_raw_text(
        source: LyricsSource,
        raw_text: String,
        lines: Vec<LyricsLine>,
        plain_text: Option<String>,
        parse_error: Option<String>,
    ) -> Self {
        Self {
            source,
            lines,
            raw_text,
            plain_text,
            parse_error,
            empty_override: false,
        }
    }

    #[must_use]
    pub const fn source(&self) -> LyricsSource {
        self.source
    }
    #[must_use]
    pub fn lines(&self) -> &[LyricsLine] {
        &self.lines
    }
    #[must_use]
    pub const fn is_plain_text(&self) -> bool {
        self.plain_text.is_some()
    }
    #[must_use]
    pub fn raw_text(&self) -> &str {
        &self.raw_text
    }
    #[must_use]
    pub fn plain_text(&self) -> Option<&str> {
        self.plain_text.as_deref()
    }
    #[must_use]
    pub fn parse_error(&self) -> Option<&str> {
        self.parse_error.as_deref()
    }
    #[must_use]
    pub const fn is_empty_override(&self) -> bool {
        self.empty_override
    }

    /// Mark this candidate as a deliberate empty override (user cleared the
    /// lyrics); blocks fallback.
    pub fn mark_empty_override(&mut self) {
        self.empty_override = true;
    }
}

// ---------------------------------------------------------------------------
// Media diagnostic
// ---------------------------------------------------------------------------

/// Per-file diagnostic produced by scan/import.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MediaDiagnostic {
    path: RelativeMediaPath,
    /// Stable code (see the error classification for strings).
    code: &'static str,
    reason: String,
    /// Song may not have been created (e.g. probe failure).
    song_created: bool,
}

impl MediaDiagnostic {
    #[must_use]
    pub const fn new(
        path: RelativeMediaPath,
        code: &'static str,
        reason: String,
        song_created: bool,
    ) -> Self {
        Self {
            path,
            code,
            reason,
            song_created,
        }
    }

    #[must_use]
    pub const fn path(&self) -> &RelativeMediaPath {
        &self.path
    }
    #[must_use]
    pub const fn code(&self) -> &'static str {
        self.code
    }
    #[must_use]
    pub fn reason(&self) -> &str {
        &self.reason
    }
    #[must_use]
    pub const fn song_created(&self) -> bool {
        self.song_created
    }
}

#[cfg(test)]
mod tests {
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
}
