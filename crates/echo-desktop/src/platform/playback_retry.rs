//! App-private, unsaved playback intents. Counts become authoritative only after
//! Core commits the corresponding stable event to its music library.

use echo_core::domain::ids::{LibraryRootId, PlaybackSessionId, SongId};
use echo_core::domain::library::{DeviceId, LibraryId};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const FORMAT_VERSION: u16 = 1;
const MAX_PENDING: usize = 65_536;
const MAX_BYTES: u64 = 32 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackRetryReason {
    NotSaved,
    SavedPendingRecovery,
    SavedPendingProjection,
    LibraryUnavailable,
    InactiveLibrary,
    AuthorChanged,
    LibraryChanged,
    MissingSong,
    EnrollmentPending,
}

/// A path-free diagnostic view. `operation_id` is the original private session,
/// never a new event to submit when a user opens the details panel.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlaybackRetryStatus {
    pub operation_id: PlaybackSessionId,
    pub song: SongId,
    pub qualified_at: u64,
    pub reason: PlaybackRetryReason,
    pub attempts: u32,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PendingPlayback {
    pub root: LibraryRootId,
    pub library: LibraryId,
    pub author: DeviceId,
    pub session: PlaybackSessionId,
    pub song: SongId,
    pub qualified_at: u64,
    pub reason: PlaybackRetryReason,
    pub attempts: u32,
}

impl PendingPlayback {
    fn same_intent(&self, other: &Self) -> bool {
        self.root == other.root
            && self.library == other.library
            && self.author == other.author
            && self.session == other.session
            && self.song == other.song
    }

    fn validate(&self) -> Result<(), String> {
        if self.root.as_uuid().is_nil()
            || self.library.as_uuid().is_nil()
            || self.author.as_uuid().is_nil()
            || self.session.as_uuid().is_nil()
            || self.song.as_uuid().is_nil()
        {
            return Err("playback retry identity is invalid".into());
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    format_version: u16,
    pending: Vec<PendingPlayback>,
}

impl Default for Document {
    fn default() -> Self {
        Self {
            format_version: FORMAT_VERSION,
            pending: Vec::new(),
        }
    }
}

struct Inner {
    path: PathBuf,
    gate: Mutex<()>,
}

/// A durable request queue beside the runtime-owned local database. It is not
/// part of portable recovery and is never exported to another device.
#[derive(Clone)]
pub struct PlaybackRetryStore(Arc<Inner>);

impl PlaybackRetryStore {
    #[must_use]
    pub fn beside_database(database: &Path) -> Self {
        Self(Arc::new(Inner {
            path: database.with_extension("playback-retry.json"),
            gate: Mutex::new(()),
        }))
    }

    /// Read unsaved requests for the selected local root without exposing paths.
    /// # Errors
    /// Preserves and reports unreadable, corrupt or future-format queue bytes.
    pub fn status(&self, root: LibraryRootId) -> Result<Vec<PlaybackRetryStatus>, String> {
        Ok(self
            .pending()?
            .into_iter()
            .filter(|entry| entry.root == root)
            .map(|entry| PlaybackRetryStatus {
                operation_id: entry.session,
                song: entry.song,
                qualified_at: entry.qualified_at,
                reason: entry.reason,
                attempts: entry.attempts,
            })
            .collect())
    }

    pub(crate) fn pending(&self) -> Result<Vec<PendingPlayback>, String> {
        self.with_document(|document| Ok((document.pending.clone(), false)))
    }

    pub(crate) fn enqueue(&self, entry: PendingPlayback) -> Result<(), String> {
        entry.validate()?;
        self.with_document(move |document| {
            if let Some(previous) = document
                .pending
                .iter()
                .find(|pending| pending.session == entry.session)
            {
                if !previous.same_intent(&entry) {
                    return Err("playback retry session has conflicting identity".into());
                }
                return Ok(((), false));
            }
            if document.pending.len() >= MAX_PENDING {
                return Err("playback retry queue is full; previous intents were retained".into());
            }
            document.pending.push(entry);
            Ok(((), true))
        })
    }

    pub(crate) fn complete(&self, entry: &PendingPlayback) -> Result<(), String> {
        self.with_document(|document| {
            let before = document.pending.len();
            document
                .pending
                .retain(|pending| !pending.same_intent(entry));
            Ok(((), document.pending.len() != before))
        })
    }

    pub(crate) fn failed(
        &self,
        entry: &PendingPlayback,
        reason: PlaybackRetryReason,
    ) -> Result<(), String> {
        self.with_document(|document| {
            let Some(pending) = document
                .pending
                .iter_mut()
                .find(|pending| pending.same_intent(entry))
            else {
                return Ok(((), false));
            };
            pending.reason = reason;
            pending.attempts = pending.attempts.saturating_add(1);
            Ok(((), true))
        })
    }

    fn with_document<T>(
        &self,
        update: impl FnOnce(&mut Document) -> Result<(T, bool), String>,
    ) -> Result<T, String> {
        let _guard = self
            .0
            .gate
            .lock()
            .map_err(|_| "playback retry lock failed")?;
        let parent = self.0.path.parent().unwrap_or_else(|| Path::new("."));
        std::fs::create_dir_all(parent).map_err(|error| io(&error))?;
        reject_symlink(&self.0.path)?;
        let lock_path = self.0.path.with_extension("json.lock");
        reject_symlink(&lock_path)?;
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(lock_path)
            .map_err(|error| io(&error))?;
        lock.try_lock_exclusive().map_err(|error| io(&error))?;
        let mut document = self.load()?;
        let (value, changed) = update(&mut document)?;
        if changed {
            self.save(&document)?;
        }
        Ok(value)
    }

    fn load(&self) -> Result<Document, String> {
        let metadata = match std::fs::metadata(&self.0.path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Document::default());
            }
            Err(error) => return Err(io(&error)),
        };
        if !metadata.is_file() || metadata.len() > MAX_BYTES {
            return Err("playback retry document is not a supported file".into());
        }
        let document: Document =
            serde_json::from_slice(&std::fs::read(&self.0.path).map_err(|error| io(&error))?)
                .map_err(|_| "playback retry document is damaged; original bytes were retained")?;
        if document.format_version != FORMAT_VERSION || document.pending.len() > MAX_PENDING {
            return Err("playback retry document requires a supported version".into());
        }
        let mut sessions = std::collections::BTreeSet::new();
        for entry in &document.pending {
            entry.validate()?;
            if !sessions.insert(entry.session.as_uuid()) {
                return Err("playback retry document repeats an event identity".into());
            }
        }
        Ok(document)
    }

