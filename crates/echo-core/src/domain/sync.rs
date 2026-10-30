//! Synchronization identity and ordering value objects.
//!
//! These types are transport and persistence independent. In particular, a
//! commit sequence is scoped by a binding and must never be compared across
//! libraries or epochs.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

use super::ids::{PlaylistId, PlaylistItemId, Revision, SongId};
pub use super::sync_binding::{CommitCheckpoint, SyncBinding, SyncSessionIntent};

/// Audio properties are probed from the file and can be missing on one
/// device even when both devices have the exact same media bytes. They are
/// useful metadata, but do not represent competing user edits.
const SONG_DERIVED_AUDIO_FIELDS: [&str; 4] = [
    "bitrate_bps",
    "sample_rate_hz",
    "channels",
    "bits_per_sample",
];

fn merge_song_derived_audio_facts(winner: &mut Value, other: &Value) {
    let (Some(winner), Some(other)) = (winner.as_object_mut(), other.as_object()) else {
        return;
    };
    for field in SONG_DERIVED_AUDIO_FIELDS {
        let missing = winner.get(field).is_none_or(Value::is_null);
        if missing {
            if let Some(value) = other.get(field).filter(|value| !value.is_null()) {
                winner.insert(field.to_owned(), value.clone());
            }
        }
    }
}

fn same_song_user_data(left: &Value, right: &Value) -> bool {
    let mut left = left.clone();
    let mut right = right.clone();
    if let Some(object) = left.as_object_mut() {
        for field in SONG_DERIVED_AUDIO_FIELDS {
            object.remove(field);
        }
    }
    if let Some(object) = right.as_object_mut() {
        for field in SONG_DERIVED_AUDIO_FIELDS {
            object.remove(field);
        }
    }
    left == right
}

macro_rules! uuid_identity {
    ($name:ident) => {
        #[derive(
            Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize,
        )]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            #[must_use]
            pub const fn from_uuid(value: Uuid) -> Self {
                Self(value)
            }
            #[must_use]
            pub const fn as_uuid(self) -> Uuid {
                self.0
            }
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }
        }
        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }
    };
}

uuid_identity!(LibraryId);
uuid_identity!(AuthorityId);
uuid_identity!(DeviceId);
uuid_identity!(MutationId);

/// Monotone generation of one library authority. A restored authority must
/// receive a new epoch before serving peers.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Epoch(u64);

impl Epoch {
    pub const INITIAL: Self = Self(1);
    #[must_use]
    pub const fn from_u64(value: u64) -> Self {
        Self(value)
    }
    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

pub use super::sync_members::*;
pub use super::sync_resources::*;

/// An object identity whose type remains part of the key.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum ObjectKey {
    Song(SongId),
    Playlist(PlaylistId),
    PlaylistItem(PlaylistItemId),
    LibraryRoot(Uuid),
    Favorite(SongId),
    Override(SongId),
    PlayStats(SongId),
}

/// A sequence allocated by one authority. Values from different bindings are
/// intentionally incomparable through this API.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CommitSeq(u64);

impl CommitSeq {
    pub const GENESIS: Self = Self(0);
    #[must_use]
    pub const fn from_u64(value: u64) -> Self {
        Self(value)
    }
    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }
    pub fn next(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}

/// The most recent host-confirmed sequence for one typed object. It is
/// deliberately distinct from a device-local [`Revision`].
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ObjectVersion(CommitSeq);

impl ObjectVersion {
    pub const INITIAL: Self = Self(CommitSeq::GENESIS);
    #[must_use]
    pub const fn from_commit_seq(value: CommitSeq) -> Self {
        Self(value)
    }
    #[must_use]
    pub const fn commit_seq(self) -> CommitSeq {
        self.0
    }
}

/// Device-local object revision, named separately at protocol boundaries to
/// prevent it being mistaken for a host version.
pub type LocalRevision = Revision;

/// A stable, already-persisted local edit awaiting host confirmation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MutationCandidate {
    pub mutation_id: MutationId,
    pub origin_device_id: DeviceId,
    pub origin_seq: u64,
    pub previous_mutation_id: Option<MutationId>,
    pub object: ObjectKey,
    pub base_object_version: Option<ObjectVersion>,
    pub payload: Value,
    /// Immutable resource declarations frozen with this local event.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resource_refs: Vec<SharedResourceManifest>,
}

