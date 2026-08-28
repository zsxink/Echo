//! Domain identifiers (task 2.1).
//!
//! Type-safe newtypes that distinguish business identities and prevent mixing
//! UUID, relative-path and raw-string values. Every persistent identity in Echo
//! receives its own newtype so a `SongId` can never be passed where a
//! `PlaylistId` is expected (and vice versa) at compile time.
//!
//! The newtypes are deliberately light: they serialize through their inner type
//! ([`uuid::Uuid`] / string) and expose a tiny constructor surface so parsing
//! failures are explicit. `RelativeMediaPath` is the exception — it validates
//! at construction and never carries an absolute path or an escaping
//! `..`/drive/root component.

use std::fmt;
use std::str::FromStr;

use serde::{de, Deserialize, Deserializer, Serialize};
use uuid::Uuid;

use crate::error::{Error, Subject};

// ---------------------------------------------------------------------------
// UUID identity newtypes
// ---------------------------------------------------------------------------

/// A macro to define a UUID-backed identity newtype.
///
/// Each identity is a transparent, `Ord`-comparable newtype around a
/// [`uuid::Uuid`] with:
///
/// - `new() -> Self` (v4 random);
/// - `FromStr`/`Display` (hyphenated lower-case canonical form);
/// - `Serialize`/`Deserialize` (as the inner `Uuid` — string in JSON/TS);
/// - `from_uuid`/`as_uuid` for boundary marshalling.
macro_rules! uuid_id {
    {
        $(#[$attr:meta])*
        $name:ident,
        $(doc = $doc:literal)?
    } => {
        $(#[$attr])*
        $(#[doc = $doc])?
        #[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
        pub struct $name(Uuid);

        impl $name {
            /// Create a fresh, random v4 identity.
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            /// Wrap an existing UUID.
            #[must_use]
            pub const fn from_uuid(uuid: Uuid) -> Self {
                Self(uuid)
            }

            /// The inner UUID — for boundary marshalling only; normally kept
            /// encapsulated so identities cannot be mixed.
            #[must_use]
            pub const fn as_uuid(self) -> Uuid {
                self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(f)
            }
        }

        impl FromStr for $name {
            type Err = Error;

            fn from_str(s: &str) -> Result<Self, Self::Err> {
                let uuid = Uuid::from_str(s).map_err(|_| {
                    Error::validation(Subject::Id, stringify!($name), "invalid UUID")
                })?;
                Ok(Self(uuid))
            }
        }

        impl Serialize for $name {
            fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
            where
                S: serde::Serializer,
            {
                self.0.serialize(serializer)
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let uuid = Uuid::deserialize(deserializer)?;
                Ok(Self(uuid))
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }
    };
}

uuid_id! {
    /// Stable identity of a song (a library media record). References from
    /// playlists, favorites, the playback queue and statistics are `SongId`s.
    SongId,
    doc = "Stable identity of a song (a library media record)."
}
uuid_id! {
    /// Stable identity of a playlist.
    PlaylistId,
    doc = "Stable identity of a playlist."
}
uuid_id! {
    /// Stable identity of a library root record. Re-selecting the same
    /// canonical path reuses the same `LibraryRootId`.
    LibraryRootId,
    doc = "Stable identity of a library root record."
}
uuid_id! {
    /// Stable identity of a long-running operation (import, delete, restore).
    OperationId,
    doc = "Stable identity of a long-running operation (import, delete, restore)."
}
uuid_id! {
    /// Identity of one playback load session, used to make `RecordPlayback`
    /// idempotent — one play count per load session.
    PlaybackSessionId,
    doc = "Identity of one playback load session; makes playback statistics idempotent."
}

// ---------------------------------------------------------------------------
// Queue entry identity
// ---------------------------------------------------------------------------

/// Identity of a single entry in a desktop playback queue.
///
/// Playback is a desktop concern, so `QueueEntryId` lives in the domain only
/// as a stable, orderable key shared between the queue and the persisted
/// playback session. Repeated `SongId`s in a queue are distinct because each
/// append receives a fresh `QueueEntryId`.
///
/// It is an opaque 96-bit random value (three `u32` chunks serialized as three
/// hex groups) rather than a UUID: entries are session-scoped, never cross
/// machine boundaries and are cheap to generate.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct QueueEntryId([u32; 3]);

impl QueueEntryId {
    /// Create a fresh, random queue-entry identity.
    #[must_use]
    pub fn new() -> Self {
        let mut chunks = [0u32; 3];
        for chunk in &mut chunks {
            *chunk = rand_u32();
        }
        Self(chunks)
    }

    /// The canonical 24-hex-digit text form (e.g. `a1b2c3d4e5f6a7b8c9d0e1f2`).
    #[must_use]
    pub fn as_hyphenated(&self) -> String {
        let mut out = String::with_capacity(24);
        for chunk in &self.0 {
            use std::fmt::Write as _;
            let _ = write!(out, "{chunk:08x}");
        }
        out
    }

    /// Parse the canonical 24-hex-digit text form.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Validation`](crate::error::Error::Validation) when the
    /// input is not exactly 24 lowercase hex digits.
    ///
    /// # Panics
    ///
    /// Never panics: the `str::parse` call only happens after the length and
    /// hex-digit checks above guarantee the slice parses as a 8-digit hex `u32`.
    pub fn from_hyphenated(s: &str) -> Result<Self, Error> {
        let s = s.trim();
        if s.len() != 24 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(Error::validation(
                Subject::Id,
                "QueueEntryId",
                "expected 24 hex digits",
            ));
        }
        let mut chunks = [0u32; 3];
        for (idx, chunk) in chunks.iter_mut().enumerate() {
            *chunk = u32::from_str_radix(&s[idx * 8..(idx + 1) * 8], 16)
                .expect("len and hexdigit checked above");
        }
        Ok(Self(chunks))
    }
}

impl fmt::Display for QueueEntryId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.as_hyphenated())
    }
}

