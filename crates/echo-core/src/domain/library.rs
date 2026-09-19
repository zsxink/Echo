//! Portable library layout domain types (task 1.1).
//!
//! Defines the portable, versioned library surface that makes media and logical
//! records restorable across devices without carrying `SQLite`, credentials or
//! absolute paths.  These types live in the Domain layer: they are pure value
//! objects and validators with no dependency on the file system or database.
//!
//! Layout (design §1, portable-library-layout spec):
//!
//! ```text
//! <library-root>/
//! ├── media/<artist>/<artist> - <title>.<ext>
//! └── echo/
//!     ├── manifest.json
//!     └── records/<kind>/<prefix>/<uuid>.json
//! ```
//!
//! [`ControlPath`] validates paths *within* the `echo/` control surface.
//! [`LibraryRelativePath`] validates paths within the library root (starts with
//! `media/`).  Neither type carries an absolute path, `..`, or a path that
//! would escape the root.
//!
//! [`PortableRecord`] is the top-level serializable object stored in
//! `echo/records/<kind>/<uuid-prefix>/<uuid>.json`.  Each variant holds a
//! [`RecordKind`] discriminator.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Subject};

use super::ids::Revision;

// ── Layout constants ────────────────────────────────────────────────────────

/// The media tree root inside a portable library (`media/`).
pub const MEDIA_ROOT: &str = "media";

/// The control surface root (`echo/`).
pub const CONTROL_ROOT: &str = "echo";

/// The manifest path relative to the library root (`echo/manifest.json`).
pub const MANIFEST_PATH: &str = "echo/manifest.json";

/// The records tree root (`echo/records/`).
pub const RECORDS_ROOT: &str = "echo/records";

/// The staging directory root (`echo/tmp/`), local-only and never synced.
pub const STAGING_ROOT: &str = "echo/tmp";

/// Current format version for [`LibraryManifest`].
pub const CURRENT_FORMAT_VERSION: u64 = 1;

/// Maximum depth for object record paths inside `echo/records/<kind>/` to
/// prevent pathological directory nesting.  The real layout is at most
/// `kind/prefix/uuid.json` (depth 3).
pub const MAX_RECORD_DEPTH: usize = 8;

// ── Path types ──────────────────────────────────────────────────────────────

/// A validated, normalized path within the portable `echo/` control surface.
///
/// Starts with `echo/` or `echo/<subdir>/`, never with `media/`, is always
/// relative to the library root, never contains `..`, NUL, or control
/// characters.
///
/// Paths resolve inside `echo/` but **never** inside `echo/tmp/` (staging is
/// local-only and never enters portable objects).
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct ControlPath {
    normalized: String,
}

impl ControlPath {
    /// Construct and validate a control surface path.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Validation`] when the value is not a safe, normalized
    /// control surface path (not under `echo/`, under `echo/tmp/`, absolute,
    /// escaping `..`, NUL, control characters, empty, or redundant components).
    pub fn new(value: &str) -> Result<Self, Error> {
        Self::validate(value)?;
        Ok(Self {
            normalized: normalize_slash(value),
        })
    }

    /// Validate without constructing (cheap guard for boundary checks).
    ///
    /// # Errors
    ///
    /// Same failure set as [`Self::new`].
    pub fn validate(value: &str) -> Result<(), Error> {
        validate_relative(value)?;
        let normalized = normalize_slash(value);
        if normalized.is_empty() {
            return Err(err("path is empty"));
        }
        if !normalized.starts_with("echo/") {
            return Err(err("path must start with 'echo/'"));
        }
        if normalized == "echo" {
            return Err(err("path must not be the bare 'echo' directory"));
        }
        if normalized.starts_with("echo/tmp/") || normalized == "echo/tmp" {
            return Err(err("path must not reference echo/tmp/ staging directory"));
        }
        // Control paths may not directly reference a media file.
        if normalized.starts_with("media/") {
            return Err(err("control path must not reference media/ tree"));
        }
        if normalized.len() > MAX_COMPONENT_BYTES {
            return Err(err("control path exceeds byte limit"));
        }
        // Reject `..`/`.`/empty/trailing components like any validated path.
        validate_components(&normalized, MAX_RECORD_DEPTH, MAX_COMPONENT_BYTES)?;
        Ok(())
    }

    /// The normalised, `/`-separated form.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.normalized
    }
}

impl fmt::Display for ControlPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.normalized)
    }
}

impl std::str::FromStr for ControlPath {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

/// A validated path within the library root that starts with `media/`.
///
/// This is the portable media-relative form: `media/歌手/歌手 - 晴天.flac`.
/// Unlike [`super::ids::RelativeMediaPath`] (which accepts *any* relative
/// path under the root), `LibraryRelativePath` enforces the portable `media/`
/// prefix — the only tree Echo manages for portable media.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct LibraryRelativePath {
    normalized: String,
}

impl LibraryRelativePath {
    /// Maximum number of path components.
    pub const MAX_COMPONENTS: usize = 64;
    /// Maximum bytes per component (UTF-8).
    pub const MAX_COMPONENT_BYTES: usize = 4096;

    /// Construct and validate a library-relative media path.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Validation`] when the value is not under `media/`,
    /// is absolute, escapes with `..`, contains NUL/control characters, or has
    /// empty/redundant components.
    pub fn new(value: &str) -> Result<Self, Error> {
        Self::validate(value)?;
        Ok(Self {
            normalized: normalize_slash(value),
        })
    }

    /// Validate without constructing.
    ///
    /// # Errors
    ///
    /// Same failure set as [`Self::new`].
    pub fn validate(value: &str) -> Result<(), Error> {
        validate_relative(value)?;
        let normalized = normalize_slash(value);
        if normalized.is_empty() {
            return Err(err("path is empty"));
        }
        if !normalized.starts_with("media/") {
            return Err(err("media path must start with 'media/'"));
        }
        // Reject bare "media" — must be "media/..." with content inside.
        if normalized == "media" {
            return Err(err("media path must include content under 'media/'"));
        }
        validate_components(&normalized, Self::MAX_COMPONENTS, Self::MAX_COMPONENT_BYTES)?;
        Ok(())
    }

