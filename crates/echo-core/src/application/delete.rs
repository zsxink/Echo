//! Echo's own delete flow with a 10-second undo window (task 5.7, design §9).
//!
//! Deleting a song is not an instant truncation. It is a per-resource journal
//! operation exactly like the import (design §8/§9):
//!
//! - the audio and the same-basename `.lrc` are moved **individually** into the
//!   dedicated controlled `trash/<operation-id>` slot (同盘 rename), each
//!   rename preceded by a `StagePending` journal write and followed by a hash
//!   verification and `StageApplied`;
//! - once every required item is `StageApplied`, **one** database transaction
//!   marks the song `PendingDelete` (hidden from the default library and the
//!   queue), advances every item to `HiddenInDatabase` and records the undo
//!   deadline (`now + 10s`);
//! - the undo window keeps the song's UUID, favorite, play stats and playlist
//!   memberships intact — undo only restores the files and the song state;
//! - `RestoreDeletedOperation` (undo) moves each file back to its original
//!   path (`RestorePending → RestoreApplied → Restored`), choosing a safe
//!   numbered path when the original is occupied, then restores the song.
//!
//! The trash forward-roll (`SystemTrashPort`, `TrashApplied`, finalization) is
//! task 5.8 and deliberately NOT implemented here; the recovery path hands an
//! expired hidden operation to 5.8 by advancing its items to `TrashPending`.

use crate::application::portable_materialize::delete_record_guarded;
use crate::application::ports::{OperationItem, OperationResourceKind, TxAccess};
use crate::application::scan::ScanDeps;
use crate::domain::ids::{LibraryRootId, OperationId, RelativeMediaPath, SongId};
use crate::domain::library::{PortableRecord, RecordKind};
use crate::domain::state::OperationState;
use crate::domain::sync::ObjectKey;
use crate::error::Error;

pub mod intent;

/// Journal `kind` of a delete operation. The operation lives across the whole
/// delete→hide→(undo|trash) lifecycle of one song.
pub const DELETE_OPERATION: &str = "delete";
/// The undo window: 10 seconds (design §9: `undo_deadline = now + 10s`).
pub const UNDO_WINDOW_MS: i64 = 10_000;

/// Which companion resource of the song is being deleted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeleteResourceKind {
    /// The audio file itself.
    Audio,
    /// The same-basename `.lrc` sidecar, when present.
    Lyrics,
}

/// The outcome of one delete: the operation handle the desktop uses to offer
/// and later perform the undo.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DeleteOutcome {
    pub operation: OperationId,
    /// Whether a same-basename `.lrc` companion was staged and hidden too.
    pub staged_lyrics: bool,
}

/// Delete one song: stage its resources into `trash/<operation-id>`, then hide
/// it with a 10-second undo window. Returns the operation the desktop shows as
/// "撤销删除". The operation's per-resource journal lets recovery finish or
/// undo each item from its persisted intent (design §9 恢复矩阵).
pub struct DeleteSongs<'a> {
    deps: &'a ScanDeps,
}

impl<'a> DeleteSongs<'a> {
    #[must_use]
    pub const fn new(deps: &'a ScanDeps) -> Self {
        Self { deps }
    }

