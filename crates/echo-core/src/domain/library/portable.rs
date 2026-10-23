//! Portable object records (v1 flat records & v2 envelopes).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::{Error, Subject};

use super::super::ids::{PlaylistId, PlaylistItemId, Revision, SongId};
use super::super::sync::{ObjectKey, ObjectVersion, SyncBinding};
use super::identity::{DeviceId, HybridLogicalClock};
use super::{LibraryRelativePath, CURRENT_FORMAT_VERSION};

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

/// Historical identity mappings retained with the canonical portable object,
/// including after its deletion. Song and member identities never share an
/// alias namespace, even when their UUID bytes happen to be equal.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum PortableAlias {
    Song {
        alias_uuid: uuid::Uuid,
        canonical_uuid: uuid::Uuid,
    },
    PlaylistItem {
        playlist_uuid: uuid::Uuid,
        alias_uuid: uuid::Uuid,
        canonical_uuid: uuid::Uuid,
    },
}

impl PortableAlias {
    /// The typed historical identity being redirected.
    #[must_use]
    pub const fn alias_key(self) -> ObjectKey {
        match self {
            Self::Song { alias_uuid, .. } => ObjectKey::Song(SongId::from_uuid(alias_uuid)),
            Self::PlaylistItem { alias_uuid, .. } => {
                ObjectKey::PlaylistItem(PlaylistItemId::from_uuid(alias_uuid))
            }
        }
    }

    /// The typed canonical identity. Its kind is fixed by the alias variant.
    #[must_use]
    pub const fn canonical_key(self) -> ObjectKey {
        match self {
            Self::Song { canonical_uuid, .. } => ObjectKey::Song(SongId::from_uuid(canonical_uuid)),
            Self::PlaylistItem { canonical_uuid, .. } => {
                ObjectKey::PlaylistItem(PlaylistItemId::from_uuid(canonical_uuid))
            }
        }
    }
}

/// A genuinely confirmed baseline. Absence of this structure means that an
/// object is local portable state, with no claimed authority or host version.
/// In particular, upgrading v1 never invents a default binding or version.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PortableSyncMetadata {
    pub binding: SyncBinding,
    pub object_version: ObjectVersion,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<PortableAlias>,
}

/// The v2 on-disk envelope, distinct from a transport mutation. The inner v1
/// payload stays source compatible with existing record constructors.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "PortableRecordEnvelopeWire")]
pub struct PortableRecordEnvelope {
    format_version: u64,
    record: PortableRecord,
    #[serde(skip_serializing_if = "Option::is_none")]
    sync: Option<PortableSyncMetadata>,
}

#[derive(Deserialize)]
struct PortableRecordEnvelopeWire {
    format_version: u64,
    record: PortableRecord,
    #[serde(default)]
    sync: Option<PortableSyncMetadata>,
}

impl TryFrom<PortableRecordEnvelopeWire> for PortableRecordEnvelope {
    type Error = Error;

    fn try_from(value: PortableRecordEnvelopeWire) -> Result<Self, Self::Error> {
        let envelope = Self {
            format_version: value.format_version,
            record: value.record,
            sync: value.sync,
        };
        envelope.validate()?;
        Ok(envelope)
    }
}

impl PortableRecordEnvelope {
    /// Create a v2 local object or a confirmed object with explicit metadata.
    ///
    /// # Errors
    /// Rejects invalid typed tombstones and inconsistent alias mappings.
    pub fn new(record: PortableRecord, sync: Option<PortableSyncMetadata>) -> Result<Self, Error> {
        let envelope = Self {
            format_version: CURRENT_FORMAT_VERSION,
            record,
            sync,
        };
        envelope.validate()?;
        Ok(envelope)
    }

    /// Validate the version and the identity relationships in the envelope.
    ///
    /// # Errors
    /// Rejects unknown versions, non-object tombstones, self aliases, conflicting
    /// alias targets, and aliases attached to another type or canonical object.
    pub fn validate(&self) -> Result<(), Error> {
        if self.format_version != 2 {
            return Err(err("portable record envelope must use format version 2"));
        }
        let object = self.record.object_key()?;
        let Some(sync) = &self.sync else {
            return Ok(());
        };
        if sync.binding.epoch.as_u64() == 0 {
            return Err(err("confirmed portable record epoch must be positive"));
        }
        let mut aliases = Vec::new();
        for alias in &sync.aliases {
            if alias.alias_key() == alias.canonical_key() {
                return Err(err("portable alias must not point to itself"));
            }
            if alias.canonical_key() != object {
                return Err(err(
                    "portable alias must target this typed canonical object",
                ));
            }
            if let (
                PortableAlias::PlaylistItem { playlist_uuid, .. },
                PortableRecord::PlaylistItem(member),
            ) = (alias, &self.record)
            {
                if *playlist_uuid != member.playlist_uuid {
                    return Err(err("portable member alias belongs to another playlist"));
                }
            }
            if aliases.contains(&alias.alias_key()) {
                return Err(err("portable alias identity is duplicated"));
            }
            aliases.push(alias.alias_key());
        }
        Ok(())
    }

    #[must_use]
    pub const fn format_version(&self) -> u64 {
        self.format_version
    }