    /// The normalised, `/`-separated form.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.normalized
    }
}

impl fmt::Display for LibraryRelativePath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.normalized)
    }
}

impl std::str::FromStr for LibraryRelativePath {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

// ── Layout validation ───────────────────────────────────────────────────────

/// Rules for a valid portable library layout.
pub trait ValidateLibraryLayout {
    /// Validate that the library root follows the portable layout convention.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Validation`] when the layout violates portable
    /// constraints (missing `media/`, orphan `echo/tmp/` as media, etc.).
    fn validate_layout_rules(root: &str) -> Result<(), Error>;
}

/// Default implementation of layout validation rules.
pub struct DefaultLayoutValidator;

impl ValidateLibraryLayout for DefaultLayoutValidator {
    fn validate_layout_rules(root: &str) -> Result<(), Error> {
        let root = normalize_slash(root);
        if root.is_empty() {
            return Ok(());
        }

        // Reject media path under echo/ control surface.
        if root.starts_with("echo/") || root == "echo" {
            return Err(err(
                "media must not be placed under the echo/ control surface",
            ));
        }

        // Reject bare echo/tmp reference.
        if root == STAGING_ROOT || root.starts_with("echo/tmp/") {
            return Err(err(
                "echo/tmp/ is a local staging directory and not part of the portable layout",
            ));
        }

        Ok(())
    }
}

// ── Manifest ────────────────────────────────────────────────────────────────

/// The library manifest persisted at `echo/manifest.json`.
///
/// Contains the library's stable identity and format metadata.  A new device
/// reads this first, verifies format compatibility, then pulls object records
/// and scans `media/`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LibraryManifest {
    /// Schema version for backward compatibility checks.
    pub format_version: u64,
    /// The stable, globally unique identity of this library.
    pub library_id: LibraryId,
    /// Application version that last wrote this manifest.
    pub written_by_app_version: String,
}

impl LibraryManifest {
    /// Create a new manifest for a freshly initialized library.
    #[must_use]
    pub fn new(library_id: LibraryId, app_version: impl Into<String>) -> Self {
        Self {
            format_version: CURRENT_FORMAT_VERSION,
            library_id,
            written_by_app_version: app_version.into(),
        }
    }

    /// Whether this manifest's format version is compatible with the current
    /// application.
    #[must_use]
    pub const fn is_compatible(&self) -> bool {
        self.format_version == CURRENT_FORMAT_VERSION
    }
}

// ── Identity newtypes ───────────────────────────────────────────────────────

/// A stable, globally unique identity for a portable library.
///
/// Derived deterministically from the library root path at initialization time
/// (UUID v5), ensuring that the same directory always yields the same
/// [`LibraryId`] across devices and restarts.
#[derive(
    Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
pub struct LibraryId(uuid::Uuid);

impl LibraryId {
    /// Create from an existing UUID.
    #[must_use]
    pub const fn from_uuid(uuid: uuid::Uuid) -> Self {
        Self(uuid)
    }

    /// The inner UUID for boundary marshalling.
    #[must_use]
    pub const fn as_uuid(self) -> uuid::Uuid {
        self.0
    }
}

impl fmt::Display for LibraryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl std::str::FromStr for LibraryId {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let uuid = uuid::Uuid::parse_str(s)
            .map_err(|_| Error::validation(Subject::Id, "LibraryId", "invalid UUID format"))?;
        Ok(Self(uuid))
    }
}

/// The identity of the device that wrote or last modified a portable object.
///
/// Persisted alongside the object's HLC and revision to enable deterministic
/// last-writer-wins conflict resolution in the sync engine (design §3-5).
///
/// Each device maintains exactly one `DeviceId`, generated randomly (UUID v4)
/// at first startup and stored in the application data directory.  It is
/// **never** regenerated and **never** carried in the `echo/` control surface
/// (it is local-only metadata).
#[derive(
    Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
pub struct DeviceId(uuid::Uuid);

impl DeviceId {
    /// Create a new random device identity.
    #[must_use]
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4())
    }

    /// Wrap an existing UUID.
    #[must_use]
    pub const fn from_uuid(uuid: uuid::Uuid) -> Self {
        Self(uuid)
    }

    /// The inner UUID.
    #[must_use]
    pub const fn as_uuid(self) -> uuid::Uuid {
        self.0
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl std::str::FromStr for DeviceId {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let uuid = uuid::Uuid::parse_str(s)
            .map_err(|_| Error::validation(Subject::Id, "DeviceId", "invalid UUID format"))?;
        Ok(Self(uuid))
    }
}

// ── Hybrid Logical Clock ────────────────────────────────────────────────────

/// A Hybrid Logical Clock (HLC) timestamp for cross-device event ordering.
///
/// Combines wall-clock seconds since epoch with a monotonic counter to ensure
/// that no two events share the same timestamp-counter pair, and that the
/// ordering is consistent with both wall-clock time and logical precedence.
///
/// HLCs are **persistent and durable**: they are written to object records on
/// each mutation, never regenerated at runtime, and never go backward.  For
/// events on the *same* device the local `revision` is authoritative; the HLC
/// is the cross-device ordering signal.
///
/// Lexicographic comparison on `(wall_secs, counter)` gives a total order
/// suitable for the LWW conflict resolution in the future sync engine (design
/// §3-5: "从本 change 起持久化 HLC 与设备 ID，未来统一 LWW 规则").
#[derive(
    Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
)]
pub struct HybridLogicalClock {
    /// Seconds since Unix epoch (never regresses within a device).
    pub wall_secs: u64,
    /// Monotonic counter that increments when the wall clock hasn't advanced.
    pub counter: u32,
}

impl HybridLogicalClock {
    /// Construct from explicit components.
    #[must_use]
    pub const fn new(wall_secs: u64, counter: u32) -> Self {
        Self { wall_secs, counter }
    }
}