/// Current confirmed state for one object, used by the pure host decision
/// function. Persistence and publication remain application/infrastructure
/// responsibilities.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConfirmedObject {
    pub object: ObjectKey,
    pub version: ObjectVersion,
    pub origin_device_id: DeviceId,
    pub payload: Value,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OriginCursor {
    pub origin_device_id: DeviceId,
    pub origin_seq: u64,
    pub last_mutation_id: MutationId,
}

/// A position in one device's origin chain, without asserting that a local
/// cursor row exists. A replica re-anchors its pending events onto the host's
/// position: the confirmed ledger is the only authority for that sequence
/// space, so a local database that was rebuilt, rebound to another library
/// root, or trimmed by a migration can never invent a chain the host cannot
/// follow.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OriginAnchor {
    pub origin_seq: u64,
    pub last_mutation_id: Option<MutationId>,
}

impl OriginAnchor {
    /// The anchor of an origin whose chain has not started yet.
    pub const GENESIS: Self = Self {
        origin_seq: 0,
        last_mutation_id: None,
    };

    #[must_use]
    pub const fn from_cursor(cursor: OriginCursor) -> Self {
        Self {
            origin_seq: cursor.origin_seq,
            last_mutation_id: Some(cursor.last_mutation_id),
        }
    }

    /// Whether `candidate` continues this anchor's chain immediately.
    #[must_use]
    pub fn is_next(self, candidate: &MutationCandidate) -> bool {
        candidate.origin_seq == self.origin_seq.saturating_add(1)
            && candidate.previous_mutation_id == self.last_mutation_id
    }
}

