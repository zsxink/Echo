//! Song entity and availability.

use std::time::Duration;

use crate::domain::ids::{LibraryRootId, PlayCount, RelativeMediaPath, Revision, SongId};
use crate::domain::media::{AudioFormat, AudioParameters};

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
    // Scan bookkeeping (phase 4). These are persisted per-song facts the scan
    // pipeline reads back for the size/mtime fast-skip and hash re-linking;
    // they are presentation-neutral and never change user-owned state.
    /// Full-file BLAKE3 hash (hex), `None` until first hashed.
    blake3_hash: Option<String>,
    /// File size in bytes at the last scan that processed the file.
    file_size: Option<u64>,
    /// File mtime in nanoseconds since the epoch at the last scan.
    file_mtime_ns: Option<i64>,
    /// Format family from the media probe.
    format: Option<AudioFormat>,
    /// Audio stream parameters (bitrate / sample rate / channels / bits per
    /// sample) from the metadata reader at scan time (task 6.4). Default-empty
    /// for imported seeds and rows written before migration 0004.
    audio_parameters: AudioParameters,
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
            blake3_hash: None,
            file_size: None,
            file_mtime_ns: None,
            format: None,
            audio_parameters: AudioParameters::default(),
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
    /// Full-file BLAKE3 hash (hex) from the last scan that processed the file.
    #[must_use]
    pub fn blake3_hash(&self) -> Option<&str> {
        self.blake3_hash.as_deref()
    }
    /// File size in bytes at the last processing scan.
    #[must_use]
    pub const fn file_size(&self) -> Option<u64> {
        self.file_size
    }
    /// File mtime (ns since the epoch) at the last processing scan.
    #[must_use]
    pub const fn file_mtime_ns(&self) -> Option<i64> {
        self.file_mtime_ns
    }
    /// Format family from the media probe.
    #[must_use]
    pub const fn format(&self) -> Option<AudioFormat> {
        self.format
    }
    /// Audio stream parameters read at scan time (task 6.4).
    #[must_use]
    pub const fn audio_parameters(&self) -> AudioParameters {
        self.audio_parameters
    }

    /// Record the scan facts of one processed file (hash, size, mtime, format).
    ///
    /// This is the only way scan bookkeeping changes; it never touches
    /// favorite, play count or availability.
    pub fn apply_scan_facts(
        &mut self,
        blake3_hash: String,
        file_size: u64,
        file_mtime_ns: i64,
        format: AudioFormat,
    ) {
        self.apply_scan_facts_with_params(
            blake3_hash,
            file_size,
            file_mtime_ns,
            format,
            AudioParameters::default(),
        );
    }

    /// Same as [`Self::apply_scan_facts`], additionally persisting the audio
    /// stream parameters read by the metadata probe at scan time (task 6.4).
    /// The params live on the committed row so the read-only detail DTO never
    /// re-probes the file.
    pub fn apply_scan_facts_with_params(
        &mut self,
        blake3_hash: String,
        file_size: u64,
        file_mtime_ns: i64,
        format: AudioFormat,
        audio_parameters: AudioParameters,
    ) {
        self.blake3_hash = Some(blake3_hash);
        self.file_size = Some(file_size);
        self.file_mtime_ns = Some(file_mtime_ns);
        self.format = Some(format);
        self.audio_parameters = audio_parameters;
        self.bump();
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
    pub const fn set_favorite(&mut self, favorite: bool) {
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
    pub const fn record_play(&mut self) {
        self.play_count = PlayCount::from_u64(self.play_count.as_u64() + 1);
        self.bump();
    }

    /// Restore a play count merged from portable records.
    ///
    /// The counter is monotone: a projection can only ever raise it, so a
    /// library restored from `echo/records/play-stats/` never loses plays this
    /// device recorded but that another device had not yet merged. Repeated
    /// projections are therefore idempotent (same value, no double counting).
    pub const fn restore_play_count(&mut self, count: PlayCount) {
        if count.as_u64() > self.play_count.as_u64() {
            self.play_count = count;
            self.bump();
        }
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
        blake3_hash: Option<String>,
        file_size: Option<u64>,
        file_mtime_ns: Option<i64>,
        format: Option<AudioFormat>,
        audio_parameters: AudioParameters,
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
            blake3_hash,
            file_size,
            file_mtime_ns,
            format,
            audio_parameters,
        }
    }

    /// Internal: advance the revision on any meaningful change.
    const fn bump(&mut self) {
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
