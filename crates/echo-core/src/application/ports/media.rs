use super::*;

/// Container-level media probe: format + duration + audio parameters.
/// Deliberately separate from tag reading so probing can live on its own
/// actor / thread budget.
pub trait MediaProbe: Send + Sync {
    fn probe(&self, root: LibraryRootId, path: &RelativeMediaPath) -> Result<ProbeOutcome, Error>;
}

/// Probe result.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProbeOutcome {
    Audio {
        format: AudioFormat,
        duration: Option<Duration>,
    },
    /// The file is a supported container but has no playable audio track.
    NoAudioTrack,
    /// Not a supported media file at all.
    Unsupported,
}

/// Metadata (tags) reader — returns parsed fields and optionally embedded
/// cover/lyrics handles through [`CoverCache`]/[`LyricsParser`].
pub trait MetadataReader: Send + Sync {
    fn read(&self, root: LibraryRootId, path: &RelativeMediaPath) -> Result<ParsedMetadata, Error>;
    /// Read the tags of in-memory content. Import sources are planned from
    /// their bytes (task 5.2: the `歌手/歌手 - 歌曲名.扩展名` target must be
    /// known before anything is written into the library), so the reader must
    /// accept content directly, not only files under a library root.
    ///
    /// # Errors
    ///
    /// Unreadable or unrecognized content (corrupt container) — the caller
    /// reports a failed input instead of guessing a name.
    fn read_bytes(&self, content: &[u8]) -> Result<ParsedMetadata, Error>;
}

/// Full-file content hashing (BLAKE3). Returns the hex digest.
pub trait ContentHasher: Send + Sync {
    fn hash(&self, root: LibraryRootId, path: &RelativeMediaPath) -> Result<String, Error>;
    fn hash_of_bytes(&self, bytes: &[u8]) -> String;
}

/// Cover asset store — keyed by content hash, returns an opaque asset key
/// (never a raw filesystem path to the UI).
pub trait CoverCache: Send + Sync {
    /// Persist cover bytes under their content hash; returns the asset key.
    fn put(&self, bytes: &[u8], mime: &str) -> Result<String, Error>;
    /// Resolve an asset key to its byte ranges for the read-only protocol;
    /// unknown/malformed keys are rejected.
    fn get(&self, asset_key: &str) -> Result<Option<Vec<u8>>, Error>;
    /// Delete unreferenced assets (GC entry point).
    fn gc(&self, referenced_keys: &[String]) -> Result<(), Error>;
}

/// Lyrics parser — turns raw text into typed, timestamp-sorted lines.
pub trait LyricsParser: Send + Sync {
    fn parse(&self, raw: &str) -> LyricsCandidate;
}