    /// Stage + hide one song.
    ///
    /// # Errors
    ///
    /// - unknown or already-pending-delete song → `Validation`/`Conflict`;
    /// - an item whose file is missing, unreadable or whose staged copy's hash
    ///   does not match → the delete never hides anything (a partial delete
    ///   rolls the operation item back to `RolledBack` and the song stays
    ///   available, so a failed delete is a clean no-op on the library).
    pub fn delete(
        &self,
        root: crate::domain::ids::LibraryRootId,
        song: SongId,
    ) -> Result<DeleteOutcome, Error> {
        let subject = self.deps.songs.by_id(song)?.ok_or_else(|| {
            Error::validation(crate::error::Subject::Other, "song", "unknown song id")
        })?;
        match subject.availability() {
            crate::domain::entities::SongAvailability::Available => {}
            crate::domain::entities::SongAvailability::PendingDelete => {
                return Err(Error::conflict("song is already pending delete"));
            }
            crate::domain::entities::SongAvailability::Missing => {
                return Err(Error::conflict(
                    "song is externally missing; restore it before deleting",
                ));
            }
        }
        // The delete is refused when the root does not permit writes (设计:
        // 只读根目录不提供删除入口).
        if self
            .deps
            .roots
            .by_id(root)?
            .is_some_and(|record| !record.write_capable())
            || !self.deps.fs.write_capable(root)?
        {
            return Err(Error::unavailable("library", "root is read-only"));
        }

        let audio_source = subject.path().clone();
        let audio_hash = self.deps.hasher.hash(root, &audio_source)?;
        let lyrics_path = derive_lrc_path(&audio_source);
        let has_lyrics = self.deps.fs.path_exists(root, &lyrics_path)?;

        let operation = self.deps.ids.new_operation_id();
        self.deps
            .journal
            .ensure_operation(operation, root, DELETE_OPERATION, Some(song))?;

        // Stage the audio first, then the optional lyrics (deterministic order
        // so the journal item keys are stable across runs).
        self.stage_resource(
            root,
            operation,
            song,
            DeleteResourceKind::Audio,
            &audio_source,
            &audio_hash,
        )?;
        let staged_lyrics = if has_lyrics {
            let lyrics_hash = self.deps.hasher.hash(root, &lyrics_path)?;
            self.stage_resource(
                root,
                operation,
                song,
                DeleteResourceKind::Lyrics,
                &lyrics_path,
                &lyrics_hash,
            )?;
            true
        } else {
            false
        };

        // All items applied: one transaction hides the song, marks each item
        // HiddenInDatabase and records the undo deadline.
        //
        // The reversible intent is a directory fact recorded *before* the
        // hide: a crash between the two then leaves an intent the recovery
        // pass can see, never a hidden song whose intent exists only in the
        // database the hide was supposed to write. It carries the identity the
        // final tombstone will use, so undo and finalization compete for one
        // terminal state.
        let created_at_ms = u64::try_from(wall_ms(&*self.deps.clock)?).unwrap_or(0);
        let intent = intent::begin(
            self.deps,
            root,
            operation,
            ObjectKey::Song(song),
            created_at_ms,
        )?;
        let items = self.deps.journal.items(operation)?;
        let deadline = wall_ms(&*self.deps.clock)? + UNDO_WINDOW_MS;
        let hidden: Vec<OperationItem> = items
            .iter()
            .map(|item| OperationItem {
                state: OperationState::HiddenInDatabase,
                ..item.clone()
            })
            .collect();
        let final_mutation_id = intent.final_mutation_id;
        self.deps
            .uow
            .with_tx(Box::new(move |tx: &mut dyn TxAccess| {
                tx.set_song_availability(
                    song,
                    crate::domain::entities::SongAvailability::PendingDelete,
                )?;
                for item in &hidden {
                    tx.upsert_operation_item(operation, item.clone())?;
                }
                tx.set_undo_deadline(operation, deadline, final_mutation_id)?;
                Ok(())
            }))?;

        Ok(DeleteOutcome {
            operation,
            staged_lyrics,
        })
    }

    /// Stage one resource's file into `trash/<operation>` and verify it.
    fn stage_resource(
        &self,
        root: crate::domain::ids::LibraryRootId,
        operation: OperationId,
        song: SongId,
        kind: DeleteResourceKind,
        source: &crate::domain::ids::RelativeMediaPath,
        expected_hash: &str,
    ) -> Result<(), Error> {
        let key = delete_item_key(kind);
        let trash_name = trash_resource_name(source)?;
        let trash_path = self.deps.fs.trash_path(root, operation, trash_name)?;
        let item = OperationItem {
            kind: kind.into(),
            state: OperationState::StagePending,
            song: Some(song),
            source: None,
            staging_path: Some(trash_path.clone()),
            target_path: source.clone(),
            expected_hash: expected_hash.to_owned(),
            item_key: key.to_owned(),
            claim_key: source.identity_key().to_owned(),
        };
        self.deps.journal.upsert_item(operation, item.clone())?;
        let moved = self
            .deps
            .fs
            .stage_to_trash(root, operation, source, trash_name)?;
        debug_assert_eq!(moved, trash_path, "adapter trash path must be stable");
        // Verify the staged copy against the expected hash (设计: 验证暂存 hash
        // 后才写 StageApplied).
        if self.deps.hasher.hash(root, &trash_path)? != expected_hash {
            return Err(Error::CorruptMedia {
                operation: DELETE_OPERATION.to_owned(),
                reason: "staged delete copy hash differs from the source".to_owned(),
            });
        }
        let applied = OperationItem {
            state: OperationState::StageApplied,
            ..item
        };
        self.deps.journal.upsert_item(operation, applied)?;
        Ok(())
    }
}

