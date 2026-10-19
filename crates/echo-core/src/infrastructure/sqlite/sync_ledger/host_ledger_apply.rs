//! Helper fns behind [`super::host_ledger`] (commit encoding, outcome records, cursor recovery).

#[allow(clippy::wildcard_imports)]
use super::*;

pub(super) fn ensure_origin_authorized(
    tx: &rusqlite::Transaction<'_>,
    origin: &str,
    library: &str,
) -> Result<(), Error> {
    let local_device: String = tx
        .query_row(
            "SELECT value FROM device_state WHERE key = 'device_id'",
            [],
            |row| row.get(0),
        )
        .map_err(storage)?;
    let authorized = local_device == origin || tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_peers WHERE device_id=?1 AND library_id=?2 AND revoked_at IS NULL)",
        params![origin, library],
        |row| row.get::<_, bool>(0),
    ).map_err(storage)?;
    if authorized {
        Ok(())
    } else {
        Err(Error::conflict(
            "origin device is not authorized for this library",
        ))
    }
}

/// One confirmed-object row: version, commit sequence, payload, author, the
/// tombstone flag, the confirming mutation, and the winning change time.
type ConfirmedObjectRow = (i64, i64, Option<String>, String, bool, String, i64);

pub(super) fn persist_member_aliases(
    tx: &rusqlite::Transaction<'_>,
    aliases: &[crate::domain::sync::PlaylistMemberAlias],
    sequence: i64,
) -> Result<(), Error> {
    for alias in aliases {
        let playlist = alias.playlist_id.to_string();
        let alias_id = alias.alias_member_id.to_string();
        let canonical_id = alias.canonical_member_id.to_string();
        let prior: Option<String> = tx.query_row(
            "SELECT canonical_member_uuid FROM sync_member_aliases WHERE playlist_uuid=?1 AND alias_member_uuid=?2",
            params![playlist, alias_id],
            |row| row.get(0),
        ).optional().map_err(storage)?;
        if prior.as_deref().is_some_and(|value| value != canonical_id) {
            return Err(Error::conflict(
                "playlist member alias already has a different canonical identity",
            ));
        }
        let aliases_target_alias: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_member_aliases WHERE playlist_uuid=?1 AND alias_member_uuid=?2)",
            params![playlist, canonical_id],
            |row| row.get(0),
        ).map_err(storage)?;
        if aliases_target_alias {
            return Err(Error::conflict(
                "playlist member alias target is not canonical",
            ));
        }
        tx.execute(
            "INSERT OR IGNORE INTO sync_member_aliases(playlist_uuid,alias_member_uuid,canonical_member_uuid,commit_seq) VALUES(?1,?2,?3,?4)",
            params![playlist, alias_id, canonical_id, sequence],
        ).map_err(storage)?;
    }
    Ok(())
}

pub(super) fn prior_confirmation(
    tx: &rusqlite::Transaction<'_>,
    origin: &str,
    candidate: &MutationCandidate,
    payload_digest: &str,
) -> Result<Option<StagedConfirmation>, Error> {
    let prior: Option<(String, String, Option<i64>)> = tx.query_row(
        "SELECT payload_digest, outcome_json, commit_seq FROM sync_inbox WHERE origin_device_id=?1 AND mutation_id=?2",
        params![origin, candidate.mutation_id.as_uuid().to_string()],
        |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
    ).optional().map_err(storage)?;
    let Some((digest, outcome, commit_seq)) = prior else {
        return Ok(None);
    };
    let staged: StagedConfirmation = serde_json::from_str(&outcome).map_err(storage)?;
    // A predecessor rejection is not an outcome: the event never entered the
    // ledger. The sender re-anchors its chain onto this host's cursor, which
    // changes the envelope (`origin_seq`/`previous_mutation_id`) but never the
    // event payload, so identity reuse must be decided only for events the
    // ledger actually recorded. Checking the digest first would reject the
    // repair as "same identity, different content".
    let retryable_predecessor_rejection =
        commit_seq.is_none() && staged.outcome_json.contains("OutOfOrderPredecessor");
    if retryable_predecessor_rejection {
        return Ok(None);
    }
    if digest != payload_digest {
        return Err(Error::conflict(
            "mutation identity was reused with different content",
        ));
    }
    if commit_seq.is_none() && terminal_rejection_consumes_origin_json(&staged.outcome_json) {
        let candidate = normalize_member_alias_object(tx, candidate.clone())?;
        let cursor = load_origin_cursor(tx, candidate.origin_device_id)?;
        advance_origin_cursor_for_terminal_rejection(tx, origin, &candidate, cursor)?;
    }
    if let Some(sequence) = commit_seq {
        ensure_no_pending_predecessor(tx, Some(sequence))?;
    }
    Ok(Some(staged))
}