    #[must_use]
    pub const fn record(&self) -> &PortableRecord {
        &self.record
    }

    #[must_use]
    pub const fn sync_metadata(&self) -> Option<&PortableSyncMetadata> {
        self.sync.as_ref()
    }

    #[must_use]
    pub fn into_record(self) -> PortableRecord {
        self.record
    }
}

/// Explicitly reads both historical flat v1 JSON and current v2 envelopes.
/// Writers choose `V2` only after their recoverable upgrade has completed.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(untagged)]
pub enum PortableRecordDocument {
    V1(PortableRecord),
    V2(PortableRecordEnvelope),
}

impl<'de> Deserialize<'de> for PortableRecordDocument {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        // A claimed version must never fall back to the legacy decoder, even
        // if the document also supplies fields that look like a v1 record.
        if value.get("format_version").is_some() {
            serde_json::from_value(value)
                .map(Self::V2)
                .map_err(serde::de::Error::custom)
        } else {
            serde_json::from_value(value)
                .map(Self::V1)
                .map_err(serde::de::Error::custom)
        }
    }
}

impl PortableRecordDocument {
    #[must_use]
    pub const fn record(&self) -> &PortableRecord {
        match self {
            Self::V1(record) => record,
            Self::V2(envelope) => envelope.record(),
        }
    }

    #[must_use]
    pub const fn sync_metadata(&self) -> Option<&PortableSyncMetadata> {
        match self {
            Self::V1(_) => None,
            Self::V2(envelope) => envelope.sync_metadata(),
        }
    }

    #[must_use]
    pub fn into_record(self) -> PortableRecord {
        match self {
            Self::V1(record) => record,
            Self::V2(envelope) => envelope.into_record(),
        }
    }
}

impl PortableRecord {
    /// Typed identity used for confirmation and tombstone lookup. Tombstones
    /// retain the deleted kind, so a favorite never suppresses the song sharing
    /// its UUID.
    ///
    /// # Errors
    /// Rejects a tombstone claiming to delete another tombstone.
    pub fn object_key(&self) -> Result<ObjectKey, Error> {
        let kind = match self {
            Self::Tombstone(record) => record.deleted_kind,
            _ => self.kind(),
        };
        let id = self.object_uuid();
        Ok(match kind {
            RecordKind::Song => ObjectKey::Song(SongId::from_uuid(id)),
            RecordKind::Favorite => ObjectKey::Favorite(SongId::from_uuid(id)),
            RecordKind::Playlist => ObjectKey::Playlist(PlaylistId::from_uuid(id)),
            RecordKind::PlaylistItem => ObjectKey::PlaylistItem(PlaylistItemId::from_uuid(id)),
            RecordKind::Override => ObjectKey::Override(SongId::from_uuid(id)),
            RecordKind::PlayStats => ObjectKey::PlayStats(SongId::from_uuid(id)),
            RecordKind::Tombstone => return Err(err("tombstone must delete a typed live object")),
        })
    }

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
/// The `media_path` is a safe path relative to the library root (new imports
/// use `media/`), and the `content_hash` is the full-file BLAKE3 hex digest. Neither an
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
    /// Wall-clock milliseconds when this song was added to the library
    /// (`Song::added_at`). Missing on records written before this field
    /// existed: `#[serde(default)]` reads them back as `0`, which matches the
    /// historical fallback order exactly.
    #[serde(default)]
    pub added_at: u64,
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
    /// Original creation time, absent when historical evidence has no value.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub created_at: Option<u64>,
    /// Recoverable original image, independent of application caches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover_asset: Option<PortableAssetReference>,
}

/// Content-addressed original under `echo/assets/<content_hash>`.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PortableAssetReference {
    pub content_hash: String,
    pub byte_len: u64,
    pub mime: String,
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
    /// Original membership time; older records deliberately retain absence.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added_at: Option<u64>,
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
pub(super) const MAX_COMPONENT_BYTES: usize = 4096;

/// Shared validation for any relative path inside the library root.
pub(super) fn validate_relative(value: &str) -> Result<(), Error> {
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
pub(super) fn validate_components(
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
        if component.chars().any(char::is_control)
            || component
                .chars()
                .any(|character| matches!(character, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
        {
            return Err(err(
                "path component contains a character unsupported on Windows",
            ));
        }
        if component.ends_with(['.', ' ']) {
            return Err(err("path component cannot end in a dot or space"));
        }
        let device_name = component
            .split_once('.')
            .map_or(component, |(base, _)| base)
            .trim_end_matches(['.', ' '])
            .to_ascii_uppercase();
        if matches!(device_name.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || ["COM", "LPT"].iter().any(|prefix| {
                device_name.strip_prefix(prefix).is_some_and(|suffix| {
                    matches!(
                        suffix,
                        "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                    )
                })
            })
        {
            return Err(err("path component uses a reserved Windows device name"));
        }
    }
    Ok(())
}

/// Normalise `\` to `/` for cross-platform comparison.
pub(super) fn normalize_slash(value: &str) -> String {
    value.replace('\\', "/")
}

pub(super) fn err(reason: &str) -> Error {
    Error::validation(Subject::Path, "LibraryPath", reason)
}