/// Undo a delete within its undo window: move every staged resource back to
/// its original path (a safe numbered path when the original is occupied) and
/// restore the song — keeping its UUID, favorite, stats and playlist
/// positions.
pub struct RestoreDeletedOperation<'a> {
    deps: &'a ScanDeps,
}

impl<'a> RestoreDeletedOperation<'a> {
    #[must_use]
    pub const fn new(deps: &'a ScanDeps) -> Self {
        Self { deps }
    }

    /// Restore (undo) one delete operation.
    ///
    /// # Errors
    ///
    /// - the operation is not a delete / has no items → `InvariantViolation`;
    /// - the undo window has expired → `Conflict` (the desktop must not offer
    ///   undo; the operation is on its way to 5.8's trash);
    /// - a staged file vanished or a restore hash mismatches → the restore
    ///   fails without touching the song state (the journal keeps the pending
    ///   restore evidence for recovery).
    pub fn restore(
        &self,
        root: crate::domain::ids::LibraryRootId,
        operation: OperationId,
    ) -> Result<SongId, Error> {
        if self
            .deps
            .roots
            .by_id(root)?
            .is_some_and(|record| !record.write_capable())
            || !self.deps.fs.write_capable(root)?
        {
            return Err(Error::unavailable("library", "root is read-only"));
        }
        let items = self.deps.journal.items(operation)?;
        if items.is_empty() {
            return Err(Error::InvariantViolation {
                why: "restore of an operation without journal items".to_owned(),
            });
        }
        // The undo window: only an un-expired hidden delete may be undone.
        match self.deps.journal.undo_deadline(operation)? {
            Some(deadline) if wall_ms(&*self.deps.clock)? <= deadline => {}
            Some(_) => return Err(Error::conflict("undo window expired")),
            None => {
                return Err(Error::InvariantViolation {
                    why: "delete operation without an undo deadline".to_owned(),
                })
            }
        }
        let subject = Self::subject_song(&items)?;

        // A remote final tombstone holds these resources for explicit recovery
        // under a new identity. Check before any filesystem move. SQLite also
        // checks each RestorePending transition and the final visibility write
        // against the confirmed base to close concurrent tombstone races.
        if items.iter().any(|item| {
            !matches!(
                item.state,
                OperationState::HiddenInDatabase
                    | OperationState::RestorePending
                    | OperationState::RestoreApplied
                    | OperationState::Restored
            )
        }) {
            return Err(Error::conflict(
                "delete operation is held; recovery requires a new UUID",
            ));
        }

        // Undo and deadline finalization race for the one terminal state. The
        // final tombstone, and a finalized one refuses this undo before the
        // song is made visible again or any staged resource is moved back.
        intent::cancel(
            self.deps,
            root,
            operation,
            ObjectKey::Song(subject),
            u64::try_from(wall_ms(&*self.deps.clock)?).unwrap_or(u64::MAX),
        )?;

        for item in &items {
            self.restore_resource(root, operation, item)?;
        }

        // All resources restored: restore the song in one transaction.
        self.deps
            .uow
            .with_tx(Box::new(move |tx: &mut dyn TxAccess| {
                tx.set_song_availability(
                    subject,
                    crate::domain::entities::SongAvailability::Available,
                )?;
                Ok(())
            }))?;
        self.deps.journal.release_claims(operation)?;
        // New intents never write a portable tombstone. Remove only the
        // recognizable v1 temporary tombstone left by older builds: same
        // local author and exactly one revision newer than its song record.
        let device = self.deps.device_id.current_device_id();
        let tombstone =
            self.deps
                .control
                .read_record(root, RecordKind::Tombstone, &subject.to_string())?;
        let song_record =
            self.deps
                .control
                .read_record(root, RecordKind::Song, &subject.to_string())?;
        if let (
            Some(PortableRecord::Tombstone(tombstone)),
            Some(PortableRecord::Song(song_record)),
        ) = (tombstone, song_record)
        {
            let legacy_temporary = tombstone.deleted_kind == RecordKind::Song
                && tombstone.updated_by_device_id == device
                && song_record.updated_by_device_id == device
                && tombstone.revision.as_u64() == song_record.revision.as_u64().saturating_add(1);
            if legacy_temporary {
                delete_record_guarded(
                    self.deps.control.as_ref(),
                    root,
                    RecordKind::Tombstone,
                    &subject.to_string(),
                )?;
            }
        }
        Ok(subject)
    }