/// Reject malformed event shapes before any identity work: shared resources are
/// only meaningful for a live object, never for a typed deletion.
pub(super) fn validate_candidate_payload(candidate: &MutationCandidate) -> Result<(), Error> {
    super::super::sync_manifests::validate(&candidate.resource_refs)?;
    if candidate.payload == serde_json::json!({"tombstone": true})
        && !candidate.resource_refs.is_empty()
    {
        return Err(Error::conflict(
            "deleted objects cannot declare resource references",
        ));
    }
    Ok(())
}

/// A confirmed object that is already tombstoned can never be resurrected, so
/// the acceptance is downgraded to a final rejection. It still consumes a
/// correctly chained origin event: otherwise every later event from that device
/// waits forever behind an object that will never come back.
pub(super) fn stage_already_deleted(
    tx: &Connection,
    origin: &str,
    candidate: &MutationCandidate,
    payload_digest: &str,
    confirmed: &Confirmation,
) -> Result<StagedConfirmation, Error> {
    let staged = StagedConfirmation {
        mutation_id: candidate.mutation_id,
        outcome_json: "{\"accepted\":false,\"reason\":\"object_already_deleted\"}".to_owned(),
        checkpoint: None,
    };
    tx.execute(
        "INSERT INTO sync_origin_cursors(origin_device_id, origin_seq, last_mutation_id) VALUES(?1, ?2, ?3) ON CONFLICT(origin_device_id) DO UPDATE SET origin_seq=excluded.origin_seq, last_mutation_id=excluded.last_mutation_id",
        params![origin, i64::try_from(confirmed.origin_cursor.origin_seq).map_err(storage)?, candidate.mutation_id.as_uuid().to_string()],
    )
    .map_err(storage)?;
    record_inbox_outcome(tx, origin, candidate, payload_digest, &staged, None)?;
    Ok(staged)
}

/// Record the durable outcome for one mutation. Rejections keep `commit_seq`
/// NULL: they are remembered so retries stay idempotent but never become a
/// published commit.
pub(super) fn record_inbox_outcome(
    tx: &Connection,
    origin: &str,
    candidate: &MutationCandidate,
    payload_digest: &str,
    staged: &StagedConfirmation,
    commit_seq: Option<i64>,
) -> Result<(), Error> {
    let serialized = serde_json::to_string(staged).map_err(storage)?;
    tx.execute(
        "INSERT INTO sync_inbox(origin_device_id, mutation_id, payload_digest, outcome_json, commit_seq, received_at) VALUES(?1, ?2, ?3, ?4, ?5, ?6) ON CONFLICT(origin_device_id, mutation_id) DO UPDATE SET outcome_json=excluded.outcome_json, commit_seq=excluded.commit_seq, received_at=excluded.received_at",
        params![origin, candidate.mutation_id.as_uuid().to_string(), payload_digest, serialized, commit_seq, now_ms()],
    )
    .map_err(storage)?;
    Ok(())
}

/// The outcome body one rejection carries back to the replica.
///
/// A predecessor rejection is the only retryable one, and it is the only place
/// a replica can learn why its chain stopped fitting the host's ledger. Naming
/// the host's current position lets the sender re-anchor instead of retrying a
/// sequence the host will never accept.
pub(super) fn rejection_outcome_json(
    rejection: ConfirmationError,
    cursor: Option<OriginCursor>,
) -> String {
    if rejection != ConfirmationError::OutOfOrderPredecessor {
        return format!("{{\"accepted\":false,\"reason\":\"{rejection:?}\"}}");
    }
    let anchor = cursor.map_or(OriginAnchor::GENESIS, OriginAnchor::from_cursor);
    serde_json::json!({
        "accepted": false,
        "reason": "OutOfOrderPredecessor",
        "origin_seq": anchor.origin_seq,
        "last_mutation_id": anchor.last_mutation_id,
    })
    .to_string()
}

pub(super) const fn terminal_rejection_consumes_origin(rejection: ConfirmationError) -> bool {
    matches!(
        rejection,
        ConfirmationError::WrongObject
            | ConfirmationError::WrongOrigin
            | ConfirmationError::UnauthorizedPlayStatsBucket
    )
}

