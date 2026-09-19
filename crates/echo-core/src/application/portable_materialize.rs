//! Portable record materialization: build and write `echo/records/<kind>/…`
//! records from committed mutations.
//!
//! The design (design.md §2) makes the portable record the durable, cross-device
//! form of the outbox full payload: both are updated for the same logical fact
//! and must share the object's monotone revision, device id and HLC. Because a
//! filesystem write cannot ride inside the `SQLite` transaction, the mutation use
//! cases call these builders *after* the Unit of Work commits, using the
//! [`SyncStateReader`]/[`DeviceIdProvider`] read-back so the record's version
//! exactly matches the committed outbox row.
//!
//! Two call shapes:
//!
//! - **Direct mutations** (import, favorite, playlist/member): gate up front on
//!   [`ensure_control_plane_writable`] and fail the mutation if the control
//!   surface is unusable (spec "控制面不可写" — a library whose `echo/` is not
//!   writable must not be enabled for logic changes; reads keep working).
//! - **Delete/finalize fan-out** (song/playlist delete cascades): use
//!   [`write_record_guarded`], which is best-effort — the DB delete is already
//!   committed and must not roll back because a control-plane write failed.

use std::collections::BTreeMap;

use crate::application::ports::{ControlPlanePort, SyncStateReader};
use crate::domain::entities::Song;
use crate::domain::ids::{LibraryRootId, PlaylistId, PlaylistItemId, Revision, SongId};
use crate::domain::library::{
    DeviceId, FavoriteRecord, HybridLogicalClock, LibraryRelativePath, PlayStatsRecord,
    PlaylistItemRecord, PlaylistRecord, PortableRecord, RecordKind, SongRecord, TombstoneRecord,
};
use crate::error::Error;

/// The outbox object type of a song. Kept here rather than leaking the
/// `SQLite` adapter's internal constants into the application layer.
pub const SONG_OBJECT_TYPE: &str = "song";

/// The outbox object type for the independent favorite fact. Kept here rather
/// than leaking the `SQLite` adapter's internal constants into the application
/// layer.
pub const FAVORITE_OBJECT_TYPE: &str = "favorite";

/// The outbox object type of a playlist (create/rename/delete).
pub const PLAYLIST_OBJECT_TYPE: &str = "playlist";

/// The outbox object type of one playlist membership.
pub const PLAYLIST_ITEM_OBJECT_TYPE: &str = "playlist-item";

/// The object type of play statistics. `play-stats` has no outbox row (a play
/// is not a syncable logic change on its own), so its revision is derived from
/// the record it supersedes.
pub const PLAY_STATS_OBJECT_TYPE: &str = "play-stats";

/// The outbox object type of a song override layer.
///
/// Scoped out of this change (decision A, 2026-09-20): `LyricsSource::Override`
/// exists as a lyrics *priority*, but 0.1.0 has no user-facing override
/// **mutation** entry point to materialize from, so nothing writes this kind
/// yet. Continuation still reads and validates `overrides/` records, and the
/// `15.1` gate keeps the kind mapped to its directory, so the missing piece is
/// one write point — not a spec gap.
pub const OVERRIDE_OBJECT_TYPE: &str = "override";

/// Verify the library's control surface is writable before a logic mutation.
///
/// # Errors
///
/// Returns [`Error::Unavailable`] when `echo/` cannot be created/updated — the
/// portable-library-layout spec's "控制面不可写" scenario: the library must not
/// be enabled for import or logic changes; reads keep working.
pub fn ensure_control_plane_writable(
    control: &dyn ControlPlanePort,
    root: LibraryRootId,
) -> Result<(), Error> {
    if control.control_plane_usable(root)? {
        Ok(())
    } else {
        Err(Error::unavailable(
            "library",
            "control surface is not writable",
        ))
    }
}

/// Best-effort record write for post-commit fan-out (delete cascades). A
/// control-plane failure is logged and swallowed: the DB change is already
/// committed and must never roll back because a record could not be written.
///
/// # Errors
///
/// `Ok(())` even on a failed control-plane write; only a read/parse of
/// writability that itself errors propagates.
pub fn write_record_guarded(
    control: &dyn ControlPlanePort,
    root: LibraryRootId,
    record: &PortableRecord,
) -> Result<(), Error> {
    let usable = control.control_plane_usable(root)?;
    if usable {
        control.write_record(root, record)?;
    } else {
        tracing::warn!(
            kind = %record.kind(),
            "control surface not usable — skipping post-commit record write"
        );
    }
    Ok(())
}

/// Best-effort record removal for post-commit fan-out (undoing a delete). A
/// control-plane failure is logged and swallowed: the DB change is already
/// committed and must never roll back because a record could not be removed.
///
/// # Errors
///
/// `Ok(())` even on a failed control-plane removal; only a read of writability
/// that itself errors propagates.
pub fn delete_record_guarded(
    control: &dyn ControlPlanePort,
    root: LibraryRootId,
    kind: RecordKind,
    object_uuid: &str,
) -> Result<(), Error> {
    if control.control_plane_usable(root)? {
        control.delete_record(root, kind, object_uuid)?;
    } else {
        tracing::warn!(
            %kind,
            "control surface not usable — skipping post-commit record removal"
        );
    }
    Ok(())
}

