//! Durable playback-session persistence and startup restore (task 8.9).
//!
//! The desktop player keeps its queue, current/history, shuffle round, volume,
//! mute and mode entirely on the desktop layer (design §12). The durable
//! snapshot of that state lives here, stored through the application-private
//! [`crate::platform::local_state::DesktopStateStore`] (which already gives us
//! temp+fsync+atomic-replace), never inside `echo-core`.
//!
//! ## What survives / what is filtered (task 8.9)
//!
//! On **save**:
//! - Temporary (session-only) items are filtered out — they must never persist.
//! - Duplicate `SongId`s are kept as independent `QueueEntryId`s (never folded).
//!
//! On **restore** (冷启动 `restore_playback_session`, stays paused):
//! - External-missing or currently-unreachable songs are preserved as *blocked*
//!   entries (they may come back; see `recovers {}` render decision).
//! - Songs Echo permanently deleted, or not belonging to the *active* root, are
//!   dropped and summarized once.
//! - The recovered positions/settings are applied but playback stays paused.
//!
//! A later explicit system file-open request overrides the restored context and
//! may *auto-play* (task 9.2); normal restore never auto-plays (设计: 不得自动
//! 开始发声).
//!
//! ## Schema
//!
//! Stored as a camelCase JSON value inside `desktop-state.json` under
//! `playbackSession`. `PlaybackSession` is the typed counterpart; it is
//! serialized via serde and exchanged with the opaque
//! [`crate::platform::local_state::PlaybackSessionValue`] slot.
//!
//! The schema is versioned (`version: 1`) for forward migration; unknown
//! versions are treated as empty (safe) rather than panicking.

use std::collections::HashSet;

use echo_core::domain::ids::{QueueEntryId, SongId};
use serde::{Deserialize, Serialize};

use super::port::PlayMode;
#[cfg(test)]
use super::queue::TemporaryItem;
use super::queue::{HistoryRecord, Queue, QueueEntry, QueueItem};

/// The current on-disk session schema version.
pub const SESSION_VERSION: u32 = 3;

/// The serialized, durable playback-session document (camelCase, versioned).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackSession {
    pub version: u32,
    /// Only *library* queue entries (temporaries filtered on save).
    pub entries: Vec<PersistedEntry>,
    /// The current entry id, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current: Option<String>,
    /// Timestamped entry history, newest first. `PersistedHistoryRecord`
    /// accepts legacy string IDs while deserializing version-1 sessions.
    pub history: Vec<PersistedHistoryRecord>,
    /// The active shuffle round (entry ids in play order).
    pub shuffle_bag: Vec<String>,
    /// FIFO manual "play next" lane. Missing in v1/v2 sessions means no
    /// manual priorities, preserving backward-compatible restore behavior.
    #[serde(default)]
    pub priority: Vec<String>,
    /// Whether shuffle mode was active.
    pub shuffle_active: bool,
    /// Playback settings.
    pub mode: PlayMode,
    /// The view the queue was built from ("allSongs" / "favorites" / "recent"
    /// / "search" / "playlist:<id>") — 记住当前播放的是哪个歌单的哪首歌. `None`
    /// for sessions started before sources existed (or a temporary play).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub volume: f64,
    pub muted: bool,
    /// Last position of the *current* song (best-effort; restored paused).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub position: Option<f64>,
}

impl PlaybackSession {
    /// A new empty session (nothing to restore).
    #[must_use]
    pub fn empty() -> Self {
        Self {
            version: SESSION_VERSION,
            entries: Vec::new(),
            current: None,
            history: Vec::new(),
            shuffle_bag: Vec::new(),
            priority: Vec::new(),
            shuffle_active: false,
            mode: PlayMode::Sequential,
            source: None,
            volume: 1.0,
            muted: false,
            position: None,
        }
    }
}

/// One persisted queue entry: always a *library* song (temporaries are dropped).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersistedEntry {
    pub queue_entry_id: String,
    pub song_id: String,
}

/// A persisted history visit. The custom untagged representation lets a
/// version-1 `history: ["entry-id"]` document load safely; old records are
/// deliberately assigned timestamp zero and expire rather than becoming an
/// immortal or misleading previous-track entry.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum PersistedHistoryRecord {
    Timestamped { entry_id: String, played_at_ms: u64 },
    Legacy(String),
}