pub(super) fn terminal_rejection_consumes_origin_json(outcome_json: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(outcome_json)
        .ok()
        .and_then(|value| value.get("reason")?.as_str().map(str::to_owned))
        .is_some_and(|reason| {
            matches!(
                reason.as_str(),
                "WrongObject" | "WrongOrigin" | "UnauthorizedPlayStatsBucket"
            )
        })
}

/// Repair the one-event cursor gap left by older releases that acknowledged a
/// terminal rejection locally but did not advance the host cursor. The current
/// candidate must name that exact rejected inbox event as its immediate
/// predecessor; out-of-order and accepted inbox rows are never consumed here.
pub(super) fn recover_adjacent_terminal_predecessor(
    tx: &rusqlite::Transaction<'_>,
    origin: &str,
    candidate: &MutationCandidate,
    cursor: Option<OriginCursor>,
) -> Result<Option<OriginCursor>, Error> {
    let expected = match cursor {
        Some(value) => match value.origin_seq.checked_add(1) {
            Some(expected) => expected,
            None => return Ok(cursor),
        },
        None => 1,
    };
    let Some(successor_sequence) = expected.checked_add(1) else {
        return Ok(cursor);
    };
    if candidate.origin_seq != successor_sequence {
        return Ok(cursor);
    }
    let Some(predecessor) = candidate.previous_mutation_id else {
        return Ok(cursor);
    };
    let prior: Option<(String, Option<i64>)> = tx
        .query_row(
            "SELECT outcome_json, commit_seq FROM sync_inbox WHERE origin_device_id=?1 AND mutation_id=?2",
            params![origin, predecessor.as_uuid().to_string()],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(storage)?;
    let Some((outcome_json, commit_seq)) = prior else {
        return Ok(cursor);
    };
    let prior: StagedConfirmation = serde_json::from_str(&outcome_json).map_err(storage)?;
    if commit_seq.is_some() || !terminal_rejection_consumes_origin_json(&prior.outcome_json) {
        return Ok(cursor);
    }

    tx.execute(
        "INSERT INTO sync_origin_cursors(origin_device_id, origin_seq, last_mutation_id) VALUES(?1, ?2, ?3) ON CONFLICT(origin_device_id) DO UPDATE SET origin_seq=excluded.origin_seq, last_mutation_id=excluded.last_mutation_id",
        params![origin, i64::try_from(expected).map_err(storage)?, predecessor.as_uuid().to_string()],
    )
    .map_err(storage)?;
    Ok(Some(OriginCursor {
        origin_device_id: candidate.origin_device_id,
        origin_seq: expected,
        last_mutation_id: predecessor,
    }))
}

pub(super) fn advance_origin_cursor_for_terminal_rejection(
    tx: &rusqlite::Transaction<'_>,
    origin: &str,
    candidate: &MutationCandidate,
    cursor: Option<OriginCursor>,
) -> Result<(), Error> {
    let expected_seq = cursor.map_or(1, |value| value.origin_seq.saturating_add(1));
    let expected_previous = cursor.map(|value| value.last_mutation_id);
    if candidate.origin_seq == expected_seq && candidate.previous_mutation_id == expected_previous {
        tx.execute(
            "INSERT INTO sync_origin_cursors(origin_device_id, origin_seq, last_mutation_id) VALUES(?1, ?2, ?3) ON CONFLICT(origin_device_id) DO UPDATE SET origin_seq=excluded.origin_seq, last_mutation_id=excluded.last_mutation_id",
            params![origin, i64::try_from(candidate.origin_seq).map_err(storage)?, candidate.mutation_id.as_uuid().to_string()],
        )
        .map_err(storage)?;
    }
    Ok(())
}

pub(super) fn ensure_no_pending_predecessor(
    tx: &rusqlite::Transaction<'_>,
    before_sequence: Option<i64>,
) -> Result<(), Error> {
    let pending: bool = before_sequence.map_or_else(
        || tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_commits WHERE publication_state='pending')",
            [],
            |row| row.get(0),
        ),
        |sequence| tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM sync_commits WHERE publication_state='pending' AND commit_seq<?1)",
            params![sequence],
            |row| row.get(0),
        ),
    ).map_err(storage)?;
    if pending {
        Err(Error::unavailable(
            "sync publication",
            "publish the pending predecessor before confirming this mutation",
        ))
    } else {
        Ok(())
    }
}