/// The next HLC after `prev` observed at wall-clock second `now_wall_secs`.
///
/// The rule (HLC advance on the same device) keeps the clock monotone and never
/// regressing, so two consecutive local writes can always be ordered:
///
/// - wall time advanced → `(now_wall_secs, 0)`;
/// - otherwise the counter increments within the same wall second;
/// - and a saturated counter still advances the wall second (so the tuple
///   grows even if wall time did not move).
///
/// The loop-back case (`now_wall_secs < prev.wall_secs`, e.g. the system clock
/// stepped backwards) keeps `prev`'s wall second rather than stepping back.
#[must_use]
pub const fn next_hlc(prev: HybridLogicalClock, now_wall_secs: u64) -> HybridLogicalClock {
    if now_wall_secs > prev.wall_secs {
        HybridLogicalClock::new(now_wall_secs, 0)
    } else if prev.counter < u32::MAX {
        HybridLogicalClock::new(prev.wall_secs, prev.counter + 1)
    } else {
        HybridLogicalClock::new(prev.wall_secs.saturating_add(1), 0)
    }
}

impl fmt::Display for HybridLogicalClock {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.wall_secs, self.counter)
    }
}

impl std::str::FromStr for HybridLogicalClock {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (wall, counter) = s.split_once(':').ok_or_else(|| {
            Error::validation(Subject::Other, "HLC", "expected 'wall_secs:counter'")
        })?;
        let wall_secs = wall
            .parse::<u64>()
            .map_err(|_| Error::validation(Subject::Other, "HLC", "wall_secs must be a u64"))?;
        let counter = counter
            .parse::<u32>()
            .map_err(|_| Error::validation(Subject::Other, "HLC", "counter must be a u32"))?;
        Ok(Self { wall_secs, counter })
    }
}

// ── Portable record ─────────────────────────────────────────────────────────

/// Discriminator for the type of a portable object record.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RecordKind {
    /// A song (media file) record.
    Song,
    /// A favorite (user liked a song).
    Favorite,
    /// A playlist definition.
    Playlist,
    /// A playlist member (a song within a playlist).
    PlaylistItem,
    /// A song override (user-edited metadata or lyrics override layer).
    Override,
    /// Per-song play statistics (additive, per-device counters).
    PlayStats,
    /// A durable deletion record propagated across devices.
    Tombstone,
}

impl RecordKind {
    /// Every object kind the layout carries (excluding [`Self::Tombstone`],
    /// which is a deletion marker rather than a live object).
    ///
    /// This is the single source of truth for "which directories exist": the
    /// projection/reconciliation passes and the layout-consistency gate both
    /// enumerate it, so a new kind cannot silently skip materialization.
    pub const ALL: &'static [Self] = &[
        Self::Song,
        Self::Favorite,
        Self::Playlist,
        Self::PlaylistItem,
        Self::Override,
        Self::PlayStats,
        Self::Tombstone,
    ];

    /// The directory name under `echo/records/` for this kind.
    #[must_use]
    pub const fn dir_name(self) -> &'static str {
        match self {
            Self::Song => "songs",
            Self::Favorite => "favorites",
            Self::Playlist => "playlists",
            Self::PlaylistItem => "playlist-items",
            Self::Override => "overrides",
            Self::PlayStats => "play-stats",
            Self::Tombstone => "tombstones",
        }
    }

    /// Valid [`TraitOperation`] values for this record kind.
    #[must_use]
    pub const fn valid_operations(self) -> &'static [TraitOperation] {
        match self {
            Self::Song | Self::Favorite | Self::Override | Self::PlayStats => {
                &[TraitOperation::Upsert]
            }
            Self::Playlist | Self::PlaylistItem => {
                &[TraitOperation::Upsert, TraitOperation::Delete]
            }
            Self::Tombstone => &[TraitOperation::Tombstone],
        }
    }
}

impl std::fmt::Display for RecordKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.dir_name())
    }
}

/// Operations that can be represented in the portable object format.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraitOperation {
    /// Create or update an object (the full snapshot replaces the prior state).
    Upsert,
    /// Mark an object as deleted (propagated tombstone, not local removal).
    Delete,
    /// A tombstone record in `echo/records/tombstones/`.
    Tombstone,
}

/// Top-level portable object serialized to `echo/records/<kind>/<uuid>.json`.
///
/// The envelope carries the revision, the device that wrote it, and the HLC
/// timestamp.  The inner record is one of the variant structs that hold the
/// object-type-specific payload.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PortableRecord {
    Song(SongRecord),
    Favorite(FavoriteRecord),
    Playlist(PlaylistRecord),
    PlaylistItem(PlaylistItemRecord),
    Override(OverrideRecord),
    PlayStats(PlayStatsRecord),
    Tombstone(TombstoneRecord),
}

impl PortableRecord {
    /// The record kind discriminator.
    #[must_use]
    pub const fn kind(&self) -> RecordKind {
        match self {
            Self::Song(_) => RecordKind::Song,
            Self::Favorite(_) => RecordKind::Favorite,
            Self::Playlist(_) => RecordKind::Playlist,
            Self::PlaylistItem(_) => RecordKind::PlaylistItem,
            Self::Override(_) => RecordKind::Override,
            Self::PlayStats(_) => RecordKind::PlayStats,
            Self::Tombstone(_) => RecordKind::Tombstone,
        }
    }

    /// The object UUID shared by all variants.
    #[must_use]
    pub const fn object_uuid(&self) -> uuid::Uuid {
        match self {
            Self::Song(r) => r.song_uuid,
            Self::Favorite(r) => r.song_uuid,
            Self::Playlist(r) => r.playlist_uuid,
            Self::PlaylistItem(r) => r.item_uuid,
            Self::Override(r) => r.song_uuid,
            Self::PlayStats(r) => r.song_uuid,
            Self::Tombstone(r) => r.object_uuid,
        }
    }

    /// The revision counter (monotonic per object).
    #[must_use]
    pub const fn revision(&self) -> Revision {
        match self {
            Self::Song(r) => r.revision,
            Self::Favorite(r) => r.revision,
            Self::Playlist(r) => r.revision,
            Self::PlaylistItem(r) => r.revision,
            Self::Override(r) => r.revision,
            Self::PlayStats(r) => r.revision,
            Self::Tombstone(r) => r.revision,
        }
    }