impl PersistedHistoryRecord {
    fn parse(self) -> Option<HistoryRecord> {
        match self {
            Self::Timestamped {
                entry_id,
                played_at_ms,
            } => Some(HistoryRecord {
                entry_id: entry_id.parse().ok()?,
                played_at_ms,
            }),
            Self::Legacy(_) => None,
        }
    }
}

impl PersistedEntry {
    /// Parse the entry ids back into domain types; malformed ids are skipped.
    #[must_use]
    pub fn parse(self) -> Option<(QueueEntryId, SongId)> {
        let qid = self.queue_entry_id.parse().ok()?;
        let sid = self.song_id.parse().ok()?;
        Some((qid, sid))
    }
}

/// How a song should be handled at restore time.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RestoreDisposition {
    /// Available: restore a playable library entry.
    Restore,
    /// Missing / temporarily unreachable: keep as a *blocked* entry.
    Blocked,
    /// Permanently deleted or not on the active root: drop + summarize.
    Drop,
}

/// The verdict for one queued song at restore; decided by the caller (desktop
/// platform layer) against the live repository, then applied here.
pub struct RestoreVerdict {
    pub song_id: SongId,
    pub disposition: RestoreDisposition,
}

impl Default for RestoreVerdict {
    fn default() -> Self {
        Self {
            song_id: SongId::new(),
            disposition: RestoreDisposition::Restore,
        }
    }
}

/// Build the durable snapshot of a live queue. Temporary items are filtered;
/// duplicate `SongId`s keep independent entry ids; the current, history and
/// shuffle-bag positions, mode, volume and mute are captured verbatim.
#[must_use]
pub fn snapshot_queue(
    queue: &Queue,
    mode: PlayMode,
    volume: f64,
    muted: bool,
    current_position: Option<f64>,
    source: Option<&str>,
) -> PlaybackSession {
    let entries: Vec<PersistedEntry> = queue
        .entries()
        .iter()
        .filter_map(|entry| match &entry.item {
            QueueItem::Library(song) => Some(PersistedEntry {
                queue_entry_id: entry.id.to_string(),
                song_id: song.to_string(),
            }),
            // Temporaries never persist.
            QueueItem::Temporary(_) => None,
        })
        .collect();
    let priority: Vec<String> = queue
        .priority_ids()
        .iter()
        .filter(|id| {
            entries
                .iter()
                .any(|entry| entry.queue_entry_id == id.to_string())
        })
        .map(std::string::ToString::to_string)
        .collect();
    PlaybackSession {
        version: SESSION_VERSION,
        entries,
        current: queue.current_id().map(|id| id.to_string()),
        history: queue
            .history_records()
            .iter()
            .map(|record| PersistedHistoryRecord::Timestamped {
                entry_id: record.entry_id.to_string(),
                played_at_ms: record.played_at_ms,
            })
            .collect(),
        shuffle_bag: queue
            .shuffle_bag()
            .iter()
            .map(|id| id.to_string())
            .collect(),
        priority,
        shuffle_active: queue.is_shuffle(),
        mode,
        source: source.map(std::string::ToString::to_string),
        volume,
        muted,
        position: current_position,
    }
}

