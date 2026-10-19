//! Replica-apply layer: page validation + Core no-outbox apply boundary.

#[allow(clippy::wildcard_imports, unused_imports)]
use super::*;

pub(super) fn validate_transfer_progress(
    progress: &TransferProgress,
    resource: &crate::protocol::ResourceRefDto,
) -> Result<(), ClientError> {
    if crate::resource_transfer::validate_hash(&progress.upload_id).is_err()
        || progress.resource != *resource
        || progress.chunk_size != crate::protocol::MAX_RESOURCE_CHUNK_BYTES
        || resource.length > MAX_RESOURCE_BYTES
    {
        return Err(ClientError::InvalidResponseBody);
    }
    let chunks = resource
        .length
        .div_ceil(crate::protocol::MAX_RESOURCE_CHUNK_BYTES as u64);
    if progress.confirmed_chunks.len() as u64 > chunks
        || progress
            .confirmed_chunks
            .iter()
            .any(|index| u64::from(*index) >= chunks)
        || progress
            .confirmed_chunks
            .windows(2)
            .any(|pair| pair[0] >= pair[1])
    {
        return Err(ClientError::InvalidResponseBody);
    }
    Ok(())
}

pub(super) fn validate_mutation_ack(ack: &MutationAck, mutation: &MutationDto) -> Result<(), ClientError> {
    #[derive(serde::Deserialize)]
    struct Outcome {
        accepted: bool,
        reason: Option<String>,
    }
    let confirmation = &ack.confirmation;
    let outcome: Outcome = serde_json::from_str(&confirmation.outcome_json)
        .map_err(|_| ClientError::InvalidResponseBody)?;
    if confirmation.mutation_id != mutation.mutation_id
        || outcome.accepted != confirmation.checkpoint.is_some()
        || (outcome.accepted && outcome.reason.is_some())
        || (!outcome.accepted && outcome.reason.as_ref().is_none_or(String::is_empty))
        || confirmation.checkpoint.is_some_and(|checkpoint| {
            checkpoint.binding != mutation.binding || checkpoint.sequence.as_u64() == 0
        })
    {
        return Err(ClientError::InvalidResponseBody);
    }
    Ok(())
}

fn valid_endpoint(endpoint: &Url) -> bool {
    endpoint.scheme() == "https" && endpoint.username().is_empty() && endpoint.password().is_none()
}

async fn read_bounded_json<T: DeserializeOwned>(
    mut response: reqwest::Response,
) -> Result<T, ClientError> {
    if response
        .content_length()
        .is_some_and(|length| length > MAX_SYNC_PAGE_BYTES as u64)
    {
        return Err(ClientError::InvalidResponseBody);
    }
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(ClientError::Transport)? {
        if body.len().saturating_add(chunk.len()) > MAX_SYNC_PAGE_BYTES {
            return Err(ClientError::InvalidResponseBody);
        }
        body.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&body).map_err(|_| ClientError::InvalidResponseBody)
}

/// Apply a verified host page through Core's no-outbox replica boundary.
///
/// The supplied peer identity must be the already authenticated host for this
/// binding; Core atomically persists the page and its cursor.
///
/// # Errors
/// Rejects invalid commit pages or propagates failures from local apply.
pub fn apply_commit_page(
    page: &CommitPageDto,
    peer_device_id: DeviceId,
    local_apply: &dyn RemoteApplyPort,
) -> Result<AppliedRemoteCursor, ClientError> {
    validate_commit_page(page, page.anchor_sequence.as_u64(), page.anchor_digest)?;
    let commits = page
        .entries
        .iter()
        .map(|entry| {
            let mutation = &entry.commit.mutation;
            let payload_json = serde_json::to_string(&mutation.payload)
                .map_err(|_| ClientError::InvalidCommitChain)?;
            let canonical_json = serde_json::to_string(&entry.commit)
                .map_err(|_| ClientError::InvalidCommitChain)?;
            Ok(RemoteCommit {
                sequence: entry.commit.commit_seq,
                previous_digest: entry.commit.previous_digest,
                digest: entry.digest,
                origin_device_id: mutation.origin_device_id,
                mutation_id: mutation.mutation_id,
                object: mutation.object,
                is_tombstone: mutation.payload == serde_json::json!({"tombstone": true}),
                payload_json,
                canonical_json,
            })
        })
        .collect::<Result<Vec<_>, ClientError>>()?;
    local_apply
        .apply_remote_batch(
            page.binding,
            peer_device_id,
            echo_core::domain::sync::CommitCheckpoint {
                binding: page.binding,
                sequence: page.anchor_sequence,
                digest: page.anchor_digest,
            },
            &commits,
        )
        .map_err(ClientError::RemoteApply)
}