/// A final, typed deletion is permanent for this object identity.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FinalTombstone {
    pub object: ObjectKey,
    pub version: ObjectVersion,
    pub mutation_id: MutationId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TombstoneDecisionError {
    ObjectAlreadyDeleted,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum SyncSessionState {
    Disabled,
    Ready,
    AwaitingPairing,
    Pairing,
    Enrolling,
    Synchronizing,
    Paused,
    Offline,
    Failed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyncSessionEvent {
    Enable,
    PairingRequired,
    BeginPairing,
    PairingAccepted,
    BeginEnrollment,
    EnrollmentComplete,
    Pause,
    Resume,
    HostUnavailable,
    Reconnected,
    Retry,
    Failure,
    Cancel,
    Disable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidSyncTransition {
    pub from: SyncSessionState,
    pub event: SyncSessionEvent,
}

/// Pure session transition function. Cancelling a network/session transition
/// never rolls back a mutation already committed to the local database.
///
/// # Errors
/// Returns the state and event when the transition is unsupported.
pub const fn transition_sync_session(
    state: SyncSessionState,
    event: SyncSessionEvent,
) -> Result<SyncSessionState, InvalidSyncTransition> {
    use SyncSessionEvent as E;
    use SyncSessionState as S;
    let next = match (state, event) {
        (S::Disabled, E::Enable)
        | (
            S::AwaitingPairing
            | S::Pairing
            | S::Enrolling
            | S::Synchronizing
            | S::Paused
            | S::Offline,
            E::Cancel,
        ) => S::Ready,
        (S::Ready, E::PairingRequired) => S::AwaitingPairing,
        (S::AwaitingPairing | S::Ready, E::BeginPairing) => S::Pairing,
        (S::Ready, E::BeginEnrollment) | (S::Pairing, E::PairingAccepted) => S::Enrolling,
        (S::Enrolling, E::EnrollmentComplete)
        | (S::Paused, E::Resume)
        | (S::Offline, E::Reconnected)
        | (S::Failed, E::Retry) => S::Synchronizing,
        (S::Synchronizing | S::Offline, E::Pause) => S::Paused,
        (S::Synchronizing | S::Paused, E::HostUnavailable) => S::Offline,
        (S::Pairing | S::Enrolling | S::Synchronizing | S::Paused | S::Offline, E::Failure) => {
            S::Failed
        }
        (
            S::Ready
            | S::AwaitingPairing
            | S::Pairing
            | S::Enrolling
            | S::Synchronizing
            | S::Paused
            | S::Offline
            | S::Failed,
            E::Disable,
        ) => S::Disabled,
        _ => return Err(InvalidSyncTransition { from: state, event }),
    };
    Ok(next)
}

/// Reject stale updates for the deleted typed identity. Reimport uses a newly
/// allocated object key and therefore cannot accidentally revive this record.
///
/// # Errors
/// Returns an error if the candidate targets the finally deleted identity.
pub fn check_candidate_against_tombstone(
    candidate: &MutationCandidate,
    tombstone: Option<&FinalTombstone>,
) -> Result<(), TombstoneDecisionError> {
    if tombstone.is_some_and(|deleted| deleted.object == candidate.object) {
        return Err(TombstoneDecisionError::ObjectAlreadyDeleted);
    }
    Ok(())
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConflictCandidate {
    pub object: ObjectKey,
    pub replaced_version: ObjectVersion,
    pub replaced_payload: Value,
    pub accepted_mutation_id: MutationId,
    pub accepted_payload: Value,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Confirmation {
    pub object: ConfirmedObject,
    pub origin_cursor: OriginCursor,
    pub conflict: Option<ConflictCandidate>,
    /// Immutable identity mappings created while confirming this event.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub aliases: Vec<PlaylistMemberAlias>,
    /// Change time of the winning ordinary fields, never the host receive time.
    #[serde(default)]
    pub changed_at_ms: u64,
    /// Resources and ordinary fields follow the same winning library value.
    #[serde(default)]
    pub candidate_wins: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConfirmationError {
    WrongObject,
    WrongOrigin,
    UnauthorizedPlayStatsBucket,
    OriginSequenceExhausted,
    OutOfOrderPredecessor,
    NonMonotoneHostSequence,
    HostSequenceExhausted,
}

fn merge_playlist_append(
    current: Option<&ConfirmedObject>,
    playlist_id: PlaylistId,
    proposed: &[Value],
) -> Result<(Value, Vec<PlaylistMemberAlias>), ConfirmationError> {
    let mut combined = current
        .and_then(|value| value.payload.get("members"))
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut songs = std::collections::BTreeMap::<String, String>::new();
    for member in &combined {
        let song = member
            .get("song_uuid")
            .and_then(Value::as_str)
            .ok_or(ConfirmationError::WrongObject)?;
        let id = member
            .get("member_uuid")
            .and_then(Value::as_str)
            .ok_or(ConfirmationError::WrongObject)?;
        songs
            .entry(song.to_owned())
            .or_insert_with(|| id.to_owned());
    }
    let mut aliases = Vec::new();
    for member in proposed {
        let song = member
            .get("song_uuid")
            .and_then(Value::as_str)
            .ok_or(ConfirmationError::WrongObject)?;
        let id = member
            .get("member_uuid")
            .and_then(Value::as_str)
            .ok_or(ConfirmationError::WrongObject)?;
        if let Some(canonical) = songs.get(song) {
            if canonical != id {
                aliases.push(PlaylistMemberAlias {
                    playlist_id,
                    alias_member_id: PlaylistItemId::from_uuid(
                        Uuid::parse_str(id).map_err(|_| ConfirmationError::WrongObject)?,
                    ),
                    canonical_member_id: PlaylistItemId::from_uuid(
                        Uuid::parse_str(canonical).map_err(|_| ConfirmationError::WrongObject)?,
                    ),
                });
            }
        } else {
            songs.insert(song.to_owned(), id.to_owned());
            combined.push(member.clone());
        }
    }
    for (position, member) in combined.iter_mut().enumerate() {
        let position =
            u64::try_from(position).map_err(|_| ConfirmationError::HostSequenceExhausted)?;
        member["position"] = Value::from(position);
    }
    Ok((Value::Array(combined), aliases))
}

/// One decoded play-stats payload. The payload is the full
/// [`super::library::PlayStatsRecord`], so both fields are always present.
struct DecodedPlayStats {
    declared_song: Uuid,
    claimant: Uuid,
    buckets: std::collections::BTreeMap<Uuid, u64>,
}

/// Decode one play-stats payload.
///
/// The payload is the full [`super::library::PlayStatsRecord`]: only that shape
/// attests `updated_by_device_id`, so anything else is refused.
fn decode_play_stats(payload: &Value) -> Result<DecodedPlayStats, ConfirmationError> {
    let record: super::library::PlayStatsRecord =
        serde_json::from_value(payload.clone()).map_err(|_| ConfirmationError::WrongObject)?;
    Ok(DecodedPlayStats {
        declared_song: record.song_uuid,
        claimant: record.updated_by_device_id.as_uuid(),
        buckets: record.by_device,
    })
}

fn merge_authorized_play_stats(
    current: Option<&ConfirmedObject>,
    candidate: &MutationCandidate,
    song: SongId,
) -> Result<Value, ConfirmationError> {
    let proposed = decode_play_stats(&candidate.payload)?;
    if proposed.declared_song != song.as_uuid() {
        return Err(ConfirmationError::WrongObject);
    }
    if proposed.claimant != candidate.origin_device_id.as_uuid() {
        return Err(ConfirmationError::WrongOrigin);
    }
    // A confirmed row written by an earlier release only recorded buckets; it
    // still authorizes exactly the buckets it recorded, and nothing else.
    let previous = current.and_then(|value| decode_play_stats(&value.payload).ok());
    for (device, count) in &proposed.buckets {
        if *device == candidate.origin_device_id.as_uuid() {
            continue;
        }
        let confirmed = previous
            .as_ref()
            .and_then(|decoded| decoded.buckets.get(device))
            .copied()
            .unwrap_or(0);
        if *count > confirmed {
            return Err(ConfirmationError::UnauthorizedPlayStatsBucket);
        }
    }
    let mut merged = previous.map(|decoded| decoded.buckets).unwrap_or_default();
    for (device, count) in proposed.buckets {
        merged
            .entry(device)
            .and_modify(|confirmed| *confirmed = (*confirmed).max(count))
            .or_insert(count);
    }
    serde_json::to_value(merged).map_err(|_| ConfirmationError::WrongObject)
}

/// Extract the wall-clock change time from a legacy portable record. Its HLC
/// counter orders edits on one device and is not a cross-device time tie-break.
#[must_use]
pub fn payload_changed_at_ms(payload: &Value) -> Option<u64> {
    payload
        .get("hlc")?
        .get("wall_secs")?
        .as_u64()?
        .checked_mul(1000)
}

/// Confirm an event while resolving ordinary library fields by change time.
/// A timestamp tie retains the host's current value. Accepted events always
/// consume the origin chain and receive a canonical commit, including losers;
/// this lets every replica replace its optimistic value with the same winner.
/// Playlist appends and per-device counters keep their independent merge rules.
///
/// # Errors
/// Rejects mismatched objects or origins, invalid predecessor order, and
/// exhausted or nonmonotone sequences.
#[allow(clippy::too_many_lines)]
pub fn confirm_candidate_at(
    current: Option<&ConfirmedObject>,
    cursor: Option<OriginCursor>,
    candidate: &MutationCandidate,
    commit_seq: CommitSeq,
    changed_at_ms: u64,
    current_changed_at_ms: u64,
) -> Result<Confirmation, ConfirmationError> {
    if current.is_some_and(|value| value.object != candidate.object) {
        return Err(ConfirmationError::WrongObject);
    }
    if current.is_some_and(|value| commit_seq <= value.version.commit_seq()) {
        return Err(ConfirmationError::NonMonotoneHostSequence);
    }
    let expected = match cursor {
        None => (1, None),
        Some(cursor) => {
            if cursor.origin_device_id != candidate.origin_device_id {
                return Err(ConfirmationError::WrongOrigin);
            }
            (
                cursor
                    .origin_seq
                    .checked_add(1)
                    .ok_or(ConfirmationError::OriginSequenceExhausted)?,
                Some(cursor.last_mutation_id),
            )
        }
    };
    if candidate.origin_seq != expected.0 || candidate.previous_mutation_id != expected.1 {
        return Err(ConfirmationError::OutOfOrderPredecessor);
    }
    let is_tombstone = candidate.payload == serde_json::json!({"tombstone": true});
    // A typed deletion is not a field-level value that a newer clock could
    // outvote: it terminates the object identity. Resurrection is already
    // answered by `check_candidate_against_tombstone`, so arbitration here only
    // decides ordinary fields, where a tie keeps the host's current value.
    let candidate_wins = is_tombstone || current.is_none() || changed_at_ms > current_changed_at_ms;
    let mut confirmed_payload = match current {
        Some(existing) if !candidate_wins => existing.payload.clone(),
        _ => candidate.payload.clone(),
    };
    if matches!(candidate.object, ObjectKey::Song(_)) {
        if let Some(existing) = current {
            let other_payload = if candidate_wins {
                &existing.payload
            } else {
                &candidate.payload
            };
            merge_song_derived_audio_facts(&mut confirmed_payload, other_payload);
        }
    }
    let aliases = if is_tombstone {
        Vec::new()
    } else if let ObjectKey::Playlist(playlist_id) = candidate.object {
        let proposed_members = candidate
            .payload
            .get("members")
            .and_then(Value::as_array)
            .cloned()
            .ok_or(ConfirmationError::WrongObject)?;
        let (members, member_aliases) =
            merge_playlist_append(current, playlist_id, &proposed_members)?;
        confirmed_payload["members"] = members;
        member_aliases
    } else {
        Vec::new()
    };
    if !is_tombstone {
        if let ObjectKey::PlayStats(song) = candidate.object {
            confirmed_payload["by_device"] = merge_authorized_play_stats(current, candidate, song)?;
        }
    }
    let version = ObjectVersion::from_commit_seq(commit_seq);
    let conflict = current.and_then(|old| {
        let stale_base = candidate.base_object_version != Some(old.version);
        let losing_payload = if candidate_wins {
            &old.payload
        } else {
            &candidate.payload
        };
        let changed_value = match candidate.object {
            // Revisions and HLCs advance as events are confirmed. They do not
            // make a user-visible favorite conflict when both devices chose
            // the same value.
            ObjectKey::Favorite(_) => {
                losing_payload.get("is_favorite") != confirmed_payload.get("is_favorite")
            }
            // Play statistics are merged by device. Compare the merged
            // counters, not the stale submitted snapshot or record metadata.
            ObjectKey::PlayStats(_) => {
                losing_payload.get("by_device") != confirmed_payload.get("by_device")
            }
            ObjectKey::Song(_) => !same_song_user_data(losing_payload, &confirmed_payload),
            _ => losing_payload != &confirmed_payload,
        };
        ((stale_base || !candidate_wins)
            && old.origin_device_id != candidate.origin_device_id
            && changed_value)
            .then(|| {
                ConflictCandidate {
                    object: old.object,
                    replaced_version: old.version,
                    replaced_payload: losing_payload.clone(),
                    accepted_mutation_id: candidate.mutation_id,
                    // The accepted value is the merged canonical object, not the
                    // submitted sketch: a playlist append is merged against the
                    // confirmed membership, and a recovery UI that echoed the raw
                    // candidate would silently drop every member it never saw.
                    accepted_payload: confirmed_payload.clone(),
                }
            })
    });
    Ok(Confirmation {
        object: ConfirmedObject {
            object: candidate.object,
            version,
            // A losing candidate keeps the confirmed author that already owns
            // the surviving value; only a winning candidate re-authors it.
            origin_device_id: match current {
                Some(existing) if !candidate_wins => existing.origin_device_id,
                _ => candidate.origin_device_id,
            },
            payload: confirmed_payload,
        },
        origin_cursor: OriginCursor {
            origin_device_id: candidate.origin_device_id,
            origin_seq: candidate.origin_seq,
            last_mutation_id: candidate.mutation_id,
        },
        conflict,
        aliases,
        changed_at_ms: if candidate_wins {
            changed_at_ms
        } else {
            current_changed_at_ms
        },
        candidate_wins,
    })
}

/// Local reversible delete intent. The final mutation identity is allocated
/// once with the intent, but is not uploadable until the deadline expires.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LocalDeleteIntent {
    pub intent_id: MutationId,
    pub final_mutation_id: MutationId,
    pub object: ObjectKey,
    pub created_at_ms: u64,
    pub undo_deadline_ms: u64,
    pub state: DeleteIntentState,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum DeleteIntentState {
    Open,
    Cancelled,
    Finalized,
    SupersededByRemoteTombstone,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeleteIntentAction {
    Undo,
    DeadlineReached,
    RemoteFinalTombstone,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeleteIntentOutcome {
    Pending,
    Cancelled,
    EmitFinalTombstone {
        mutation_id: MutationId,
        object: ObjectKey,
    },
    RemovedByRemoteTombstone,
    AlreadyFinalized,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DeleteIntentError {
    InvalidDeadline,
    UndoWindowExpired,
    AlreadyResolved,
}

impl LocalDeleteIntent {
    pub const UNDO_WINDOW_MS: u64 = 10_000;

    /// Allocate a reversible delete intent with its final mutation identity.
    ///
    /// # Errors
    /// Returns an error if adding the undo window overflows the timestamp.
    pub fn begin(object: ObjectKey, created_at_ms: u64) -> Result<Self, DeleteIntentError> {
        let undo_deadline_ms = created_at_ms
            .checked_add(Self::UNDO_WINDOW_MS)
            .ok_or(DeleteIntentError::InvalidDeadline)?;
        Ok(Self {
            intent_id: MutationId::new(),
            final_mutation_id: MutationId::new(),
            object,
            created_at_ms,
            undo_deadline_ms,
            state: DeleteIntentState::Open,
        })
    }

    /// Resolve one transition while the caller holds the serialized local
    /// write transaction. Repeated finalization returns the same event ID.
    ///
    /// # Errors
    /// Rejects undo after the deadline and transitions on a cancelled intent.
    pub fn resolve(
        &mut self,
        action: DeleteIntentAction,
        now_ms: u64,
    ) -> Result<DeleteIntentOutcome, DeleteIntentError> {
        match self.state {
            DeleteIntentState::Cancelled => return Err(DeleteIntentError::AlreadyResolved),
            DeleteIntentState::SupersededByRemoteTombstone => {
                return Ok(DeleteIntentOutcome::RemovedByRemoteTombstone)
            }
            DeleteIntentState::Finalized => {
                return Ok(DeleteIntentOutcome::EmitFinalTombstone {
                    mutation_id: self.final_mutation_id,
                    object: self.object,
                });
            }
            DeleteIntentState::Open => {}
        }

        if action == DeleteIntentAction::RemoteFinalTombstone {
            self.state = DeleteIntentState::SupersededByRemoteTombstone;
            return Ok(DeleteIntentOutcome::RemovedByRemoteTombstone);
        }
        if action == DeleteIntentAction::Undo {
            if now_ms >= self.undo_deadline_ms {
                return Err(DeleteIntentError::UndoWindowExpired);
            }
            self.state = DeleteIntentState::Cancelled;
            return Ok(DeleteIntentOutcome::Cancelled);
        }
        if now_ms < self.undo_deadline_ms {
            return Ok(DeleteIntentOutcome::Pending);
        }
        self.state = DeleteIntentState::Finalized;
        Ok(DeleteIntentOutcome::EmitFinalTombstone {
            mutation_id: self.final_mutation_id,
            object: self.object,
        })
    }
}

/// Typed comparison prevents accidentally comparing positions from distinct
/// authority histories.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SequenceRelation {
    Before,
    Equal,
    After,
}

#[must_use]
pub fn compare_in_binding(
    left_binding: SyncBinding,
    left: CommitSeq,
    right_binding: SyncBinding,
    right: CommitSeq,
) -> Option<SequenceRelation> {
    if left_binding.library_id.as_uuid() != right_binding.library_id.as_uuid()
        || left_binding.authority_id.as_uuid() != right_binding.authority_id.as_uuid()
        || left_binding.epoch.as_u64() != right_binding.epoch.as_u64()
    {
        return None;
    }
    Some(match left.as_u64().cmp(&right.as_u64()) {
        std::cmp::Ordering::Less => SequenceRelation::Before,
        std::cmp::Ordering::Greater => SequenceRelation::After,
        std::cmp::Ordering::Equal => SequenceRelation::Equal,
    })
}