pub(super) fn object_identity(object: ObjectKey) -> (&'static str, String) {
    match object {
        ObjectKey::Song(id) => ("song", id.to_string()),
        ObjectKey::Playlist(id) => ("playlist", id.to_string()),
        ObjectKey::PlaylistItem(id) => ("playlist-item", id.to_string()),
        ObjectKey::LibraryRoot(id) => ("library-root", id.to_string()),
        ObjectKey::Favorite(id) => ("favorite", id.to_string()),
        ObjectKey::Override(id) => ("override", id.to_string()),
        ObjectKey::PlayStats(id) => ("play-stats", id.to_string()),
    }
}

pub(super) fn persist_confirmed_object(
    tx: &rusqlite::Transaction<'_>,
    candidate: &MutationCandidate,
    object: &ConfirmedObject,
    next: i64,
    is_tombstone: bool,
    local_root_uuid: Option<&str>,
    changed_at_ms: u64,
) -> Result<(), Error> {
    let origin = candidate.origin_device_id.as_uuid().to_string();
    let (kind, object_uuid) = object_identity(object.object);
    let version = i64::try_from(object.version.commit_seq().as_u64()).map_err(storage)?;
    let changed_at = i64::try_from(changed_at_ms).unwrap_or(i64::MAX);
    tx.execute("INSERT INTO sync_object_versions(object_type, object_uuid, object_version, commit_seq, payload_json, is_tombstone, changed_at_ms) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7) ON CONFLICT(object_type, object_uuid) DO UPDATE SET object_version=excluded.object_version, commit_seq=excluded.commit_seq, payload_json=excluded.payload_json, is_tombstone=excluded.is_tombstone, changed_at_ms=excluded.changed_at_ms", params![kind, object_uuid, version, next, if is_tombstone { None } else { Some(serde_json::to_string(&object.payload).map_err(storage)?) }, i64::from(is_tombstone), changed_at]).map_err(storage)?;
    tx.execute(
                "UPDATE sync_mutations SET status='confirmed', outcome_json=?3 WHERE origin_device_id=?1 AND mutation_id=?2 AND status='pending'",
                params![origin, candidate.mutation_id.as_uuid().to_string(), "{\"accepted\":true}"],
            )
            .map_err(storage)?;
    if is_tombstone {
        super::super::sync_projection::project_confirmed_tombstone(tx, kind, &object_uuid)?;
    } else {
        super::super::sync_projection::project_confirmed_song_state(
            tx,
            kind,
            &object_uuid,
            &object.payload,
            local_root_uuid,
        )?;
    }
    Ok(())
}

pub(super) fn encode_commit(
    binding: SyncBinding,
    candidate: &MutationCandidate,
    confirmed_payload: &serde_json::Value,
    sequence: CommitSeq,
    previous_digest: [u8; 32],
    aliases: &[crate::domain::sync::PlaylistMemberAlias],
    song_alias: Option<&(String, String)>,
) -> Result<(String, Vec<u8>), Error> {
    let mut confirmed_candidate = candidate.clone();
    // The submitted mutation remains immutable in the sender's outbox. The
    // host log records its deterministic canonical result for every replica.
    confirmed_candidate.payload = confirmed_payload.clone();
    let commit_body = WireCommitBody {
        protocol_version: 1,
        binding,
        commit_seq: sequence,
        previous_digest,
        mutation: WireMutationBody {
            protocol_version: 1,
            binding,
            origin_device_id: candidate.origin_device_id,
            mutation_id: candidate.mutation_id,
            origin_seq: candidate.origin_seq,
            previous_mutation_id: candidate.previous_mutation_id,
            object: candidate.object,
            base_object_version: candidate.base_object_version,
            payload: confirmed_candidate.payload,
            resource_refs: candidate
                .resource_refs
                .iter()
                .map(super::super::sync_manifests::WireResource::from)
                .collect(),
        },
        aliases: aliases
            .iter()
            .map(|alias| {
                serde_json::json!({"kind":"playlist_member","alias":{
                    "playlist_id":alias.playlist_id,
                    "alias":alias.alias_member_id,
                    "canonical":alias.canonical_member_id
                }})
            })
            .chain(song_alias.into_iter().map(|(alias, canonical)| serde_json::json!({"kind":"song","alias":{"alias":alias,"canonical":canonical}})))
            .collect(),
        resource_refs: candidate
            .resource_refs
            .iter()
            .map(super::super::sync_manifests::WireResource::from)
            .collect(),
    };
    let body = serde_json::to_vec(&commit_body).map_err(storage)?;
    let body_len = u32::try_from(body.len()).map_err(storage)?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"echo.lan.commit.v1\0");
    hasher.update(&1u16.to_be_bytes());
    hasher.update(&sequence.as_u64().to_be_bytes());
    hasher.update(&previous_digest);
    hasher.update(&body_len.to_be_bytes());
    hasher.update(&body);
    let commit_digest = hasher.finalize().as_bytes().to_vec();
    let canonical_json = String::from_utf8(body).map_err(storage)?;
    Ok((canonical_json, commit_digest))
}