/// Convert a validated wire page into Core's durable snapshot enrollment API.
///
/// # Errors
/// Rejects malformed resource metadata or Core enrollment failures.
pub fn apply_snapshot_page(
    page: &SnapshotPageDto,
    peer_device_id: DeviceId,
    local_apply: &dyn ReplicaSnapshotApplyPort,
) -> Result<AppliedSnapshotPage, ClientError> {
    validate_snapshot_page(page).map_err(ClientError::InvalidSnapshot)?;
    let records = page
        .objects
        .iter()
        .map(|object| {
            let mutation = &object.mutation;
            let resources = object
                .resources
                .iter()
                .map(|resource| {
                    let digest = parse_digest(&resource.content_hash)
                        .ok_or(ClientError::InvalidSnapshot(ProtocolError::InvalidResource))?;
                    SharedResourceManifest::new(
                        resource.kind,
                        ResourceDigest::from_bytes(digest),
                        resource.length,
                        resource.media_type.clone(),
                        &resource.relative_path,
                    )
                    .map_err(|_| ClientError::InvalidSnapshot(ProtocolError::InvalidResource))
                })
                .collect::<Result<Vec<_>, _>>()?;
            Ok(HostSnapshotRecord::Object {
                candidate: MutationCandidate {
                    resource_refs: resources.clone(),
                    mutation_id: mutation.mutation_id,
                    origin_device_id: mutation.origin_device_id,
                    origin_seq: mutation.origin_seq,
                    previous_mutation_id: mutation.previous_mutation_id,
                    object: mutation.object,
                    base_object_version: mutation.base_object_version,
                    payload: mutation.payload.clone(),
                },
                version: object.version,
                resources,
            })
        })
        .collect::<Result<Vec<_>, ClientError>>()?;
    let mut records = records;
    records.extend(page.aliases.iter().map(|alias| match alias {
        crate::protocol::TypedAliasDto::Song(alias) => HostSnapshotRecord::SongAlias {
            alias: alias.alias,
            canonical: alias.canonical,
        },
        crate::protocol::TypedAliasDto::PlaylistMember(alias) => {
            HostSnapshotRecord::PlaylistMemberAlias(PlaylistMemberAlias {
                playlist_id: alias.playlist_id,
                alias_member_id: alias.alias,
                canonical_member_id: alias.canonical,
            })
        }
    }));
    let replica_page = ReplicaSnapshotPage {
        snapshot_token: page.snapshot_token.clone(),
        checkpoint: CommitCheckpoint {
            binding: page.checkpoint.binding,
            sequence: page.checkpoint.commit_seq,
            digest: page.checkpoint.digest,
        },
        page_index: page.page_index,
        next_page_index: page.next_page_index,
        records,
    };
    local_apply
        .apply_snapshot_page(peer_device_id, &replica_page)
        .map_err(ClientError::RemoteApply)
}

fn parse_digest(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return None;
    }
    let mut digest = [0; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).ok()?;
    }
    Some(digest)
}

fn validate_commit_page(
    page: &CommitPageDto,
    after: u64,
    previous_digest: [u8; 32],
) -> Result<(), ClientError> {
    if page.anchor_sequence.as_u64() != after
        || page.anchor_digest != previous_digest
        || page.entries.len() > MAX_PAGE_SIZE
    {
        return Err(ClientError::InvalidCommitChain);
    }
    let mut sequence = after;
    let mut digest = previous_digest;
    for entry in &page.entries {
        validate_mutation(&entry.commit.mutation).map_err(ClientError::InvalidMutation)?;
        if entry.commit.mutation.binding != page.binding
            || serde_json::to_vec(&entry.commit)
                .map_err(|_| ClientError::InvalidCommitChain)?
                .len()
                > MAX_COMMIT_BYTES
        {
            return Err(ClientError::InvalidCommitChain);
        }
        if !entry.commit.mutation.resource_refs.is_empty()
            && entry.commit.resource_refs != entry.commit.mutation.resource_refs
        {
            return Err(ClientError::InvalidCommitChain);
        }
        for resource in &entry.commit.resource_refs {
            validate_resource(resource).map_err(ClientError::InvalidMutation)?;
        }
        sequence = sequence
            .checked_add(1)
            .ok_or(ClientError::InvalidCommitChain)?;
        if entry.commit.binding != page.binding
            || entry.commit.commit_seq.as_u64() != sequence
            || entry.commit.previous_digest != digest
            || commit_digest(&entry.commit).map_err(ClientError::InvalidMutation)? != entry.digest
        {
            return Err(ClientError::InvalidCommitChain);
        }
        digest = entry.digest;
    }
    Ok(())
}

fn hex_digest(digest: &[u8; 32]) -> String {
    let mut encoded = String::with_capacity(64);
    for byte in digest {
        use std::fmt::Write as _;
        let _ = write!(encoded, "{byte:02x}");
    }
    encoded
}

fn pairing_url(endpoint: &Url, suffix: &str) -> Result<Url, ClientError> {
    if endpoint.scheme() != "https" || endpoint.username() != "" || endpoint.password().is_some() {
        return Err(ClientError::InvalidEndpoint);
    }
    let mut url = endpoint.clone();
    url.set_path(&format!(
        "{}{suffix}",
        endpoint.path().trim_end_matches('/')
    ));
    url.set_query(None);
    url.set_fragment(None);