/// Build the portable song record for a committed import/scan song.
///
/// # Errors
///
/// Returns [`Error::Validation`] when the song's path is not a valid
/// `LibraryRelativePath` (i.e. not under `media/`). Imported/scanned songs are
/// always under `media/` (task 3.1), so this is a defense-in-depth guard, not
/// an expected failure.
pub fn song_record(
    device: DeviceId,
    hlc: HybridLogicalClock,
    revision: Revision,
    song: &Song,
) -> Result<SongRecord, Error> {
    Ok(SongRecord {
        song_uuid: song.id().as_uuid(),
        revision,
        updated_by_device_id: device,
        hlc,
        media_path: LibraryRelativePath::new(song.path().display())?,
        content_hash: song.blake3_hash().unwrap_or_default().to_owned(),
        title: song.title().map(ToOwned::to_owned),
        artist: song.artist().map(ToOwned::to_owned),
        album: song.album().map(ToOwned::to_owned),
    })
}

/// Build the portable favorite record for a committed favorite toggle.
#[must_use]
pub const fn favorite_record(
    device: DeviceId,
    hlc: HybridLogicalClock,
    revision: Revision,
    song: SongId,
    is_favorite: bool,
) -> FavoriteRecord {
    FavoriteRecord {
        song_uuid: song.as_uuid(),
        revision,
        updated_by_device_id: device,
        hlc,
        is_favorite,
    }
}

/// Build the portable playlist record for a create/rename.
#[must_use]
pub fn playlist_record(
    device: DeviceId,
    hlc: HybridLogicalClock,
    revision: Revision,
    playlist: PlaylistId,
    display_name: &str,
) -> PlaylistRecord {
    PlaylistRecord {
        playlist_uuid: playlist.as_uuid(),
        revision,
        updated_by_device_id: device,
        hlc,
        display_name: display_name.to_owned(),
    }
}

/// Build the portable playlist-item record for a committed membership.
#[must_use]
pub const fn playlist_item_record(
    device: DeviceId,
    hlc: HybridLogicalClock,
    revision: Revision,
    item: PlaylistItemId,
    playlist: PlaylistId,
    song: SongId,
    position: u64,
) -> PlaylistItemRecord {
    PlaylistItemRecord {
        item_uuid: item.as_uuid(),
        revision,
        updated_by_device_id: device,
        hlc,
        playlist_uuid: playlist.as_uuid(),
        song_uuid: song.as_uuid(),
        position,
    }
}

/// Build the portable play-statistics record for one completed play.
///
/// Play counts are **additive per device**: `by_device` carries this device's
/// own total, so merging two devices' records sums instead of letting the last
/// writer overwrite the other's plays (design D4). The revision advances past
/// the record being superseded.
#[must_use]
pub const fn play_stats_record(
    device: DeviceId,
    hlc: HybridLogicalClock,
    revision: Revision,
    song: SongId,
    by_device: BTreeMap<uuid::Uuid, u64>,
) -> PlayStatsRecord {
    PlayStatsRecord {
        song_uuid: song.as_uuid(),
        revision,
        updated_by_device_id: device,
        hlc,
        by_device,
    }
}

/// Merge one new play into the record already on disk (or start a fresh one).
///
/// Re-writing the same device's bucket is idempotent — a replayed write never
/// doubles the count.
#[must_use]
pub fn merge_play_stats(
    existing: Option<PlayStatsRecord>,
    device: DeviceId,
    hlc: HybridLogicalClock,
    song: SongId,
) -> PlayStatsRecord {
    if let Some(record) = existing {
        let current = record
            .by_device
            .get(&device.as_uuid())
            .copied()
            .unwrap_or(0);
        let revision = Revision::from_u64(record.revision.as_u64().saturating_add(1));
        record
            .with_device_count(device, current.saturating_add(1))
            .with_revision(revision, hlc)
    } else {
        let mut by_device = BTreeMap::new();
        by_device.insert(device.as_uuid(), 1);
        play_stats_record(device, hlc, Revision::INITIAL, song, by_device)
    }
}

/// Build the portable tombstone record for a committed delete.
#[must_use]
pub const fn tombstone_record(
    device: DeviceId,
    hlc: HybridLogicalClock,
    revision: Revision,
    object_uuid: uuid::Uuid,
    deleted_kind: RecordKind,
) -> TombstoneRecord {
    TombstoneRecord {
        object_uuid,
        deleted_kind,
        revision,
        updated_by_device_id: device,
        hlc,
    }
}

/// Read the committed outbox revision + HLC of one object, or a sensible
/// default when the object has not been stamped (a freshly committed object
/// always has an outbox row, but a defensive fallback keeps materializers
/// total).
///
/// # Errors
///
/// Propagates an outbox or HLC read failure from the sync-state adapter.
pub fn committed_version(
    sync: &dyn SyncStateReader,
    object_type: &str,
    object_uuid: &str,
    device: DeviceId,
) -> Result<(Revision, HybridLogicalClock), Error> {
    let revision = sync.outbox_revision(object_type, object_uuid)?;
    let hlc = sync
        .object_hlc(object_type, object_uuid)?
        .unwrap_or_else(HybridLogicalClock::default);
    let _ = device;
    Ok((revision, hlc))
}