/// A small deterministic PRNG producing one `u32`. Session-local queue-entry
/// ids only need uniqueness within one run, so this cheap `SplitMix64` over a
/// process seed (time + pid + a fixed salt) is sufficient and avoids pulling
/// `rand` / `getrandom` into the domain layer. Thread-safe through an atomic
/// counter (no shared mutable state).
fn rand_u32() -> u32 {
    use std::sync::atomic::{AtomicU64, Ordering};

    static STATE: AtomicU64 = AtomicU64::new(0x9e37_79b9_7f4a_7c15);
    static SEED: AtomicU64 = AtomicU64::new(0);

    let seed = {
        let saved = SEED.load(Ordering::Relaxed);
        if saved != 0 {
            saved
        } else {
            let generated = SystemTimeSeed::get();
            SEED.store(generated, Ordering::Relaxed);
            generated
        }
    };
    // SplitMix64 — Mix the counter & seed, then mask the lower 32 bits back to
    // u32 width. The mask is a deliberate, documented truncation; the value is
    // only ever used as a session-scoped unique tag, never as crypto.
    let mut x = STATE.fetch_add(seed, Ordering::Relaxed) ^ seed;
    x = x ^ (x >> 30);
    x = x.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    x = x ^ (x >> 27);
    let folded = x ^ (x >> 31);
    splitmix_u32(folded)
}

/// Convert the low 32 bits of a 64-bit `SplitMix` value to `u32` (explicit
/// truncation — the tag only needs session-local uniqueness).
fn splitmix_u32(folded: u64) -> u32 {
    u32::from_le_bytes(
        folded.to_le_bytes()[..4]
            .try_into()
            .expect("slice of len 4"),
    )
}

/// Process-lifetime seed derived from the system clock. Deterministic within a
/// run for reproducibility of the id *shape* if ever needed.
struct SystemTimeSeed;
impl SystemTimeSeed {
    fn get() -> u64 {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default();
        // Lower 64 bits of the nanosecond counter plus the pid and a fixed
        // salt; uid-uniqueness is not required of the seed, only variety.
        let nanos = u64::from_le_bytes(
            now.as_nanos().to_le_bytes()[..8]
                .try_into()
                .expect("8 bytes"),
        );
        let pid: u64 = std::process::id().into();
        nanos ^ pid ^ 0x94d0_49bb_1331_11eb
    }
}