/// Apply the restore verdicts to a persisted session and rebuild a queue.
///
/// Rules (task 8.9):
/// - **Restore** entries are re-added in stored order with their original
///   `QueueEntryId` (so the current/history/shuffle references stay valid).
/// - **Blocked** (missing/temporarily-unreachable) entries are kept *by id*,
///   in the queue's order, but their file is unavailable — the coordinator
///   treats a blocked current as "skip and find the next available".
/// - **Drop** entries are removed entirely and counted (to summarize once).
/// - The restored current is applied only if it still exists in the rebuilt
///   queue; otherwise the first available entry becomes current (but paused).
///
/// Returns the rebuilt queue plus a summary of what was dropped.
pub fn rebuild_queue(
    session: &PlaybackSession,
    verdicts: &[RestoreVerdict],
) -> (Queue, RestoreSummary) {
    let mut verdict_by_song = std::collections::HashMap::new();
    let mut dropped: HashSet<SongId> = HashSet::new();
    for verdict in verdicts {
        match verdict.disposition {
            RestoreDisposition::Drop => {
                dropped.insert(verdict.song_id);
            }
            _ => {
                verdict_by_song.insert(verdict.song_id, verdict.disposition);
            }
        }
    }

    let mut queue = Queue::new();
    let mut dropped_count = 0usize;
    for persisted in &session.entries {
        let Some((qid, sid)) = persisted.clone().parse() else {
            continue; // malformed id: skip
        };
        // Determine disposition: explicit drop beats all; otherwise restore.
        let disposition = verdict_by_song.get(&sid).copied().unwrap_or_else(|| {
            if dropped.contains(&sid) {
                RestoreDisposition::Drop
            } else {
                RestoreDisposition::Restore
            }
        });
        match disposition {
            RestoreDisposition::Drop => {
                dropped_count += 1;
            }
            // Blocked and Restore are both represented by a present entry; the
            // distinction matters only to the *caller*'s file resolution, which
            // the coordinator performs when it actually loads. For queue
            // membership both keep the entry with its original id.
            RestoreDisposition::Blocked | RestoreDisposition::Restore => {
                queue.push(QueueEntry {
                    id: qid,
                    item: QueueItem::Library(sid),
                });
                queue.set_blocked(qid, disposition == RestoreDisposition::Blocked);
            }
        }
    }

    // Restore the current reference if it still names an entry.
    if let Some(current) = &session.current {
        if let Ok(cid) = current.parse() {
            queue.set_current(cid);
        }
    }

    queue.restore_history(
        session
            .history
            .iter()
            .cloned()
            .filter_map(PersistedHistoryRecord::parse),
    );

    // Restore the shuffle bag / active flag (never re-shuffled — 恢复后不重洗).
    let bag: Vec<QueueEntryId> = session
        .shuffle_bag
        .iter()
        .filter_map(|s| s.parse().ok())
        .collect();
    queue.set_shuffle(session.shuffle_active, bag);
    queue.set_priority(
        session
            .priority
            .iter()
            .filter_map(|id| id.parse().ok())
            .collect(),
    );

    (
        queue,
        RestoreSummary {
            restored: session.entries.len().saturating_sub(dropped_count),
            dropped: dropped_count,
            blocked: session
                .entries
                .iter()
                .filter_map(|entry| entry.clone().parse().map(|(_, song)| song))
                .filter(|song| verdict_by_song.get(song) == Some(&RestoreDisposition::Blocked))
                .count(),
        },
    )
}

/// A short summary of what a restore rebuilt, for the one-time UI note.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RestoreSummary {
    pub restored: usize,
    pub dropped: usize,
    pub blocked: usize,
}

/// A trait for the desktop's durable-session boundary, so `Player` logic can
/// save/restore without touching a concrete file store (testable).
pub trait SessionPersistence: Send + Sync {
    /// Persist the session durably and atomically. `None` clears it.
    ///
    /// # Errors
    ///
    /// An io/serialization failure — the caller may surface a non-blocking
    /// "session not saved" note; playback continues regardless.
    fn save(&self, session: Option<&PlaybackSession>) -> Result<(), String>;

    /// Load the previously-persisted session (if any).
    ///
    /// # Errors
    ///
    /// An io failure reading the store; a missing/empty value is `Ok(None)`.
    fn load(&self) -> Result<Option<PlaybackSession>, String>;
}

/// A [`SessionPersistence`] backed by the atomic `DesktopStateStore`
/// (temp + fsync + atomic replace). Converts to/from the opaque JSON slot.
/// The store is shared (`Arc`) so the command layer, the saver thread and the
/// restore path all observe the same instance.
pub struct StateStoreSession {
    store: std::sync::Arc<crate::platform::local_state::DesktopStateStore>,
}

impl StateStoreSession {
    /// Wrap the local-state store as a session persistence boundary.
    #[must_use]
    pub fn new(store: std::sync::Arc<crate::platform::local_state::DesktopStateStore>) -> Self {
        Self { store }
    }
}

impl SessionPersistence for StateStoreSession {
    fn save(&self, session: Option<&PlaybackSession>) -> Result<(), String> {
        let value = session.map(|s| {
            serde_json::to_value(s)
                .unwrap_or_else(|_| serde_json::json!({ "version": SESSION_VERSION }))
        });
        self.store
            .set_playback_session(value)
            .map_err(|e| e.to_string())
    }

