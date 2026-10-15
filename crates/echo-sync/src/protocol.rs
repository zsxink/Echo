//! Versioned, bounded wire data for LAN library synchronization.
//!
//! The wire DTOs deliberately live outside Core and do not expose SQL or HTTP
//! types. JSON is used only as the canonical body encoding; the commit digest
//! wraps it in a versioned, length-delimited BLAKE3 envelope.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use echo_core::domain::sync::{
    CommitSeq, DeviceId, Epoch, LibraryId, MutationId, ObjectKey, ObjectVersion, ResourceDigest,
    SharedResourceKind, SharedResourceManifest, SyncBinding,
};

pub const PROTOCOL_VERSION: u16 = 1;
pub const MAX_MUTATION_BYTES: usize = 1_048_576;
/// Reserve space for the confirmation envelope around a maximum-size mutation.
pub const MAX_COMMIT_BYTES: usize = MAX_MUTATION_BYTES + 64 * 1024;
/// Maximum decoded JSON body for one host commit page.
pub const MAX_SYNC_PAGE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_RESOURCE_CHUNK_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_PAGE_SIZE: usize = 512;
pub const MAX_JSON_DEPTH: usize = 64;
const DIGEST_DOMAIN: &[u8] = b"echo.lan.commit.v1\0";

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ObjectKind {
    Song,
    Favorite,
    Playlist,
    PlaylistItem,
    Override,
    PlayStats,
    LibraryRoot,
    Tombstone,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MutationDto {
    pub protocol_version: u16,
    pub binding: SyncBinding,
    pub origin_device_id: DeviceId,
    pub mutation_id: MutationId,
    pub origin_seq: u64,
    #[serde(default)]
    pub previous_mutation_id: Option<MutationId>,
    pub object: ObjectKey,
    #[serde(default)]
    pub base_object_version: Option<ObjectVersion>,
    pub payload: Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub resource_refs: Vec<ResourceRefDto>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SongAliasDto {
    pub alias: echo_core::domain::ids::SongId,
    pub canonical: echo_core::domain::ids::SongId,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MemberAliasDto {
    pub playlist_id: echo_core::domain::ids::PlaylistId,
    pub alias: echo_core::domain::ids::PlaylistItemId,
    pub canonical: echo_core::domain::ids::PlaylistItemId,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "alias", rename_all = "snake_case")]
pub enum TypedAliasDto {
    Song(SongAliasDto),
    PlaylistMember(MemberAliasDto),
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CommitDto {
    pub protocol_version: u16,
    pub binding: SyncBinding,
    pub commit_seq: CommitSeq,
    pub previous_digest: [u8; 32],
    pub mutation: MutationDto,
    #[serde(default)]
    pub aliases: Vec<TypedAliasDto>,
    #[serde(default)]
    pub resource_refs: Vec<ResourceRefDto>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CommittedEntryDto {
    pub commit: CommitDto,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CommitPageDto {
    pub binding: SyncBinding,
    pub anchor_sequence: CommitSeq,
    pub anchor_digest: [u8; 32],
    pub entries: Vec<CommittedEntryDto>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResourceRefDto {
    pub kind: SharedResourceKind,
    /// Lowercase hexadecimal BLAKE3 digest of the complete immutable resource.
    pub content_hash: String,
    pub length: u64,
    pub media_type: String,
    /// Validated library-relative path; absolute paths are forbidden.
    pub relative_path: String,
}

impl From<&SharedResourceManifest> for ResourceRefDto {
    fn from(value: &SharedResourceManifest) -> Self {
        Self {
            kind: value.kind,
            content_hash: value.digest.to_hex(),
            length: value.length,
            media_type: value.media_type.clone(),
            relative_path: value.relative_path.as_str().to_owned(),
        }
    }
}

impl ResourceRefDto {
    /// Convert a validated wire declaration into Core's path-safe manifest.
    ///
    /// # Errors
    /// Returns `InvalidResource` for invalid metadata or digest encoding.
    pub fn to_manifest(&self) -> Result<SharedResourceManifest, ProtocolError> {
        validate_resource(self)?;
        let mut digest = [0; 32];
        for (index, byte) in digest.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&self.content_hash[index * 2..index * 2 + 2], 16)
                .map_err(|_| ProtocolError::InvalidResource)?;
        }
        SharedResourceManifest::new(
            self.kind,
            ResourceDigest::from_bytes(digest),
            self.length,
            self.media_type.clone(),
            &self.relative_path,
        )
        .map_err(|_| ProtocolError::InvalidResource)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ResourceChunkDto {
    pub content_hash: String,
    pub offset: u64,
    pub total_length: u64,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CheckpointDto {
    pub binding: SyncBinding,
    pub commit_seq: CommitSeq,
    pub digest: [u8; 32],
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MutationAck {
    pub confirmation: echo_core::application::ports::StagedConfirmation,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SnapshotPageDto {
    /// Opaque random handle for this frozen snapshot session.
    pub snapshot_token: String,
    pub checkpoint: CheckpointDto,
    pub page_index: u32,
    pub next_page_index: Option<u32>,
    pub objects: Vec<SnapshotObjectDto>,
    #[serde(default)]
    pub aliases: Vec<TypedAliasDto>,
}

/// A frozen object carries the host-confirmed version independently from the
/// originating mutation's optional conflict base.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SnapshotObjectDto {
    pub mutation: MutationDto,
    pub version: ObjectVersion,
    #[serde(default)]
    pub resources: Vec<ResourceRefDto>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProtocolError {
    UnsupportedVersion(u16),
    PayloadTooLarge(usize),
    JsonTooDeep,
    InvalidResource,
    InvalidPageSize(usize),
    InvalidPageToken,
    InvalidResourceChunk,
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for ProtocolError {}

/// Validate a client mutation before accepting a write.
///
/// Unknown optional JSON fields are naturally ignored by serde DTOs; unknown
/// required enum kinds fail deserialization and must not be silently downgraded.
///
/// # Errors
/// Rejects unsupported protocol versions, oversized payloads, or excessive JSON depth.
pub fn validate_mutation(dto: &MutationDto) -> Result<(), ProtocolError> {
    if dto.protocol_version != PROTOCOL_VERSION {
        return Err(ProtocolError::UnsupportedVersion(dto.protocol_version));
    }
    let encoded =
        serde_json::to_vec(dto).map_err(|_| ProtocolError::PayloadTooLarge(usize::MAX))?;
    if encoded.len() > MAX_MUTATION_BYTES {
        return Err(ProtocolError::PayloadTooLarge(encoded.len()));
    }
    if json_depth(&dto.payload) > MAX_JSON_DEPTH {
        return Err(ProtocolError::JsonTooDeep);
    }
    if serde_json::to_vec(&dto.resource_refs)
        .map_err(|_| ProtocolError::InvalidResource)?
        .len()
        > 32 * 1024
        || dto.resource_refs.len() > 32
    {
        return Err(ProtocolError::InvalidResource);
    }
    let mut destinations = std::collections::HashSet::new();
    for resource in &dto.resource_refs {
        if !destinations.insert(&resource.relative_path) {
            return Err(ProtocolError::InvalidResource);
        }
        validate_resource(resource)?;
    }
    Ok(())
}

/// Validate immutable resource metadata and its library relative path.
///
/// # Errors
/// Returns `InvalidResource` for an invalid digest, media type, or path.
pub fn validate_resource(resource: &ResourceRefDto) -> Result<(), ProtocolError> {
    if resource.content_hash.len() != 64
        || !resource
            .content_hash
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        || !valid_media_type(&resource.media_type)
        || resource.media_type.len() > 127
        || resource.relative_path.len() > 4096
        || echo_core::domain::library::LibraryRelativePath::validate(&resource.relative_path)
            .is_err()
    {
        return Err(ProtocolError::InvalidResource);
    }
    let mut digest = [0u8; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        let start = index * 2;
        *byte = u8::from_str_radix(&resource.content_hash[start..start + 2], 16)
            .map_err(|_| ProtocolError::InvalidResource)?;
    }
    SharedResourceManifest::new(
        resource.kind,
        ResourceDigest::from_bytes(digest),
        resource.length,
        resource.media_type.clone(),
        &resource.relative_path,
    )
    .map_err(|_| ProtocolError::InvalidResource)?;
    Ok(())
}

/// Validate the requested page size against the protocol limit.
///
/// # Errors
/// Returns `InvalidPageSize` for zero or a size above `MAX_PAGE_SIZE`.
pub const fn validate_page_size(size: usize) -> Result<(), ProtocolError> {
    if size == 0 || size > MAX_PAGE_SIZE {
        Err(ProtocolError::InvalidPageSize(size))
    } else {
        Ok(())
    }
}

/// Validate snapshot page bounds and each contained mutation.
///
/// # Errors
/// Rejects oversized pages or tokens and propagates mutation validation errors.
pub fn validate_snapshot_page(page: &SnapshotPageDto) -> Result<(), ProtocolError> {
    let record_count = page.objects.len() + page.aliases.len();
    let owned_resource_count: usize = page
        .objects
        .iter()
        .map(|object| object.resources.len())
        .sum();
    if record_count + owned_resource_count > MAX_PAGE_SIZE {
        return Err(ProtocolError::InvalidPageSize(
            record_count + owned_resource_count,
        ));
    }
    if page.snapshot_token.len() != 32
        || !page
            .snapshot_token
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        || page
            .next_page_index
            .is_some_and(|next| next != page.page_index.saturating_add(1))
    {
        return Err(ProtocolError::InvalidPageToken);
    }
    for object in &page.objects {
        validate_mutation(&object.mutation)?;
        if object.mutation.binding != page.checkpoint.binding {
            return Err(ProtocolError::InvalidPageToken);
        }
        if !object.mutation.resource_refs.is_empty()
            && object.mutation.resource_refs != object.resources
        {
            return Err(ProtocolError::InvalidResource);
        }
        for resource in &object.resources {
            validate_resource(resource)?;
        }
    }
    if page.aliases.iter().any(|alias| match alias {
        TypedAliasDto::Song(alias) => alias.alias == alias.canonical,
        TypedAliasDto::PlaylistMember(alias) => alias.alias == alias.canonical,
    }) {
        return Err(ProtocolError::InvalidPageToken);
    }
    let encoded =
        serde_json::to_vec(page).map_err(|_| ProtocolError::PayloadTooLarge(usize::MAX))?;
    if encoded.len() > MAX_SYNC_PAGE_BYTES {
        return Err(ProtocolError::PayloadTooLarge(encoded.len()));
    }
    Ok(())
}

/// Validate a resource chunk digest, size, and range.
///
/// # Errors
/// Returns `InvalidResourceChunk` for invalid metadata or a range outside the resource.
pub fn validate_resource_chunk(chunk: &ResourceChunkDto) -> Result<(), ProtocolError> {
    let end = chunk.offset.checked_add(chunk.bytes.len() as u64);
    if validate_hash(&chunk.content_hash).is_err()
        || chunk.bytes.is_empty()
        || chunk.bytes.len() > MAX_RESOURCE_CHUNK_BYTES
        || end.map_or(true, |end| end > chunk.total_length)
    {
        return Err(ProtocolError::InvalidResourceChunk);
    }
    Ok(())
}

fn validate_hash(hash: &str) -> Result<(), ProtocolError> {
    if hash.len() == 64
        && hash
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        Ok(())
    } else {
        Err(ProtocolError::InvalidResource)
    }
}

fn valid_media_type(value: &str) -> bool {
    if value.is_empty() || value.len() > 127 {
        return false;
    }
    let Some((major, minor)) = value.split_once('/') else {
        return false;
    };
    !major.is_empty()
        && !minor.is_empty()
        && major.bytes().chain(minor.bytes()).all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#' | b'$' | b'&' | b'^' | b'_' | b'.' | b'+' | b'-'
                )
        })
}

/// Calculate the protocol-defined digest for one commit.
///
/// Field encoding uses fixed-width big endian lengths and a domain tag to
/// prevent cross-protocol collisions. The serialized body has deterministic
/// struct field order.
///
/// # Errors
/// Rejects an unsupported protocol version or a body that cannot be encoded
/// within the digest envelope's length limit.
pub fn commit_digest(commit: &CommitDto) -> Result<[u8; 32], ProtocolError> {
    if commit.protocol_version != PROTOCOL_VERSION {
        return Err(ProtocolError::UnsupportedVersion(commit.protocol_version));
    }
    let body =
        serde_json::to_vec(commit).map_err(|_| ProtocolError::PayloadTooLarge(usize::MAX))?;
    let len = u32::try_from(body.len()).map_err(|_| ProtocolError::PayloadTooLarge(body.len()))?;
    let mut hasher = blake3::Hasher::new();
    hasher.update(DIGEST_DOMAIN);
    hasher.update(&commit.protocol_version.to_be_bytes());
    hasher.update(&commit.commit_seq.as_u64().to_be_bytes());
    hasher.update(&commit.previous_digest);
    hasher.update(&len.to_be_bytes());
    hasher.update(&body);
    Ok(*hasher.finalize().as_bytes())
}

fn json_depth(value: &Value) -> usize {
    match value {
        Value::Array(values) => 1 + values.iter().map(json_depth).max().unwrap_or(0),
        Value::Object(values) => 1 + values.values().map(json_depth).max().unwrap_or(0),
        _ => 0,
    }
}

// Keep imports in the public protocol surface explicit and used: binding
// material is intentionally part of every checkpoint and mutation.
const _: Option<(LibraryId, Epoch)> = None;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fmt::Write as _;

    fn mutation() -> MutationDto {
        MutationDto {
            resource_refs: Vec::new(),
            protocol_version: PROTOCOL_VERSION,
            binding: SyncBinding {
                library_id: LibraryId::new(),
                authority_id: echo_core::domain::sync::AuthorityId::new(),
                epoch: Epoch::INITIAL,
            },
            origin_device_id: DeviceId::new(),
            mutation_id: MutationId::new(),
            origin_seq: 1,
            previous_mutation_id: None,
            object: ObjectKey::Song(echo_core::domain::ids::SongId::new()),
            base_object_version: None,
            payload: json!({"value": 1}),
        }
    }

    #[test]
    fn digest_is_stable_and_binds_sequence_and_previous_digest() {
        let mut commit = CommitDto {
            protocol_version: PROTOCOL_VERSION,
            binding: mutation().binding,
            commit_seq: CommitSeq::from_u64(4),
            previous_digest: [7; 32],
            mutation: mutation(),
            aliases: vec![],
            resource_refs: vec![],
        };
        let first = commit_digest(&commit).unwrap();
        assert_eq!(first, commit_digest(&commit).unwrap());
        commit.commit_seq = CommitSeq::from_u64(5);
        assert_ne!(first, commit_digest(&commit).unwrap());
    }

    #[test]
    fn optional_unknown_json_fields_are_ignored_but_unknown_kind_is_rejected() {
        let mut value = serde_json::to_value(mutation()).unwrap();
        value["future_optional"] = json!(true);
        assert!(serde_json::from_value::<MutationDto>(value).is_ok());
        assert!(serde_json::from_str::<ObjectKind>("\"future_required_kind\"").is_err());
    }

    #[test]
    fn rejects_oversized_and_incompatible_write_payloads() {
        let mut dto = mutation();
        dto.payload = Value::String("x".repeat(MAX_MUTATION_BYTES));
        assert!(matches!(
            validate_mutation(&dto),
            Err(ProtocolError::PayloadTooLarge(_))
        ));
        dto = mutation();
        dto.protocol_version += 1;
        assert_eq!(
            validate_mutation(&dto),
            Err(ProtocolError::UnsupportedVersion(2))
        );
    }

    #[test]
    fn rejects_invalid_resource_paths_and_page_limits() {
        let resource = ResourceRefDto {
            kind: SharedResourceKind::Audio,
            content_hash: "0".repeat(64),
            length: 1,
            media_type: "audio/flac".into(),
            relative_path: "/etc/passwd".into(),
        };
        assert_eq!(
            validate_resource(&resource),
            Err(ProtocolError::InvalidResource)
        );
        assert!(validate_page_size(0).is_err());
        assert!(validate_page_size(MAX_PAGE_SIZE + 1).is_err());
    }

    #[test]
    fn permits_an_empty_snapshot_and_bounds_resource_chunks() {
        let checkpoint = CheckpointDto {
            binding: mutation().binding,
            commit_seq: CommitSeq::GENESIS,
            digest: [0; 32],
        };
        let page = SnapshotPageDto {
            snapshot_token: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            checkpoint,
            page_index: 0,
            next_page_index: None,
            objects: vec![],
            aliases: vec![],
        };
        assert!(validate_snapshot_page(&page).is_ok());
        let object_mutation = mutation();
        let object_page = SnapshotPageDto {
            snapshot_token: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
            checkpoint: CheckpointDto {
                binding: object_mutation.binding,
                commit_seq: CommitSeq::from_u64(1),
                digest: [1; 32],
            },
            page_index: 0,
            next_page_index: None,
            objects: vec![SnapshotObjectDto {
                mutation: object_mutation,
                version: ObjectVersion::from_commit_seq(CommitSeq::from_u64(1)),
                resources: vec![],
            }],
            aliases: vec![],
        };
        assert!(validate_snapshot_page(&object_page).is_ok());
        let mut wrong_binding = object_page;
        wrong_binding.objects[0].mutation.binding.epoch =
            echo_core::domain::sync::Epoch::from_u64(999);
        assert_eq!(
            validate_snapshot_page(&wrong_binding),
            Err(ProtocolError::InvalidPageToken)
        );
        let invalid = ResourceChunkDto {
            content_hash: "a".repeat(64),
            offset: 0,
            total_length: (MAX_RESOURCE_CHUNK_BYTES + 1) as u64,
            bytes: vec![0; MAX_RESOURCE_CHUNK_BYTES + 1],
        };
        assert_eq!(
            validate_resource_chunk(&invalid),
            Err(ProtocolError::InvalidResourceChunk)
        );
    }

    #[test]
    fn checked_in_commit_fixture_remains_readable_and_hashable() {
        let fixture: CommitDto =
            serde_json::from_str(include_str!("../tests/fixtures/commit-v1.json")).unwrap();
        assert_eq!(fixture.protocol_version, PROTOCOL_VERSION);
        let digest = commit_digest(&fixture).unwrap();
        let digest_hex = digest
            .iter()
            .fold(String::with_capacity(64), |mut hex, byte| {
                let _ = write!(hex, "{byte:02x}");
                hex
            });
        assert_eq!(
            digest_hex,
            "dc2bbbabce5cd3132ff411710a7350df06a736d1ca48cc1e1abe572f8977424b"
        );
    }
}