    /// The subject `SongId` shared by every item of a delete operation.
    fn subject_song(items: &[OperationItem]) -> Result<SongId, Error> {
        let first =
            items
                .iter()
                .find_map(|item| item.song)
                .ok_or_else(|| Error::InvariantViolation {
                    why: "delete operation item without a subject SongId".to_owned(),
                })?;
        if items.iter().all(|item| item.song == Some(first)) {
            Ok(first)
        } else {
            Err(Error::InvariantViolation {
                why: "delete operation items disagree on the subject song".to_owned(),
            })
        }
    }

    /// Move one staged trash file back into the library and journal the
    /// `RestorePending → RestoreApplied → Restored` evidence.
    fn restore_resource(
        &self,
        root: crate::domain::ids::LibraryRootId,
        operation: OperationId,
        item: &OperationItem,
    ) -> Result<(), Error> {
        let Some(trash_path) = item.staging_path.clone() else {
            return Err(Error::InvariantViolation {
                why: "delete item without a persisted staging (trash) path".to_owned(),
            });
        };
        let target = self.safe_restore_target(root, item)?;
        let pending = OperationItem {
            state: OperationState::RestorePending,
            target_path: target.clone(),
            claim_key: target.identity_key().to_owned(),
            ..item.clone()
        };
        self.deps.journal.upsert_item(operation, pending.clone())?;
        self.deps
            .fs
            .restore_from_trash(root, &trash_path, &target)?;
        if self.deps.hasher.hash(root, &target)? != item.expected_hash {
            return Err(Error::CorruptMedia {
                operation: DELETE_OPERATION.to_owned(),
                reason: "restored file hash differs from the staged evidence".to_owned(),
            });
        }
        let applied = OperationItem {
            state: OperationState::RestoreApplied,
            ..pending
        };
        self.deps.journal.upsert_item(operation, applied.clone())?;
        let restored = OperationItem {
            state: OperationState::Restored,
            ..applied
        };
        self.deps.journal.upsert_item(operation, restored)?;
        Ok(())
    }

    /// The path to restore into: the original target when it is free / already
    /// ours, else the minimal `stem (n).ext` safe numbered path (设计: 原路径被
    /// 占用时安全编号恢复). Delegates to the shared [`safe_restore_target`] so
    /// the live undo and the crash-recovery matrix always make the same
    /// "occupied → numbered path" decision.
    fn safe_restore_target(
        &self,
        root: LibraryRootId,
        item: &OperationItem,
    ) -> Result<RelativeMediaPath, Error> {
        safe_restore_target(self.deps, root, item)
    }
}