    fn load(&self) -> Result<Option<PlaybackSession>, String> {
        let Some(value) = self.store.playback_session().map_err(|e| e.to_string())? else {
            return Ok(None);
        };
        match serde_json::from_value::<PlaybackSession>(value) {
            Ok(session) if matches!(session.version, 1..=SESSION_VERSION) => Ok(Some(session)),
            // Unknown version: treat as empty (safe), never restore garbage.
            _ => Ok(None),
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::local_state::DesktopStateStore;

    fn lib_entry(s: SongId) -> QueueEntry {
        QueueEntry {
            id: QueueEntryId::new(),
            item: QueueItem::Library(s),
        }
    }

    fn temp_entry() -> QueueEntry {
        QueueEntry {
            id: QueueEntryId::new(),
            item: QueueItem::Temporary(TemporaryItem {
                display_name: "t.mp3".into(),
                path: std::path::PathBuf::from("/tmp/t.mp3"),
                duration: None,
                on_active_root: false,
            }),
        }
    }

    #[test]
    fn snapshot_filters_temporaries_keeps_duplicates() {
        let s1 = SongId::new();
        let s2 = SongId::new();
        let mut q = Queue::new();
        let a = q.push(lib_entry(s1));
        let _t = q.push(temp_entry());
        let b = q.push(lib_entry(s1)); // duplicate SongId, independent id
        let _c = q.push(lib_entry(s2));
        q.set_current(a);
        q.set_current(b); // current = second s1 entry

        let session = snapshot_queue(
            &q,
            PlayMode::Shuffle,
            0.6,
            true,
            Some(12.5),
            Some("playlist:p1"),
        );
        // Two library entries (the temporary is filtered), both the s1 entries
        // retained with distinct ids.
        assert_eq!(session.entries.len(), 3);
        assert!(session.current.is_some());
        assert_eq!(session.mode, PlayMode::Shuffle);
        assert_eq!(session.source.as_deref(), Some("playlist:p1"));
        assert_eq!(session.volume, 0.6);
        assert!(session.muted);
        assert_eq!(session.position, Some(12.5));
        let _ = a;
        let _ = b;
    }

    #[test]
    fn rebuild_queue_drops_permanently_deleted_and_unreachable() {
        // s1 is retained (Restore), s2 is a temp that was filtered already, and
        // s3 is dropped (Echo deleted). s4 is blocked.
        let s1 = SongId::new();
        let s2 = SongId::new();
        let s3 = SongId::new();
        let s4 = SongId::new();
        let mut q = Queue::new();
        let id1 = q.push(lib_entry(s1));
        let _id2 = q.push(lib_entry(s2));
        let _id3 = q.push(lib_entry(s3));
        let _id4 = q.push(lib_entry(s4));
        q.set_current(id1);
        let session = snapshot_queue(&q, PlayMode::Sequential, 1.0, false, None, None);

        let (queue, summary) = rebuild_queue(
            &session,
            &[
                RestoreVerdict {
                    song_id: s1,
                    disposition: RestoreDisposition::Restore,
                },
                RestoreVerdict {
                    song_id: s2,
                    disposition: RestoreDisposition::Restore,
                },
                RestoreVerdict {
                    song_id: s3,
                    disposition: RestoreDisposition::Drop,
                },
                RestoreVerdict {
                    song_id: s4,
                    disposition: RestoreDisposition::Blocked,
                },
            ],
        );
        assert_eq!(summary.dropped, 1, "only the deleted song is dropped");
        assert_eq!(queue.len(), 3, "s1, s2, s4 restored");
        // s3 (dropped) must not be present.
        let present: Vec<Option<SongId>> =
            queue.entries().iter().map(|e| e.item.song_id()).collect();
        assert!(!present.contains(&Some(s3)));
        assert!(present.contains(&Some(s1)));
        assert!(present.contains(&Some(s4)));
        // current preserved if it still exists.
        assert_eq!(queue.current_id(), Some(id1));
    }

    #[test]
    fn state_store_save_load_round_trip_is_atomic_and_clears() {
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("desktop-state.json");
        let store = DesktopStateStore::new(
            path.clone(),
            crate::platform::local_state::PlatformCloseDefault::Other,
        );
        let persist = StateStoreSession::new(std::sync::Arc::new(store));

        let session = PlaybackSession {
            version: SESSION_VERSION,
            entries: vec![PersistedEntry {
                queue_entry_id: QueueEntryId::new().to_string(),
                song_id: SongId::new().to_string(),
            }],
            current: None,
            history: vec![],
            shuffle_bag: vec![],
            priority: vec![],
            shuffle_active: false,
            mode: PlayMode::Sequential,
            source: None,
            volume: 0.9,
            muted: false,
            position: None,
        };
        persist.save(Some(&session)).expect("save");
        let loaded = persist.load().expect("load");
        assert!(loaded.is_some(), "round-trips");
        let loaded = loaded.unwrap();
        assert_eq!(loaded.version, SESSION_VERSION);
        assert_eq!(loaded.volume, 0.9);
        // Clear works.
        persist.save(None).expect("clear");
        assert!(persist.load().expect("load after clear").is_none());
    }

    #[test]
    fn unknown_version_is_treated_as_empty() {
        // A stored session with an unknown version must not be restored.
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("desktop-state.json");
        let store = DesktopStateStore::new(
            path.clone(),
            crate::platform::local_state::PlatformCloseDefault::Other,
        );
        let raw = serde_json::json!({
            "version": 99,
            "entries": [],
            "history": [],
            "shuffleBag": [],
            "shuffleActive": false,
            "mode": "sequential",
            "volume": 1.0,
            "muted": false,
        });
        store
            .set_playback_session(Some(raw))
            .expect("store opaque payload");
        let persist = StateStoreSession::new(std::sync::Arc::new(store));
        assert!(
            persist.load().expect("load unknown").is_none(),
            "unknown version is a safe empty restore"
        );
    }

    #[test]
    fn priority_lane_round_trips_through_snapshot_and_rebuild() {
        use echo_core::domain::ids::QueueEntryId;
        let s1 = SongId::new();
        let s2 = SongId::new();
        let mut queue = Queue::new();
        let current = queue.push(lib_entry(s1));
        queue.set_current(current);
        let a = queue.insert_next(lib_entry(s2));
        let b = queue.insert_next(lib_entry(s1)); // duplicate song, own id

        let session = snapshot_queue(&queue, PlayMode::Sequential, 1.0, false, None, None);
        assert_eq!(session.priority.len(), 2);
        // The lane survives a rebuild with the same FIFO order.
        let (rebuilt, _summary) = rebuild_queue(&session, &[]);
        assert_eq!(
            rebuilt
                .priority_ids()
                .iter()
                .map(id_string)
                .collect::<Vec<_>>(),
            [a, b].iter().map(id_string).collect::<Vec<_>>()
        );
        fn id_string(id: &QueueEntryId) -> String {
            std::string::ToString::to_string(id)
        }
    }

    #[test]
    fn old_session_without_priority_restores_with_an_empty_lane() {
        // v1/v2 documents have no `priority` field; `#[serde(default)]` must
        // restore them as an empty manual lane (backward-compatible playback).
        let dir = tempfile::tempdir().expect("temp");
        let path = dir.path().join("desktop-state.json");
        let store = DesktopStateStore::new(
            path.clone(),
            crate::platform::local_state::PlatformCloseDefault::Other,
        );
        let raw = serde_json::json!({
            "version": 2,
            "entries": [
                { "queueEntryId": QueueEntryId::new().to_string(), "songId": SongId::new().to_string() }
            ],
            "history": [],
            "shuffleBag": [],
            "shuffleActive": false,
            "mode": "sequential",
            "volume": 0.8,
            "muted": false
        });
        store
            .set_playback_session(Some(raw))
            .expect("store opaque payload");
        let persist = StateStoreSession::new(std::sync::Arc::new(store));
        let loaded = persist
            .load()
            .expect("load")
            .expect("version 2 is restored, not dropped");
        assert_eq!(loaded.version, 2);
        assert!(
            loaded.priority.is_empty(),
            "an old session must restore with no manual priorities"
        );
        let (rebuilt, _summary) = rebuild_queue(&loaded, &[]);
        assert!(
            rebuilt.priority_ids().is_empty(),
            "rebuilt queue carries the empty lane from the old session"
        );
    }

    #[test]
    fn restore_keeps_fresh_history_and_counts_only_blocked_entries() {
        let song = SongId::new();
        let mut queue = Queue::new();
        let entry = queue.push(lib_entry(song));
        queue.set_current(entry);
        let session = snapshot_queue(&queue, PlayMode::Sequential, 1.0, false, None, None);
        let (rebuilt, summary) = rebuild_queue(
            &session,
            &[RestoreVerdict {
                song_id: song,
                disposition: RestoreDisposition::Blocked,
            }],
        );
        assert!(rebuilt.is_blocked(entry));
        assert!(!rebuilt.history_records().is_empty());
        assert_eq!(
            summary,
            RestoreSummary {
                restored: 1,
                blocked: 1,
                dropped: 0
            }
        );
    }
}