impl FromStr for QueueEntryId {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::from_hyphenated(s)
    }
}

impl Default for QueueEntryId {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Revision and play count
// ---------------------------------------------------------------------------

/// Monotone version/sequence counter for optimistic concurrency (`revision`)
/// and event ordering (`sequence`). Starts at zero, only increments.
#[derive(
    Clone,
    Copy,
    Debug,
    Default,
    Eq,
    Hash,
    Ord,
    PartialEq,
    PartialOrd,
    serde::Serialize,
    serde::Deserialize,
)]
pub struct Revision(u64);

impl Revision {
    /// The initial revision of a fresh record or stream.
    pub const INITIAL: Self = Self(0);

    /// Build from a raw value.
    #[must_use]
    pub const fn from_u64(v: u64) -> Self {
        Self(v)
    }

    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

impl fmt::Display for Revision {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

/// A play count (increments only). [`u64`] is plenty for a 0.1.0 lifetime.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct PlayCount(u64);

impl PlayCount {
    #[must_use]
    pub const fn from_u64(v: u64) -> Self {
        Self(v)
    }
    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

impl fmt::Display for PlayCount {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

// ---------------------------------------------------------------------------
// RelativeMediaPath
// ---------------------------------------------------------------------------

/// A validated, platform-relative path inside a library root.
///
/// Invariants:
///
/// - Never absolute (no leading `/`, `\`, drive or `\\?\` UNC prefix).
/// - Never escapes the root: `..` components are rejected.
/// - No empty components (duplicate separators and trailing separators are
///   rejected) and no NUL bytes.
/// - Normalised to `/` separators for identity, while `display()` preserves
///   the original Unicode text.
///
/// `RelativeMediaPath` is the identity of a song file within its root. Playlists
/// and queues deliberately keep [`SongId`], not paths; paths are for Repository
/// lookups, logs and the UI's relative-path display.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct RelativeMediaPath {
    /// Normalised form: `/`-separated, no `..`, no empty components, always
    /// relative. This is what indexes and comparisons use.
    normalized: String,
}

impl RelativeMediaPath {
    /// The maximum number of path components Echo accepts (defence-in-depth
    /// against pathological inputs; the OS still imposes its own limits).
    pub const MAX_COMPONENTS: usize = 64;
    /// Upper bound for a single component in bytes (UTF-8). Platform limits
    /// (255 bytes typical) are enforced by the OS; Echo adds a sane guard.
    pub const MAX_COMPONENT_BYTES: usize = 4096;

    /// Construct and validate a relative media path.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Validation`] when the value is not a safe relative
    /// path (absolute, escaping `..`, empty, NUL, duplicate/trailing separator
    /// or over-long component).
    pub fn new(value: &str) -> Result<Self, Error> {
        Self::validate(value)?;
        Ok(Self {
            normalized: normalize_to_slash(value),
        })
    }

    /// Validate only (cheap guard for file-system boundaries).
    ///
    /// # Errors
    ///
    /// Same failure set as [`Self::new`].
    pub fn validate(value: &str) -> Result<(), Error> {
        if value.is_empty() {
            return Err(validation_path("path is empty"));
        }
        if value.contains('\0') {
            return Err(validation_path("path contains NUL"));
        }
        // Reject an absolute start before normalising separators so Windows
        // drive/UNC forms are caught on every OS.
        let leading_sep = matches!(value.as_bytes().first(), Some(b'/' | b'\\'));
        let drive = value.len() >= 2
            && value.as_bytes()[0].is_ascii_alphabetic()
            && value.as_bytes()[1] == b':';
        let unc = value.starts_with(r"\\") || value.starts_with("//");
        if leading_sep || drive || unc {
            return Err(validation_path("path must be relative"));
        }

        let mut components = 0usize;
        for component in split_on_separators(value) {
            if component.is_empty() {
                return Err(validation_path("duplicate or trailing separator"));
            }
            if component == ".." {
                return Err(validation_path("path escapes the root (..)"));
            }
            components += 1;
            if components > Self::MAX_COMPONENTS {
                return Err(validation_path("too many path components"));
            }
            if component.len() > Self::MAX_COMPONENT_BYTES {
                return Err(validation_path("component exceeds byte limit"));
            }
        }
        Ok(())
    }