    /// The device that wrote or last modified this record.
    #[must_use]
    pub const fn updated_by_device(&self) -> DeviceId {
        match self {
            Self::Song(r) => r.updated_by_device_id,
            Self::Favorite(r) => r.updated_by_device_id,
            Self::Playlist(r) => r.updated_by_device_id,
            Self::PlaylistItem(r) => r.updated_by_device_id,
            Self::Override(r) => r.updated_by_device_id,
            Self::PlayStats(r) => r.updated_by_device_id,
            Self::Tombstone(r) => r.updated_by_device_id,
        }
    }

    /// The HLC timestamp.
    #[must_use]
    pub const fn hlc(&self) -> HybridLogicalClock {
        match self {
            Self::Song(r) => r.hlc,
            Self::Favorite(r) => r.hlc,
            Self::Playlist(r) => r.hlc,
            Self::PlaylistItem(r) => r.hlc,
            Self::Override(r) => r.hlc,
            Self::PlayStats(r) => r.hlc,
            Self::Tombstone(r) => r.hlc,
        }
    }

    /// Whether this record is a tombstone.
    #[must_use]
    pub const fn is_tombstone(&self) -> bool {
        matches!(self, Self::Tombstone(_))
    }

    /// The owner UUID — the UUID of the "owning" entity.
    ///
    /// For songs, favorites and overrides this is the song UUID.  For playlists
    /// this is the playlist UUID.  For playlist items this is the item UUID.
    /// For tombstones this is the deleted object UUID.
    #[must_use]
    pub const fn owner_uuid(&self) -> uuid::Uuid {
        match self {
            Self::Song(r) => r.song_uuid,
            Self::Favorite(r) => r.song_uuid,
            Self::Playlist(r) => r.playlist_uuid,
            Self::PlaylistItem(r) => r.item_uuid,
            Self::Override(r) => r.song_uuid,
            Self::PlayStats(r) => r.song_uuid,
            Self::Tombstone(r) => r.object_uuid,
        }
    }

    /// The owner name (display name of the playlist, or `None` for other
    /// kinds).  Used for diagnostics and record-path construction.
    #[must_use]
    pub fn owner_name(&self) -> Option<&str> {
        match self {
            Self::Playlist(r) => Some(&r.display_name),
            _ => None,
        }
    }
}

/// A song record: the portable representation of a media file's identity and
/// metadata.
///
/// Stored at `echo/records/songs/<uuid-prefix>/<song-uuid>.json`.
///
/// The `media_path` is always a [`LibraryRelativePath`] starting with `media/`,
/// and the `content_hash` is the full-file BLAKE3 hex digest.  Neither an
/// absolute path, `SQLite` database path, credential, nor playback state is ever
/// carried in this record (design §3, portable-library-layout spec).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SongRecord {
    pub song_uuid: uuid::Uuid,
    pub revision: Revision,
    pub updated_by_device_id: DeviceId,
    pub hlc: HybridLogicalClock,
    /// Root-relative media path (e.g. `media/周杰伦/周杰伦 - 晴天.flac`).
    pub media_path: LibraryRelativePath,
    /// Full-file BLAKE3 content hash (hex).
    pub content_hash: String,
    /// Display title parsed from tags.
    pub title: Option<String>,
    /// Display artist parsed from tags.
    pub artist: Option<String>,
    /// Display album parsed from tags.
    pub album: Option<String>,
}

/// A favorite record: tracks that a user has marked as "liked".
///
/// Stored at `echo/records/favorites/<uuid-prefix>/<song-uuid>.json`.
/// The song UUID is the same as the song record's UUID — a favorite always
/// refers to an existing song.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FavoriteRecord {
    pub song_uuid: uuid::Uuid,
    pub revision: Revision,
    pub updated_by_device_id: DeviceId,
    pub hlc: HybridLogicalClock,
    pub is_favorite: bool,
}

/// A playlist record: the portable representation of a named playlist.
///
/// Stored at `echo/records/playlists/<uuid-prefix>/<playlist-uuid>.json`.
/// Members are separate [`PlaylistItemRecord`] objects, not embedded here.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PlaylistRecord {
    pub playlist_uuid: uuid::Uuid,
    pub revision: Revision,
    pub updated_by_device_id: DeviceId,
    pub hlc: HybridLogicalClock,
    /// The user-visible playlist name (display string, not a key).
    pub display_name: String,
}

/// A playlist member record: one song in a playlist.
///
/// Stored at `echo/records/playlist-items/<uuid-prefix>/<item-uuid>.json`.
/// Each member has its own stable UUID (`item_uuid`) so membership additions,
/// removals and re-ordering can be tracked independently across devices without
/// touching the entire playlist record.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PlaylistItemRecord {
    pub item_uuid: uuid::Uuid,
    pub revision: Revision,
    pub updated_by_device_id: DeviceId,
    pub hlc: HybridLogicalClock,
    /// The playlist this member belongs to.
    pub playlist_uuid: uuid::Uuid,
    /// The song this member references.
    pub song_uuid: uuid::Uuid,
    /// Append-order key (monotonically increasing within the playlist).
    pub position: u64,
}

/// An override record: user-edited metadata or lyrics that take priority over
/// the tag-extracted values.
///
/// Stored at `echo/records/overrides/<uuid-prefix>/<song-uuid>.json`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OverrideRecord {
    pub song_uuid: uuid::Uuid,
    pub revision: Revision,
    pub updated_by_device_id: DeviceId,
    pub hlc: HybridLogicalClock,
    /// Overridden title, if any.
    pub title: Option<String>,
    /// Overridden artist, if any.
    pub artist: Option<String>,
    /// Overridden album, if any.
    pub album: Option<String>,
    /// Overridden lyrics text (the user-edited override layer).
    pub lyrics_text: Option<String>,
}

/// A play-statistics record: how often one song has been played.
///
/// Stored at `echo/records/play-stats/<uuid-prefix>/<song-uuid>.json`.
///
/// The counter is **additive per device** (`by_device`) rather than a single
/// last-writer-wins number: two devices that each played the same song once
/// must merge to 2, never to 1. Writing the same device's bucket again
/// overwrites that bucket (idempotent), so replaying a write never double
/// counts. `total()` is the sum a reader projects into `play_count`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PlayStatsRecord {
    pub song_uuid: uuid::Uuid,
    pub revision: Revision,
    pub updated_by_device_id: DeviceId,
    pub hlc: HybridLogicalClock,
    /// Play counts keyed by the device that recorded them (`Σ = play_count`).
    pub by_device: BTreeMap<uuid::Uuid, u64>,
}