/// Read the confirmed state for the candidate's typed identity.
///
/// The tombstone question is answered by the domain predicate rather than by a
/// local boolean, so the ledger cannot drift from the deletion contract. The
/// tombstone and the live object are mutually exclusive, so one read serves
/// both answers.
pub(super) fn load_confirmed_object(
    tx: &rusqlite::Transaction<'_>,
    candidate: &MutationCandidate,
) -> Result<(bool, Option<ConfirmedObject>, u64), Error> {
    let object = candidate.object;
    let (kind, object_uuid) = object_identity(object);
    let current_row: Option<ConfirmedObjectRow> = tx.query_row(
                "SELECT v.object_version, v.commit_seq, v.payload_json, c.origin_device_id, v.is_tombstone, c.mutation_id, v.changed_at_ms FROM sync_object_versions v JOIN sync_commits c ON c.commit_seq=v.commit_seq WHERE v.object_type = ?1 AND v.object_uuid = ?2",
                params![kind, object_uuid],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?, row.get(5)?, row.get(6)?)),
            ).optional().map_err(storage)?;
    let current_changed_at_ms = current_row
        .as_ref()
        .map_or(0, |row| u64::try_from(row.6).unwrap_or(0));
    let commit_version = |version: i64| -> Result<ObjectVersion, Error> {
        u64::try_from(version)
            .map(CommitSeq::from_u64)
            .map(ObjectVersion::from_commit_seq)
            .map_err(storage)
    };
    // The tombstone is the typed identity, not a boolean budget: the pure
    // decision function below owns "may this event still touch this object".
    let tombstone = current_row
        .as_ref()
        .filter(|row| row.4)
        .map(
            |(version, _sequence, _payload, _author, _tombstone, mutation_id, _changed_at)| -> Result<FinalTombstone, Error> {
                Ok(FinalTombstone {
                    object,
                    version: commit_version(*version)?,
                    mutation_id: MutationId::from_uuid(
                        uuid::Uuid::parse_str(mutation_id).map_err(storage)?,
                    ),
                })
            },
        )
        .transpose()?;
    let current = current_row
        .filter(|row| !row.4)
        .map(
            |(version, _sequence, payload, author, _tombstone, _mutation, _changed_at)| -> Result<ConfirmedObject, Error> {
                Ok(ConfirmedObject {
                    object,
                    version: commit_version(version)?,
                    origin_device_id: DeviceId::from_uuid(
                        uuid::Uuid::parse_str(&author).map_err(storage)?,
                    ),
                    payload: serde_json::from_str(payload.as_deref().ok_or_else(|| {
                        Error::InvariantViolation {
                            why: "live sync object has no payload".to_owned(),
                        }
                    })?)
                    .map_err(storage)?,
                })
            },
        )
        .transpose()?;
    let already_deleted = matches!(
        check_candidate_against_tombstone(candidate, tombstone.as_ref()),
        Err(TombstoneDecisionError::ObjectAlreadyDeleted)
    );
    Ok((already_deleted, current, current_changed_at_ms))
}

pub(super) fn exclude_final_playlist_members(
    tx: &rusqlite::Transaction<'_>,
    object: ObjectKey,
    current: Option<ConfirmedObject>,
) -> Result<Option<ConfirmedObject>, Error> {
    let ObjectKey::Playlist(playlist_id) = object else {
        return Ok(current);
    };
    let Some(mut current) = current else {
        return Ok(None);
    };
    let Some(members) = current
        .payload
        .get_mut("members")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return Ok(Some(current));
    };
    let mut live = Vec::with_capacity(members.len());
    for member in members.drain(..) {
        let member_id = member
            .get("member_uuid")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| Error::conflict("confirmed playlist member is missing its identity"))?;
        let song_id = member
            .get("song_uuid")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| Error::conflict("confirmed playlist member is missing its song"))?;
        let deleted: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_object_versions WHERE object_type='playlist-item' AND object_uuid=?1 AND is_tombstone=1) OR EXISTS(SELECT 1 FROM sync_object_versions WHERE object_type='song' AND object_uuid=?2 AND is_tombstone=1)",
                params![member_id, song_id],
                |row| row.get(0),
            )
            .map_err(storage)?;
        let is_historical_alias: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_member_aliases WHERE playlist_uuid=?1 AND alias_member_uuid=?2)",
                params![playlist_id.to_string(), member_id],
                |row| row.get(0),
            )
            .map_err(storage)?;
        if !deleted && !is_historical_alias {
            live.push(member);
        }
    }
    *members = live;
    Ok(Some(current))
}

