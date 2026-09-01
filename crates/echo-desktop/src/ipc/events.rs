//! Typed IPC events with monotonic sequence and revision (task 7.4, design §10).
//!
//! Every state change the frontend should hear about is delivered as an
//! [`IpcEvent`] wrapped in a [`VersionedEvent`] envelope carrying a per-stream
//! monotonic [`EventSequence`]. The frontend honours one rule set:
//!
//! - **Sequence is monotonic.** A consumer records the highest sequence it has
//!   applied (its [`EventWatermark`]) and *drops* any event whose sequence is
//!   `<=` that watermark — that is what kills duplicates and late/out-of-order
//!   deliveries. A **gap** (a skipped sequence number) is legal and must not be
//!   treated as stale: a broker may coalesce or drop an intermediate event, and
//!   the consumer must still apply the next higher one.
//! - **Events are invalidations, not the database.** `SongsInvalidated`
//!   carries the catalog `revision` it refers to. After a listener re-subscribes
//!   (window recreated, connection re-established) it must first pull the
//!   authoritative snapshot, adopt that snapshot's watermark, and only then
//!   apply later events — so a backlog of old events can never overwrite fresher
//!   state.
//!
//! This module is deliberately self-contained (no Tauri, no player actor): the
//! envelope and the guard are pure, so the stale/duplicate/gap/rebuild rules are
//! proven here without any live subsystem. The heavyweight payloads (a full
//! `PlayerSnapshot`, operation per-step progress) arrive with their owning tasks
//! (8.x / 7.5) and simply extend [`IpcEvent`].

use serde::Serialize;

/// A per-stream, monotonic event sequence. Never reused; a gap (skipped number)
/// is legal and carries no "stale" meaning by itself.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct EventSequence(u64);

impl EventSequence {
    /// The first sequence a broker issues.
    pub const INITIAL: Self = Self(1);

    /// The next sequence after this one.
    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0.wrapping_add(1))
    }

    /// The raw value (used for diagnostics, never for ordering decisions).
    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }

    /// The value carried in the event envelope.
    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }
}

impl Default for EventSequence {
    fn default() -> Self {
        Self::INITIAL
    }
}

/// The catalog revision an event refers to. Monotonic per library; a consumer
/// treats an event whose revision is already covered by its snapshot as
/// redundant.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct CatalogRevision(u64);

impl CatalogRevision {
    /// The revision of an empty/initial catalog.
    pub const INITIAL: Self = Self(0);

    /// Bump to the next revision.
    #[must_use]
    pub const fn next(self) -> Self {
        Self(self.0.wrapping_add(1))
    }

    /// The raw value.
    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

/// The typed payload of one IPC event.
///
/// Variants mirror the event streams of design §10 (`library://`,
/// `operation://`, `player://`, `app://`). The heavy payloads (a full player
/// snapshot, per-step operation progress) gain their fields in the owning
/// phase tasks; the envelope and the stale-guard are what 7.4 establishes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum IpcEvent {
    /// `library://status` — readiness/availability changed (re-pull status).
    LibraryStatusChanged,
    /// `library://songs-invalidated` — the catalog changed at `revision`;
    /// consumers re-pull the affected view rather than patching blindly.
    SongsInvalidated { revision: CatalogRevision },
    /// `operation://progress` — one journal operation advanced to `phase`.
    OperationProgress {
        operation_id: String,
        phase: String,
        done: bool,
    },
    /// `player://snapshot` — playback state changed. The full snapshot payload
    /// lands with the player tasks (8.x); this variant is the ordering hook.
    PlayerChanged,
    /// `app://file-open-result` — a file-open request was accepted or refused.
    FileOpenResult { accepted: bool },
}

/// The wire envelope: one event with the sequence that orders it on its stream
/// and the catalog revision it refers to.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionedEvent {
    /// The monotonic stream sequence.
    pub sequence: EventSequence,
    /// The catalog revision this event is relative to (informational for the
    /// ordering guard; the sequence is authoritative).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<CatalogRevision>,
    pub event: IpcEvent,
}

/// Issues strictly increasing [`EventSequence`]s, giving every consumer one
/// total order per `emit` stream.
///
/// Cheap to clone; callers usually keep one per stream (library / operation /
/// player / app) so a busy stream cannot crowd the sequences of a quiet one.
#[derive(Clone, Debug, Default)]
pub struct EventBroker {
    next: EventSequence,
}

impl EventBroker {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            next: EventSequence::INITIAL,
        }
    }

    /// Assign the next sequence and wrap `event` in its envelope, with the
    /// catalog revision a caller opts into (usually the current library
    /// revision).
    #[must_use]
    pub fn emit(&mut self, event: IpcEvent, revision: Option<CatalogRevision>) -> VersionedEvent {
        let sequence = self.next;
        self.next = self.next.next();
        VersionedEvent {
            sequence,
            revision,
            event,
        }
    }

    /// The next sequence that will be issued (for snapshot adoption: a consumer
    /// that re-pulls now marks everything below this as already seen).
    #[must_use]
    pub const fn next_sequence(&self) -> EventSequence {
        self.next
    }
}

/// Why a received event was accepted or rejected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Acceptance {
    /// Apply the event.
    Apply,
    /// Drop it: a duplicate or an older, out-of-order delivery.
    Stale,
}

/// The consumer-side watermark: the highest [`EventSequence`] already applied,
/// plus the catalog revision the current view was built from.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EventWatermark {
    sequence: Option<EventSequence>,
    revision: Option<CatalogRevision>,
}

