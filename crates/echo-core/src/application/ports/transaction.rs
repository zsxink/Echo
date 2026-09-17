use crate::domain::entities::{
    LibraryRoot, LyricsCandidate, LyricsSource, PlaylistMember, Song, SongAvailability,
};
use crate::domain::ids::{LibraryRootId, OperationId, PlaylistId, SongId};
use crate::error::Error;

use super::repository::{CoverAssetRef, OperationItem};

pub trait UnitOfWork: Send + Sync {
    /// Run `f` inside one `SQLite` transaction, committing on success and
    /// rolling back on error. The transaction never crosses an `.await`:
    /// `f` is a plain synchronous closure.
    fn with_tx(&self, f: TxWork) -> Result<(), Error>;
}

/// The boxed, `Send` transaction body [`UnitOfWork::with_tx`] runs.
pub type TxWork = Box<dyn FnOnce(&mut dyn TxAccess) -> Result<(), Error> + Send + 'static>;

/// The narrow, repository-like surface visible *inside* a transaction.
/// Concrete implementations map these onto the same connection as the outer
/// repositories, so atomicity is real. (Marker-ish by design: use cases that
/// need cross-repository atomic writes pass `&dyn TxAccess` to repos.)
// A transaction is deliberately thread-confined. `UnitOfWork::with_tx` takes
// a synchronous closure, so this context never crosses an async/thread
// boundary; requiring `Send + Sync` here would make a real SQLite transaction
// impossible while providing no safety benefit.
/// Song mutations that must participate in an existing transaction.
pub trait TxSongWriter {
    /// Insert/update a song within the open transaction.
    fn upsert_song(&mut self, song: &Song) -> Result<(), Error>;
    /// Permanently remove a song after the platform has durably accepted its
    /// staged files. Foreign-key cascades remove its lyrics, overrides, play
    /// sessions and playlist memberships in this same authority snapshot.
    fn delete_song(&mut self, id: SongId) -> Result<(), Error>;
    /// Update a song's availability in the same transaction as journal state.
    fn set_song_availability(
        &mut self,
        id: SongId,
        availability: SongAvailability,
    ) -> Result<(), Error>;
    /// Update a favorite without splitting its mutation snapshot.
    fn set_song_favorite(&mut self, id: SongId, favorite: bool) -> Result<(), Error>;
    /// Record an idempotence-protected playback update.
    fn increment_song_play_count(&mut self, id: SongId) -> Result<(), Error>;
}

/// Library-root mutations that must participate in an existing transaction.
pub trait TxRootWriter {
    /// Insert/update a root record.
    fn upsert_root(&mut self, root: &LibraryRoot) -> Result<(), Error>;
    /// Atomically isolate destructive writes after an indeterminate system
    /// trash result. `available` records whether the root was still readable
    /// while evaluating the staged evidence.
    fn isolate_root_writes(&mut self, id: LibraryRootId, available: bool) -> Result<(), Error>;
}

/// Playlist mutations that must participate in an existing transaction.
pub trait TxPlaylistWriter {
    /// Create a playlist in the same transaction as initial membership writes.
    fn create_playlist(
        &mut self,
        id: PlaylistId,
        root: LibraryRootId,
        name: &str,
    ) -> Result<(), Error>;
    /// Insert a playlist membership within the open transaction.
    fn insert_member(&mut self, member: &PlaylistMember) -> Result<(), Error>;
    /// Remove a playlist membership during a delete finalization.
    fn remove_member(&mut self, playlist: PlaylistId, song: SongId) -> Result<(), Error>;
}

/// Operation-journal mutations that must participate in an existing transaction.
pub trait TxOperationWriter {
    /// Persist operation-item intent/result alongside affected records.
    fn upsert_operation_item(
        &mut self,
        operation: OperationId,
        item: OperationItem,
    ) -> Result<(), Error>;
    /// Release active target claims as part of an operation's terminal
    /// transaction. This prevents a crash after finalization from leaving a
    /// path permanently reserved.
    fn release_operation_claims(&mut self, operation: OperationId) -> Result<(), Error>;
    /// Persist the operation's `undo_deadline` (epoch millis) in the same
    /// transaction as the song's pending-delete hide (design §9: the deadline
    /// and the hide are one atomic step, so a crash cannot split them).
    fn set_undo_deadline(&mut self, operation: OperationId, deadline_ms: i64) -> Result<(), Error>;
}

/// Lyrics mutations that must participate in an existing transaction.
pub trait TxLyricsWriter {
    /// Write one parsed lyrics candidate row (`(song, source)`) inside the
    /// open transaction, keyed by the candidate's source. A rescan upserts
    /// embedded/sidecar rows with fresh text; override rows are only ever
    /// written by the override use cases, never by a scan.
    fn set_lyrics_candidate(
        &mut self,
        song: SongId,
        candidate: &LyricsCandidate,
    ) -> Result<(), Error>;
    /// Remove one source's candidate row inside the open transaction (a
    /// rescan must not leave stale embedded/sidecar text behind). Scans only
    /// ever clear `Embedded`/`Sidecar`; an `Override` clear is a user action
    /// (no such entry point in 0.1.0).
    fn clear_lyrics_candidate(&mut self, song: SongId, source: LyricsSource) -> Result<(), Error>;
}

/// Cover and runtime-state mutations that must participate in an existing
/// transaction.
pub trait TxStateWriter {
    /// Attach a cover asset to a song (and upsert the `cover_assets` row) in
    /// the same transaction as the song write, keeping the cache key and the
    /// database reference consistent (task 4.6).
    fn attach_cover(&mut self, song: SongId, cover: &CoverAssetRef) -> Result<(), Error>;
    /// Persist a runtime key/value (root epoch) inside the same transaction
    /// as the activation commit (design §5).
    fn set_runtime_state(&mut self, key: &str, value: &str) -> Result<(), Error>;
}

/// Complete transaction capability retained as the stable application entry
/// point. Each focused supertrait has at most five operations, so adapters can
/// implement and test only the capability a use case actually consumes.
pub trait TxAccess:
    TxSongWriter + TxRootWriter + TxPlaylistWriter + TxOperationWriter + TxLyricsWriter + TxStateWriter
{
}

impl<T> TxAccess for T where
    T: TxSongWriter
        + TxRootWriter
        + TxPlaylistWriter
        + TxOperationWriter
        + TxLyricsWriter
        + TxStateWriter
{
}