pub(super) fn exclude_historical_member_aliases(
    tx: &rusqlite::Transaction<'_>,
    candidate: &MutationCandidate,
) -> Result<MutationCandidate, Error> {
    let ObjectKey::Playlist(playlist_id) = candidate.object else {
        return Ok(candidate.clone());
    };
    let mut normalized = candidate.clone();
    let Some(members) = normalized
        .payload
        .get_mut("members")
        .and_then(serde_json::Value::as_array_mut)
    else {
        return Ok(normalized);
    };
    let mut live = Vec::with_capacity(members.len());
    for member in members.drain(..) {
        let member_id = member
            .get("member_uuid")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| Error::conflict("playlist member is missing its identity"))?;
        let is_historical_alias: bool = tx
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sync_member_aliases WHERE playlist_uuid=?1 AND alias_member_uuid=?2)",
                params![playlist_id.to_string(), member_id],
                |row| row.get(0),
            )
            .map_err(storage)?;
        if !is_historical_alias {
            live.push(member);
        }
    }
    *members = live;
    Ok(normalized)
}

pub(super) fn normalize_member_alias_object(
    tx: &rusqlite::Transaction<'_>,
    candidate: MutationCandidate,
) -> Result<MutationCandidate, Error> {
    let ObjectKey::PlaylistItem(member_id) = candidate.object else {
        return Ok(candidate);
    };
    let mut statement = tx
        .prepare("SELECT canonical_member_uuid FROM sync_member_aliases WHERE alias_member_uuid=?1")
        .map_err(storage)?;
    let targets = statement
        .query_map(params![member_id.to_string()], |row| {
            row.get::<_, String>(0)
        })
        .map_err(storage)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(storage)?;
    if targets.len() > 1 {
        return Err(Error::conflict(
            "playlist member alias is ambiguous without a playlist identity",
        ));
    }
    let Some(target) = targets.first() else {
        return Ok(candidate);
    };
    let target = uuid::Uuid::parse_str(target).map_err(storage)?;
    Ok(MutationCandidate {
        object: ObjectKey::PlaylistItem(crate::domain::ids::PlaylistItemId::from_uuid(target)),
        ..candidate
    })
}

pub(super) fn load_origin_cursor(
    tx: &rusqlite::Transaction<'_>,
    device: DeviceId,
) -> Result<Option<OriginCursor>, Error> {
    let cursor_row: Option<(i64, String)> = tx.query_row(
                "SELECT origin_seq, last_mutation_id FROM sync_origin_cursors WHERE origin_device_id = ?1",
                params![device.as_uuid().to_string()],
                |row| Ok((row.get(0)?, row.get(1)?)),
            ).optional().map_err(storage)?;
    let cursor = cursor_row
        .map(|(seq, id)| -> Result<OriginCursor, Error> {
            Ok(OriginCursor {
                origin_device_id: device,
                origin_seq: u64::try_from(seq).map_err(storage)?,
                last_mutation_id: MutationId::from_uuid(
                    uuid::Uuid::parse_str(&id).map_err(storage)?,
                ),
            })
        })
        .transpose()?;
    Ok(cursor)
}

#[derive(Serialize)]
struct WireMutationBody {
    protocol_version: u16,
    binding: SyncBinding,
    origin_device_id: DeviceId,
    mutation_id: MutationId,
    origin_seq: u64,
    previous_mutation_id: Option<MutationId>,
    object: ObjectKey,
    base_object_version: Option<ObjectVersion>,
    payload: serde_json::Value,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    resource_refs: Vec<super::super::sync_manifests::WireResource>,
}

#[derive(Serialize)]
struct WireCommitBody {
    protocol_version: u16,
    binding: SyncBinding,
    commit_seq: CommitSeq,
    previous_digest: [u8; 32],
    mutation: WireMutationBody,
    aliases: Vec<serde_json::Value>,
    resource_refs: Vec<super::super::sync_manifests::WireResource>,
}