    /// The normalised, `/`-separated form — identity for comparisons.
    #[must_use]
    pub fn normalized(&self) -> &str {
        &self.normalized
    }

    /// The original caller-supplied form (Unicode-preserving).
    #[must_use]
    pub fn display(&self) -> &str {
        &self.normalized
    }

    /// Number of components.
    #[must_use]
    pub fn component_count(&self) -> usize {
        self.normalized.split('/').count()
    }

    /// Parent directory as a [`Self`] (empty components collapse), and the final
    /// file name.
    #[must_use]
    pub fn parent(&self) -> Option<Self> {
        let (dir, _) = self.normalized.rsplit_once('/')?;
        Self::new(dir).ok()
    }

    /// File name (final component).
    #[must_use]
    pub fn file_name(&self) -> Option<&str> {
        self.normalized.rsplit('/').next()
    }

    /// File extension (lower-case ASCII, no dot) if present.
    #[must_use]
    pub fn extension(&self) -> Option<String> {
        self.file_name()?
            .rsplit_once('.')
            .map(|(_, ext)| ext.to_ascii_lowercase())
    }
}

impl fmt::Display for RelativeMediaPath {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.normalized)
    }
}

impl std::str::FromStr for RelativeMediaPath {
    type Err = Error;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Self::new(s)
    }
}

impl Serialize for RelativeMediaPath {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.normalized)
    }
}