impl PlayStatsRecord {
    /// The merged play count (`Σ by_device`), saturating instead of wrapping.
    #[must_use]
    pub fn total(&self) -> u64 {
        self.by_device
            .values()
            .fold(0u64, |sum, count| sum.saturating_add(*count))
    }

    /// A copy with `device`'s bucket bumped to `count` (never lowered by a
    /// merge: a stale remote record must not shrink a monotone counter).
    #[must_use]
    pub fn with_device_count(mut self, device: DeviceId, count: u64) -> Self {
        let key = device.as_uuid();
        let merged =
            self.by_device.get(&key).map_or(
                count,
                |current| {
                    if count > *current {
                        count
                    } else {
                        *current
                    }
                },
            );
        self.by_device.insert(key, merged);
        self
    }

    /// A copy carrying a new version stamp (used when a merge supersedes the
    /// record that is already on disk).
    #[must_use]
    pub const fn with_revision(mut self, revision: Revision, hlc: HybridLogicalClock) -> Self {
        self.revision = revision;
        self.hlc = hlc;
        self
    }
}

/// A tombstone record: a durable deletion propagated across devices.
///
/// Stored at `echo/records/tombstones/<uuid-prefix>/<object-uuid>.json`.
/// Tombstones are never removed — they permanently record that an object was
/// deleted, so a new device that receives the tombstone never re-creates the
/// deleted entity.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct TombstoneRecord {
    /// The UUID of the deleted object.
    pub object_uuid: uuid::Uuid,
    /// The kind of the deleted object.
    pub deleted_kind: RecordKind,
    pub revision: Revision,
    pub updated_by_device_id: DeviceId,
    pub hlc: HybridLogicalClock,
}

// ── Content hashing ─────────────────────────────────────────────────────────

/// Portable-content serialization and content hashing.
pub trait PortableSerialize {
    /// Serialize the record to a canonical JSON form for storage and content
    /// hashing.  The output must be deterministic (same record → same bytes)
    /// so that the content hash is reproducible across devices.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Storage`] when the record cannot be serialized to the
    /// canonical JSON form.
    fn to_canonical_json(&self) -> Result<String, Error>;

    /// Compute the BLAKE3 content hash of the canonical JSON.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Storage`] when serialization fails.
    fn content_hash(&self) -> Result<String, Error>;
}

impl<T> PortableSerialize for T
where
    T: Serialize,
{
    fn to_canonical_json(&self) -> Result<String, Error> {
        serde_json::to_string(self).map_err(|e| Error::Storage {
            what: "record serialization".to_owned(),
            source: Box::new(e),
        })
    }

    fn content_hash(&self) -> Result<String, Error> {
        let json = self.to_canonical_json()?;
        Ok(blake3::hash(json.as_bytes()).to_hex().to_string())
    }
}

// ── Internal helpers ────────────────────────────────────────────────────────

/// Maximum total bytes for a control/library relative path (defense-in-depth).
const MAX_COMPONENT_BYTES: usize = 4096;

/// Shared validation for any relative path inside the library root.
fn validate_relative(value: &str) -> Result<(), Error> {
    if value.is_empty() {
        return Err(err("path is empty"));
    }
    if value.contains('\0') {
        return Err(err("path contains NUL byte"));
    }
    let leading_sep = matches!(value.as_bytes().first(), Some(b'/' | b'\\'));
    let drive = value.len() >= 2
        && value.as_bytes()[0].is_ascii_alphabetic()
        && value.as_bytes()[1] == b':';
    let unc = value.starts_with(r"\\") || value.starts_with("//");
    if leading_sep || drive || unc {
        return Err(err("path must be relative"));
    }
    Ok(())
}

/// Validate path components: no empty/duplicate/trailing separators, no `.`
/// or `..`, count and byte limits.
fn validate_components(
    normalized: &str,
    max_components: usize,
    max_component_bytes: usize,
) -> Result<(), Error> {
    let mut components = 0usize;
    for component in normalized.split('/') {
        if component.is_empty() {
            return Err(err("duplicate or trailing separator"));
        }
        if component == ".." {
            return Err(err("path escapes the root (..)"));
        }
        if component == "." {
            return Err(err(
                "path contains redundant '.' component; pass the canonical spelling",
            ));
        }
        components += 1;
        if components > max_components {
            return Err(err("too many path components"));
        }
        if component.len() > max_component_bytes {
            return Err(err("component exceeds byte limit"));
        }
    }
    Ok(())
}

/// Normalise `\` to `/` for cross-platform comparison.
fn normalize_slash(value: &str) -> String {
    value.replace('\\', "/")
}