/// Resolve the path to restore one delete item's staged file into: the
/// original `target_path` when it is free, or the minimal `stem (n).ext` safe
/// numbered path when the original is occupied by *other* content (设计: 原路径
/// 被占用时安全编号恢复). An original path that already carries the expected
/// content counts as free (the restore is then a no-op on the file).
///
/// Shared by the live undo ([`RestoreDeletedOperation`]) and the delete
/// crash-recovery matrix (`RecoverOperations`, task 5.7) so both agree on the
/// occupancy decision and the numbering scheme. A `Conflict` result means the
/// original *and* every ` (1..100)` candidate are occupied by foreign content —
/// the caller decides whether to hold the item; it is never replaced.
pub(crate) fn safe_restore_target(
    deps: &ScanDeps,
    root: LibraryRootId,
    item: &OperationItem,
) -> Result<RelativeMediaPath, Error> {
    if !deps.fs.path_exists(root, &item.target_path)? {
        return Ok(item.target_path.clone());
    }
    // Occupied: if it is already OUR file (hash match), restore is a no-op.
    if deps.hasher.hash(root, &item.target_path)? == item.expected_hash {
        return Ok(item.target_path.clone());
    }
    let extension = item
        .target_path
        .extension()
        .map(|extension| format!(".{extension}"))
        .unwrap_or_default();
    let Some(parent) = item.target_path.parent() else {
        return Err(Error::InvariantViolation {
            why: "delete target without a parent directory".to_owned(),
        });
    };
    let Some(file_name) = item.target_path.file_name() else {
        return Err(Error::InvariantViolation {
            why: "delete target without a file name".to_owned(),
        });
    };
    let stem = file_name
        .rsplit_once('.')
        .map_or_else(|| file_name.to_owned(), |(stem, _)| stem.to_owned());
    // The original is already occupied by foreign content, so numbering
    // starts at ` (1)` (the plain name is not a candidate).
    for suffix in 1..=100_u32 {
        let candidate = format!("{stem} ({suffix}){extension}");
        let path = RelativeMediaPath::new(&format!("{}/{}", parent.display(), candidate))?;
        if !deps.fs.path_exists(root, &path)? {
            return Ok(path);
        }
        if deps.hasher.hash(root, &path)? == item.expected_hash {
            return Ok(path);
        }
    }
    Err(Error::conflict("no safe restore path available"))
}

/// The logical per-resource item key used by the journal (and therefore kept
/// stable as `audio`/`lyrics` for database compatibility). The physical trash
/// entry uses the source file's original basename instead.
const fn delete_item_key(kind: DeleteResourceKind) -> &'static str {
    match kind {
        DeleteResourceKind::Audio => "audio",
        DeleteResourceKind::Lyrics => "lyrics",
    }
}

/// Use the original filename for the physical trash entry while keeping the
/// journal's logical resource key stable. This makes the item recognizable in
/// the system trash and keeps old journals recoverable because recovery reads
/// the persisted staging filename.
pub(crate) fn trash_resource_name(path: &RelativeMediaPath) -> Result<&str, Error> {
    path.file_name().ok_or_else(|| Error::InvariantViolation {
        why: "delete resource path without a filename".to_owned(),
    })
}

/// Wall-clock epoch millis from a [`crate::application::ports::Clock`]
/// (the undo deadline's durable unit).
fn wall_ms(clock: &dyn crate::application::ports::Clock) -> Result<i64, Error> {
    let millis = clock
        .now_wall()
        .duration_since(std::time::UNIX_EPOCH)
        .map_err(|source| {
            Error::io(
                "clock",
                std::io::Error::other(source),
                std::path::PathBuf::new(),
            )
        })?
        .as_millis();
    Ok(i64::try_from(millis).unwrap_or(i64::MAX))
}

/// The same-basename `.lrc` path beside an audio path: same directory, same
/// stem, `.lrc` extension (design §9: 同名 `.lrc`).
fn derive_lrc_path(audio: &RelativeMediaPath) -> RelativeMediaPath {
    let extension = audio.extension().map_or(0, |extension| extension.len());
    let normalized = audio.normalized();
    let without_extension = &normalized[..normalized.len().saturating_sub(extension + 1)];
    RelativeMediaPath::new(&format!("{without_extension}.lrc"))
        .expect("deriving an .lrc sibling of a valid media path stays valid")
}

impl From<DeleteResourceKind> for OperationResourceKind {
    fn from(kind: DeleteResourceKind) -> Self {
        match kind {
            DeleteResourceKind::Audio => Self::Audio,
            DeleteResourceKind::Lyrics => Self::Lyrics,
        }
    }
}

#[cfg(test)]
#[path = "delete/tests.rs"]
mod tests;