impl EventWatermark {
    /// A fresh consumer with no events applied and no snapshot yet.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            sequence: None,
            revision: None,
        }
    }

    /// Decide whether `event` may be applied against this watermark.
    ///
    /// A gap (a higher sequence after a lower one) is always applied; only a
    /// sequence `<=` the watermark is stale.
    #[must_use]
    pub fn decide(&self, event: &VersionedEvent) -> Acceptance {
        self.sequence.map_or(Acceptance::Apply, |seen| {
            if event.sequence > seen {
                Acceptance::Apply
            } else {
                Acceptance::Stale
            }
        })
    }

    /// Record that `event` was applied, advancing the sequence watermark.
    pub fn applied(&mut self, event: &VersionedEvent) {
        let replace = self.sequence.map_or(true, |seen| event.sequence > seen);
        if replace {
            self.sequence = Some(event.sequence);
        }
        if let Some(revision) = event.revision {
            self.revision = Some(revision);
        }
    }

    /// Adopt a freshly-pulled snapshot: every event below `next` is now
    /// considered already-seen (so a backlog cannot overwrite the snapshot), and
    /// the catalog revision becomes `revision`.
    pub fn rebase_from_snapshot(&mut self, next: EventSequence, revision: CatalogRevision) {
        let next_value = next.value();
        let advance = match self.sequence {
            // The snapshot is authoritative: we now know about everything before
            // `next`, even if individual events were never delivered (gaps).
            Some(seen) if seen.value() < next_value => true,
            _ => false,
        };
        if advance || self.sequence.is_none() {
            self.sequence = Some(EventSequence(next_value - 1));
        }
        self.revision = Some(revision);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn seq(n: u64) -> EventSequence {
        EventSequence(n)
    }

    fn env_at(sequence: EventSequence, revision: Option<CatalogRevision>) -> VersionedEvent {
        VersionedEvent {
            sequence,
            revision,
            event: IpcEvent::SongsInvalidated {
                revision: revision.unwrap_or(CatalogRevision::INITIAL),
            },
        }
    }

    #[test]
    fn broker_issues_strictly_increasing_sequences() {
        let mut broker = EventBroker::new();
        let a = broker.emit(IpcEvent::LibraryStatusChanged, None);
        let b = broker.emit(IpcEvent::PlayerChanged, None);
        let c = broker.emit(IpcEvent::LibraryStatusChanged, None);
        assert!(a.sequence < b.sequence);
        assert!(b.sequence < c.sequence);
    }

    #[test]
    fn duplicate_events_are_dropped() {
        let mut watermark = EventWatermark::new();
        let event = env_at(seq(5), None);
        assert_eq!(watermark.decide(&event), Acceptance::Apply);
        watermark.applied(&event);

        // Re-delivery of the same sequence must not overwrite.
        assert_eq!(watermark.decide(&event), Acceptance::Stale);
    }

    #[test]
    fn out_of_order_older_events_are_dropped() {
        let mut watermark = EventWatermark::new();
        for n in [2, 6] {
            let event = env_at(seq(n), None);
            watermark.applied(&event);
        }
        // A late older event lands after seq 6 is applied.
        assert_eq!(watermark.decide(&env_at(seq(3), None)), Acceptance::Stale);
        // A fresh one still applies.
        assert_eq!(watermark.decide(&env_at(seq(7), None)), Acceptance::Apply);
    }

    #[test]
    fn gaps_are_not_treated_as_stale() {
        let mut watermark = EventWatermark::new();
        let first = env_at(seq(4), None);
        watermark.applied(&first);
        // Sequences 5..=8 were skipped (coalesced/dropped upstream) — still apply.
        assert_eq!(watermark.decide(&env_at(seq(9), None)), Acceptance::Apply);
        watermark.applied(&env_at(seq(9), None));
        assert_eq!(watermark.decide(&env_at(seq(9), None)), Acceptance::Stale);
    }

    #[test]
    fn window_rebuild_adopts_snapshot_and_stale_backlog_is_dropped() {
        let mut broker = EventBroker::new();
        // A backlog of events emitted while the window was gone.
        let backlog_a = broker.emit(
            IpcEvent::SongsInvalidated {
                revision: CatalogRevision(1),
            },
            None,
        );
        let backbone_b = broker.emit(IpcEvent::PlayerChanged, None);

        // The window re-subscribes: it pulls the snapshot, whose catalog
        // revision is 7 and which now knows about everything up to the broker's
        // next sequence (nothing before it can be stale-to-newer).
        let mut watermark = EventWatermark::new();
        watermark.rebase_from_snapshot(broker.next_sequence(), CatalogRevision(7));

        // The old backlog can never overwrite the fresh snapshot.
        assert_eq!(watermark.decide(&backlog_a), Acceptance::Stale);
        assert_eq!(watermark.decide(&backbone_b), Acceptance::Stale);

        // A genuinely new event applies.
        let fresh = broker.emit(IpcEvent::LibraryStatusChanged, None);
        assert_eq!(watermark.decide(&fresh), Acceptance::Apply);
        watermark.applied(&fresh);
        assert_eq!(watermark.decide(&fresh), Acceptance::Stale);
    }

    #[test]
    fn revisions_are_monotonic_and_serde_round_trips() {
        assert!(CatalogRevision::INITIAL < CatalogRevision::INITIAL.next());
        let event = VersionedEvent {
            sequence: EventSequence(3),
            revision: Some(CatalogRevision(9)),
            event: IpcEvent::SongsInvalidated {
                revision: CatalogRevision(9),
            },
        };
        let json = serde_json::to_value(&event).expect("serialize");
        assert_eq!(json["sequence"], 3);
        assert_eq!(json["revision"], 9);
        assert_eq!(json["event"]["type"], "songsInvalidated");
        // camelCase, no snake_case leaks.
        assert!(json.get("event").is_some());
        let rendered = serde_json::to_string(&event).expect("string");
        assert!(!rendered.contains("operation_id"), "camelCase only");
    }
}