fn err(reason: &str) -> Error {
    Error::validation(Subject::Path, "LibraryPath", reason)
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── ControlPath ─────────────────────────────────────────────────────

    #[test]
    fn control_path_accepts_manifest_and_records() {
        assert_eq!(
            ControlPath::new("echo/manifest.json").unwrap().as_str(),
            "echo/manifest.json"
        );
        assert!(ControlPath::new("echo/records/songs/ab/uuid.json").is_ok());
        assert!(ControlPath::new("echo/records/playlists/cd/uuid.json").is_ok());
    }

    #[test]
    fn control_path_rejects_staging() {
        assert!(ControlPath::new("echo/tmp/op-123/file.json").is_err());
        assert!(ControlPath::new("echo/tmp").is_err());
    }

    #[test]
    fn control_path_rejects_media() {
        assert!(ControlPath::new("media/artist/song.mp3").is_err());
    }

    #[test]
    fn control_path_rejects_absolute_and_escape() {
        assert!(ControlPath::new("/echo/manifest.json").is_err());
        assert!(ControlPath::new("echo/../secret.json").is_err());
        assert!(ControlPath::new("echo/./manifest.json").is_err());
    }

    #[test]
    fn control_path_rejects_bare_echo() {
        assert!(ControlPath::new("echo").is_err());
    }

    #[test]
    fn control_path_rejects_empty_and_nul() {
        assert!(ControlPath::new("").is_err());
        assert!(ControlPath::new("echo/\0secret").is_err());
    }

    #[test]
    fn control_path_display_round_trips() {
        let p = ControlPath::new("echo/records/songs/ab/uuid.json").unwrap();
        assert_eq!(p.to_string(), "echo/records/songs/ab/uuid.json");
        assert_eq!(ControlPath::new(&p.to_string()).unwrap(), p);
    }

    // ── LibraryRelativePath ─────────────────────────────────────────────

    #[test]
    fn media_path_accepts_valid_paths() {
        let p = LibraryRelativePath::new("media/周杰伦/周杰伦 - 晴天.flac").unwrap();
        assert_eq!(p.as_str(), "media/周杰伦/周杰伦 - 晴天.flac");
        assert!(LibraryRelativePath::new("media/a/b.mp3").is_ok());
    }

    #[test]
    fn media_path_rejects_non_media_starts() {
        assert!(LibraryRelativePath::new("echo/manifest.json").is_err());
        assert!(LibraryRelativePath::new("artwork/cover.jpg").is_err());
        assert!(LibraryRelativePath::new("tmp/file").is_err());
    }

    #[test]
    fn media_path_rejects_bare_media() {
        assert!(LibraryRelativePath::new("media").is_err());
    }

    #[test]
    fn media_path_rejects_absolute_escape_and_controls() {
        assert!(LibraryRelativePath::new("/media/song.mp3").is_err());
        assert!(LibraryRelativePath::new("media/../etc/passwd").is_err());
        assert!(LibraryRelativePath::new("media/\0song").is_err());
    }

    #[test]
    fn media_path_display_round_trips() {
        let p = LibraryRelativePath::new("media/artist/artist - song.flac").unwrap();
        assert_eq!(p.as_str(), "media/artist/artist - song.flac");
        assert_eq!(LibraryRelativePath::new(p.as_str()).unwrap(), p);
    }

    // ── Layout validation ───────────────────────────────────────────────

    #[test]
    fn layout_validator_accepts_media_prefix() {
        assert!(DefaultLayoutValidator::validate_layout_rules("media/周杰伦").is_ok());
    }

    #[test]
    fn layout_validator_rejects_echo_under_media() {
        assert!(DefaultLayoutValidator::validate_layout_rules("echo/").is_err());
        assert!(DefaultLayoutValidator::validate_layout_rules("echo/tmp/op").is_err());
    }

    #[test]
    fn layout_validator_accepts_empty_root() {
        assert!(DefaultLayoutValidator::validate_layout_rules("").is_ok());
    }

    // ── LibraryId ───────────────────────────────────────────────────────

    #[test]
    fn library_id_round_trips_through_string() {
        let id = LibraryId::from_uuid(uuid::Uuid::new_v4());
        let text = id.to_string();
        let parsed: LibraryId = text.parse().unwrap();
        assert_eq!(id, parsed);
    }

    #[test]
    fn library_id_rejects_invalid_uuid() {
        assert!("not-a-uuid".parse::<LibraryId>().is_err());
    }

    // ── DeviceId ────────────────────────────────────────────────────────

    #[test]
    fn device_id_new_produces_distinct_values() {
        let a = DeviceId::new();
        let b = DeviceId::new();
        assert_ne!(a, b);
    }

    #[test]
    fn device_id_round_trips_through_string() {
        let id = DeviceId::new();
        let text = id.to_string();
        let parsed: DeviceId = text.parse().unwrap();
        assert_eq!(id, parsed);
    }

    // ── HybridLogicalClock ──────────────────────────────────────────────

    #[test]
    fn hlc_ordering_is_lexicographic() {
        let earlier = HybridLogicalClock::new(1000, 0);
        let later = HybridLogicalClock::new(1001, 0);
        assert!(earlier < later);

        let same_wall_higher_counter = HybridLogicalClock::new(1000, 1);
        assert!(earlier < same_wall_higher_counter);
    }

    #[test]
    fn hlc_round_trips_through_string() {
        let hlc = HybridLogicalClock::new(1_700_000_000, 42);
        let text = hlc.to_string();
        assert_eq!(text, "1700000000:42");
        let parsed: HybridLogicalClock = text.parse().unwrap();
        assert_eq!(hlc, parsed);
    }

    #[test]
    fn hlc_rejects_malformed() {
        assert!("not-a-number:0".parse::<HybridLogicalClock>().is_err());
        assert!("1000:".parse::<HybridLogicalClock>().is_err());
        assert!("".parse::<HybridLogicalClock>().is_err());
    }

    #[test]
    fn hlc_default_is_zero() {
        assert_eq!(HybridLogicalClock::default(), HybridLogicalClock::new(0, 0));
    }

    #[test]
    fn next_hlc_advances_wall_then_counter_and_never_regresses() {
        let mut hlc = HybridLogicalClock::default();
        // Advancing wall clock resets the counter.
        hlc = next_hlc(hlc, 1_700_000_000);
        assert_eq!(hlc, HybridLogicalClock::new(1_700_000_000, 0));
        // Same wall second → counter bumps.
        let bumped = next_hlc(hlc, 1_700_000_000);
        assert_eq!(bumped, HybridLogicalClock::new(1_700_000_000, 1));
        // A later wall second again resets the counter.
        let later = next_hlc(bumped, 1_700_000_001);
        assert_eq!(later, HybridLogicalClock::new(1_700_000_001, 0));
        // A wall clock that stepped *backward* never regresses the HLC.
        let stepped_back = next_hlc(later, 1_000);
        assert!(
            stepped_back > later,
            "a backward wall step must not regress the HLC"
        );
    }

    // ── RecordKind ──────────────────────────────────────────────────────

    #[test]
    fn record_kind_dir_names_are_stable() {
        assert_eq!(RecordKind::Song.dir_name(), "songs");
        assert_eq!(RecordKind::Favorite.dir_name(), "favorites");
        assert_eq!(RecordKind::Playlist.dir_name(), "playlists");
        assert_eq!(RecordKind::PlaylistItem.dir_name(), "playlist-items");
        assert_eq!(RecordKind::Override.dir_name(), "overrides");
        assert_eq!(RecordKind::Tombstone.dir_name(), "tombstones");
    }

    #[test]
    fn record_kind_operations_match_spec() {
        assert_eq!(
            RecordKind::Song.valid_operations(),
            &[TraitOperation::Upsert]
        );
        assert_eq!(
            RecordKind::Playlist.valid_operations(),
            &[TraitOperation::Upsert, TraitOperation::Delete]
        );
        assert_eq!(
            RecordKind::Tombstone.valid_operations(),
            &[TraitOperation::Tombstone]
        );
    }

    #[test]
    fn record_kind_display_matches_dir_name() {
        assert_eq!(RecordKind::Song.to_string(), "songs");
        assert_eq!(RecordKind::Tombstone.to_string(), "tombstones");
        assert_eq!(RecordKind::PlayStats.to_string(), "play-stats");
    }

    #[test]
    fn record_kind_all_covers_every_variant_once() {
        // The projection pass and the layout gate both enumerate `ALL`; a
        // variant missing here is a kind whose records are silently dropped.
        assert_eq!(RecordKind::ALL.len(), 7);
        for kind in RecordKind::ALL {
            assert!(
                RecordKind::ALL
                    .iter()
                    .filter(|other| *other == kind)
                    .count()
                    == 1,
                "{kind} must appear exactly once"
            );
            assert!(!kind.dir_name().is_empty());
        }
    }

    #[test]
    fn play_stats_counts_merge_additively_and_idempotently() {
        let device_a = DeviceId::new();
        let device_b = DeviceId::new();
        let base = PlayStatsRecord {
            song_uuid: uuid::Uuid::new_v4(),
            revision: Revision::INITIAL,
            updated_by_device_id: device_a,
            hlc: HybridLogicalClock::new(1_700_000_000, 0),
            by_device: BTreeMap::new(),
        };
        assert_eq!(base.total(), 0);

        // Two devices each play once → the merged count is 2 (never 1: an
        // LWW counter would have dropped one device's play).
        let merged = base
            .with_device_count(device_a, 1)
            .with_device_count(device_b, 1);
        assert_eq!(merged.total(), 2);

        // Re-writing a device's bucket is idempotent, never a doubling.
        let again = merged.with_device_count(device_a, 1);
        assert_eq!(again.total(), 2, "same device re-write does not double");
        // A monotone counter is never lowered by a stale merge.
        let stale = again.clone().with_device_count(device_b, 0);
        assert_eq!(stale.total(), 2, "a lower count never shrinks the total");
        // A real new play on one device still adds.
        assert_eq!(again.with_device_count(device_b, 2).total(), 3);
    }

    #[test]
    fn play_stats_record_round_trips_through_json() {
        let device = DeviceId::new();
        let song_uuid = uuid::Uuid::new_v4();
        let record = PortableRecord::PlayStats(PlayStatsRecord {
            song_uuid,
            revision: Revision::from_u64(4),
            updated_by_device_id: device,
            hlc: HybridLogicalClock::new(1_700_000_000, 3),
            by_device: BTreeMap::from([(device.as_uuid(), 7)]),
        });
        assert_eq!(record.kind(), RecordKind::PlayStats);
        assert_eq!(record.object_uuid(), song_uuid);
        let json = record.to_canonical_json().unwrap();
        let back: PortableRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(record, back);
        if let PortableRecord::PlayStats(stats) = &back {
            assert_eq!(stats.total(), 7);
        } else {
            panic!("expected PlayStats variant");
        }
    }

    // ── PortableRecord ──────────────────────────────────────────────────

    #[test]
    fn portable_record_kind_and_uuid_for_songs() {
        let uuid = uuid::Uuid::new_v4();
        let rec = PortableRecord::Song(SongRecord {
            song_uuid: uuid,
            revision: Revision::INITIAL,
            updated_by_device_id: DeviceId::new(),
            hlc: HybridLogicalClock::default(),
            media_path: LibraryRelativePath::new("media/周杰伦/周杰伦 - 晴天.flac").unwrap(),
            content_hash: "abc123".to_owned(),
            title: Some("晴天".to_owned()),
            artist: Some("周杰伦".to_owned()),
            album: None,
        });
        assert_eq!(rec.kind(), RecordKind::Song);
        assert_eq!(rec.object_uuid(), uuid);
        assert!(!rec.is_tombstone());
        assert_eq!(rec.owner_name(), None);
    }

    #[test]
    fn portable_record_kind_and_uuid_for_playlist() {
        let pl_uuid = uuid::Uuid::new_v4();
        let rec = PortableRecord::Playlist(PlaylistRecord {
            playlist_uuid: pl_uuid,
            revision: Revision::INITIAL,
            updated_by_device_id: DeviceId::new(),
            hlc: HybridLogicalClock::default(),
            display_name: "通勤路上".to_owned(),
        });
        assert_eq!(rec.kind(), RecordKind::Playlist);
        assert_eq!(rec.object_uuid(), pl_uuid);
        assert_eq!(rec.owner_name(), Some("通勤路上"));
    }

    #[test]
    fn portable_record_tombstone_is_flagged() {
        let obj_uuid = uuid::Uuid::new_v4();
        let rec = PortableRecord::Tombstone(TombstoneRecord {
            object_uuid: obj_uuid,
            deleted_kind: RecordKind::Song,
            revision: Revision::from_u64(3),
            updated_by_device_id: DeviceId::new(),
            hlc: HybridLogicalClock::new(1000, 1),
        });
        assert!(rec.is_tombstone());
        assert_eq!(rec.kind(), RecordKind::Tombstone);
        assert_eq!(rec.object_uuid(), obj_uuid);
    }

    #[test]
    fn portable_record_favorite_tracks_song() {
        let song_uuid = uuid::Uuid::new_v4();
        let rec = PortableRecord::Favorite(FavoriteRecord {
            song_uuid,
            revision: Revision::from_u64(2),
            updated_by_device_id: DeviceId::new(),
            hlc: HybridLogicalClock::new(2000, 0),
            is_favorite: true,
        });
        assert_eq!(rec.kind(), RecordKind::Favorite);
        assert_eq!(rec.object_uuid(), song_uuid);
        assert_eq!(rec.owner_uuid(), song_uuid);
    }

    #[test]
    fn portable_record_playlist_item_references_song_and_playlist() {
        let item_uuid = uuid::Uuid::new_v4();
        let pl_uuid = uuid::Uuid::new_v4();
        let song_uuid = uuid::Uuid::new_v4();
        let rec = PortableRecord::PlaylistItem(PlaylistItemRecord {
            item_uuid,
            revision: Revision::INITIAL,
            updated_by_device_id: DeviceId::new(),
            hlc: HybridLogicalClock::default(),
            playlist_uuid: pl_uuid,
            song_uuid,
            position: 0,
        });
        assert_eq!(rec.kind(), RecordKind::PlaylistItem);
        assert_eq!(rec.object_uuid(), item_uuid);
        // playlist_uuid and song_uuid are accessible through the inner record.
        if let PortableRecord::PlaylistItem(inner) = &rec {
            assert_eq!(inner.playlist_uuid, pl_uuid);
            assert_eq!(inner.song_uuid, song_uuid);
        } else {
            panic!("expected PlaylistItem variant");
        }
    }

    #[test]
    fn portable_record_override_preserves_song_identity() {
        let song_uuid = uuid::Uuid::new_v4();
        let rec = PortableRecord::Override(OverrideRecord {
            song_uuid,
            revision: Revision::from_u64(5),
            updated_by_device_id: DeviceId::new(),
            hlc: HybridLogicalClock::new(3000, 2),
            title: Some("修改标题".to_owned()),
            artist: None,
            album: None,
            lyrics_text: Some("自定义歌词".to_owned()),
        });
        assert_eq!(rec.kind(), RecordKind::Override);
        assert_eq!(rec.object_uuid(), song_uuid);
        assert_eq!(rec.revision(), Revision::from_u64(5));
    }

    // ── Manifest ────────────────────────────────────────────────────────

    #[test]
    fn manifest_format_version_is_current() {
        let lib_id = LibraryId::from_uuid(uuid::Uuid::new_v4());
        let manifest = LibraryManifest::new(lib_id, "0.1.0");
        assert_eq!(manifest.format_version, CURRENT_FORMAT_VERSION);
        assert!(manifest.is_compatible());
    }

    #[test]
    fn manifest_incompatible_version_fails_check() {
        let manifest = LibraryManifest {
            format_version: 0,
            library_id: LibraryId::from_uuid(uuid::Uuid::new_v4()),
            written_by_app_version: "0.1.0".to_owned(),
        };
        assert!(!manifest.is_compatible());
    }

    #[test]
    fn manifest_serializes_round_trips() {
        let lib_id = LibraryId::from_uuid(uuid::Uuid::new_v4());
        let manifest = LibraryManifest::new(lib_id, "0.1.0");
        let json = serde_json::to_string(&manifest).unwrap();
        let back: LibraryManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(manifest, back);
    }

    // ── Content hashing ─────────────────────────────────────────────────

    #[test]
    fn portable_record_content_hash_is_deterministic() {
        let song_uuid = uuid::Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap();
        let dev = DeviceId::from_uuid(
            uuid::Uuid::parse_str("00000000-0000-0000-0000-000000000002").unwrap(),
        );
        let rec = SongRecord {
            song_uuid,
            revision: Revision::INITIAL,
            updated_by_device_id: dev,
            hlc: HybridLogicalClock::new(1000, 0),
            media_path: LibraryRelativePath::new("media/artist/song.flac").unwrap(),
            content_hash: "abc123".to_owned(),
            title: Some("测试".to_owned()),
            artist: None,
            album: None,
        };
        let hash1 = rec.content_hash().unwrap();
        let hash2 = rec.content_hash().unwrap();
        assert_eq!(hash1, hash2);
        assert_eq!(hash1.len(), 64, "BLAKE3 hex digest is 64 chars");

        // Changing any field changes the hash.
        let mut rec2 = rec;
        rec2.title = Some("测试二".to_owned());
        let hash3 = rec2.content_hash().unwrap();
        assert_ne!(hash1, hash3);
    }

    #[test]
    fn portable_record_json_round_trips() {
        let song_uuid = uuid::Uuid::new_v4();
        let dev = DeviceId::new();
        let rec = PortableRecord::Song(SongRecord {
            song_uuid,
            revision: Revision::from_u64(3),
            updated_by_device_id: dev,
            hlc: HybridLogicalClock::new(1_700_000_000, 1),
            media_path: LibraryRelativePath::new("media/周杰伦/周杰伦 - 晴天.flac").unwrap(),
            content_hash: "deadbeef".to_owned(),
            title: Some("晴天".to_owned()),
            artist: Some("周杰伦".to_owned()),
            album: Some("叶惠美".to_owned()),
        });
        let json = rec.to_canonical_json().unwrap();
        let back: PortableRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(rec, back);
    }

    // ── Library manifest path constant ──────────────────────────────────

    #[test]
    fn manifest_path_is_valid_control_path() {
        assert!(ControlPath::new(MANIFEST_PATH).is_ok());
    }

    #[test]
    fn records_root_is_valid_control_path_prefix() {
        assert!(ControlPath::new("echo/records").is_ok());
    }

    // ── Unicode path support ────────────────────────────────────────────

    #[test]
    fn media_path_accepts_unicode_artist_and_title() {
        let p = LibraryRelativePath::new("media/周杰伦/周杰伦 - 晴天.flac").unwrap();
        assert_eq!(p.as_str(), "media/周杰伦/周杰伦 - 晴天.flac");
    }

    #[test]
    fn control_path_accepts_unicode() {
        let p = ControlPath::new("echo/records/playlists/ab/通勤路上.json").unwrap();
        assert_eq!(p.as_str(), "echo/records/playlists/ab/通勤路上.json");
    }

    // ── Component byte limits ───────────────────────────────────────────

    #[test]
    fn media_path_respects_component_byte_limit() {
        // A very long component (> 4096 bytes) should be rejected.
        let long = "a".repeat(4097);
        let path = format!("media/{long}/song.flac");
        assert!(
            LibraryRelativePath::new(&path).is_err(),
            "should reject overlong component"
        );
    }
}