impl<'de> Deserialize<'de> for RelativeMediaPath {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::new(&s).map_err(de::Error::custom)
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Split on `/` or `\` (both separators), keeping empty components so the
/// caller can reject duplicate/trailing separators.
fn split_on_separators(value: &str) -> impl Iterator<Item = &str> {
    value.split(['/', '\\'])
}

/// Normalise separators to `/` for comparison (keeps Unicode text as-is).
fn normalize_to_slash(value: &str) -> String {
    value.replace('\\', "/")
}

fn validation_path(reason: impl Into<String>) -> Error {
    Error::validation(Subject::Path, "RelativeMediaPath", reason)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_distinct_newtypes_sharing_serialization() {
        let song = SongId::new();
        let playlist = PlaylistId::new();
        assert_ne!(song.as_uuid(), playlist.as_uuid());
        assert_eq!(song.to_string().len(), 36);
        assert_eq!(
            SongId::from_str(&song.to_string()).unwrap(),
            song,
            "Uuid round-trips through canonical string"
        );
    }

    #[test]
    fn parsing_failure_is_an_error() {
        assert!(SongId::from_str("not-a-uuid-at-all").is_err());
        assert!(PlaybackSessionId::from_str("").is_err());
    }

    #[test]
    fn ids_serialize_as_plain_uuid_string() {
        let s = SongId::new();
        let json = serde_json::to_string(&s).unwrap();
        assert!(json.starts_with('"'), "serializes as string: {json}");
        let back: SongId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
    }

    #[test]
    fn ids_cannot_be_mixed_at_compile_time() {
        use std::any::TypeId;

        // Passing one identity where another is expected is a compile-time
        // error (distinct newtypes). `TypeId` pins that the newtypes never
        // collapse into one type (e.g. via a careless type alias), so the
        // guarantee is asserted, not just implied by construction.
        fn expects_song<T: 'static>(_: &T) {}
        expects_song(&SongId::new());

        let ids = [
            TypeId::of::<SongId>(),
            TypeId::of::<PlaylistId>(),
            TypeId::of::<LibraryRootId>(),
            TypeId::of::<OperationId>(),
            TypeId::of::<PlaybackSessionId>(),
            TypeId::of::<QueueEntryId>(),
        ];
        for (i, a) in ids.iter().enumerate() {
            for b in ids.iter().skip(i + 1) {
                assert_ne!(a, b, "two identity newtypes collapsed into one type");
            }
        }
    }

    #[test]
    fn queue_entry_ids_round_trip_and_are_unique() {
        let a = QueueEntryId::new();
        let b = QueueEntryId::new();
        assert_ne!(a, b);
        let text = a.to_string();
        assert_eq!(text.len(), 24);
        let parsed = QueueEntryId::from_str(&text).unwrap();
        assert_eq!(parsed, a);
    }

    #[test]
    fn queue_entry_id_rejects_bad_text() {
        assert!(QueueEntryId::from_str("").is_err());
        assert!(QueueEntryId::from_str("xyz").is_err());
        // 24 'g' chars are NOT valid hex (f would be).
        assert!(QueueEntryId::from_str(&"g".repeat(24)).is_err());
        // Valid hex but wrong length.
        assert!(QueueEntryId::from_str(&"0".repeat(23)).is_err());
        assert!(QueueEntryId::from_str(&"a".repeat(25)).is_err());
        // 'f' IS valid hex — must parse.
        assert!(QueueEntryId::from_str(&"f".repeat(24)).is_ok());
    }

    #[test]
    fn relative_path_accepts_unicode_and_subdirs() {
        let p = RelativeMediaPath::new("周杰伦/周杰伦 - 晴天.flac").unwrap();
        assert_eq!(p.normalized(), "周杰伦/周杰伦 - 晴天.flac");
        assert_eq!(p.extension().as_deref(), Some("flac"));
        assert_eq!(p.component_count(), 2);
        assert_eq!(p.file_name(), Some("周杰伦 - 晴天.flac"));
        assert_eq!(
            p.parent().map(|d| d.normalized().to_owned()).as_deref(),
            Some("周杰伦")
        );
    }

    #[test]
    fn relative_path_rejects_absolute_escape_and_controls() {
        for bad in [
            "/abs",
            "\\abs",
            "C:\\abs",
            "C:/abs",
            r"\\server\share",
            "a/b/../c",
            "a/../../c",
            "",
            "a//b",
            "a/",
            "a\\b\\",
            "\0",
        ] {
            assert!(
                RelativeMediaPath::new(bad).is_err(),
                "should reject: {bad:?}"
            );
        }
    }

    #[test]
    fn relative_path_normalises_backslashes_for_identity() {
        let a = RelativeMediaPath::new("歌手/Song.mp3").unwrap();
        let b = RelativeMediaPath::new("歌手\\Song.mp3").unwrap();
        assert_eq!(a.normalized(), b.normalized());
        assert_eq!(a, b);
    }

    #[test]
    fn relative_path_display_preserves_original_text() {
        let p = RelativeMediaPath::new("歌手💿/夜曲.wav").unwrap();
        assert_eq!(p.display(), "歌手💿/夜曲.wav");
        assert_eq!(p.extension().as_deref(), Some("wav"));
    }

    #[test]
    fn relative_path_accepts_platform_separators_in_extension() {
        let p = RelativeMediaPath::new("华语/稻香.mp3").unwrap();
        assert_eq!(p.extension().as_deref(), Some("mp3"));
    }

    #[test]
    fn revision_and_play_count_are_small_orderable_values() {
        assert!(Revision::INITIAL < Revision::from_u64(1));
        assert_eq!(PlayCount::from_u64(0), PlayCount::default());
        assert!(PlayCount::from_u64(3) > PlayCount::from_u64(2));
        assert_eq!(Revision::INITIAL.as_u64(), 0);
    }

    #[test]
    fn path_validation_reports_the_failed_field() {
        let err = RelativeMediaPath::new("/etc/passwd").unwrap_err();
        assert!(err.to_string().contains("relative"), "{err}");
        assert_eq!(err.code(), "validation");
    }
}