    fn save(&self, document: &Document) -> Result<(), String> {
        let bytes = serde_json::to_vec(document).map_err(|_| "encode playback retry document")?;
        if bytes.len() as u64 > MAX_BYTES {
            return Err("playback retry document reached its safe size limit".into());
        }
        let temporary = self
            .0
            .path
            .with_extension(format!("retry-{}.tmp", uuid::Uuid::new_v4()));
        let result = (|| {
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)
                .map_err(|error| io(&error))?;
            file.write_all(&bytes).map_err(|error| io(&error))?;
            file.sync_all().map_err(|error| io(&error))?;
            drop(file);
            std::fs::rename(&temporary, &self.0.path).map_err(|error| io(&error))?;
            sync_parent(self.0.path.parent().unwrap_or_else(|| Path::new(".")))
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
        }
        result
    }
}

fn reject_symlink(path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Err("playback retry path is a symbolic link".into())
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io(&error)),
    }
}

#[cfg(unix)]
fn sync_parent(path: &Path) -> Result<(), String> {
    use std::fs::File;

    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(|error| io(&error))
}

/// Windows has no supported directory fsync: `FlushFileBuffers` requires a
/// writable handle and `CreateFile` cannot open a directory for writing. The
/// temporary file's content flush before the rename remains the durability
/// guarantee.
#[cfg(not(unix))]
#[allow(clippy::missing_const_for_fn, clippy::unnecessary_wraps)]
fn sync_parent(_path: &Path) -> Result<(), String> {
    Ok(())
}

fn io(error: &std::io::Error) -> String {
    format!("playback retry storage is unavailable: {}", error.kind())
}

#[cfg(test)]
mod tests;
