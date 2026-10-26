//! Complete local publication evidence; it never authorizes file availability.
use super::{
    state::{encode, object_identity},
    valid_digest, StateKey, StateNamespace, StateTarget,
};
use crate::domain::{
    ids::{LibraryRootId, RelativeMediaPath},
    sync::{ObjectKey, ResourceDigest, SharedResourceKind, SharedResourceManifest},
};
use crate::error::Error;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;
use uuid::Uuid;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Fact {
    schema: String,
    operation: Uuid,
    publication: Publication,
    original_body: String,
    completed: bool,
    state_key: StateKey,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Publication {
    root: LibraryRootId,
    operation: Uuid,
    object: ObjectKey,
    items: Vec<Item>,
    completed: bool,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Item {
    target: Target,
    stage: Stage,
    staging_path: String,
    local_path: String,
    #[serde(deserialize_with = "required_option")]
    old_path: Option<String>,
}
#[derive(Deserialize, PartialEq)]
enum Stage {
    Planned,
    Validated,
    OldIsolated,
    Published,
    Completed,
}
// Field order matches the existing journal declaration serializer. Its hash is
// the managed resource-set version, not a host object_version or authorization.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Target {
    manifest: Manifest,
    #[serde(deserialize_with = "required_option")]
    expected_old_digest: Option<ResourceDigest>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    kind: SharedResourceKind,
    digest: ResourceDigest,
    length: u64,
    media_type: String,
    relative_path: Path,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Path {
    normalized: String,
}

fn required_option<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(deserializer)
}
fn invalid() -> Error {
    Error::conflict("invalid complete resource publication evidence")
}

pub(super) fn validate(key: &StateKey, payload: &Value) -> Result<(), Error> {
    let fact: Fact = serde_json::from_value(payload.clone()).map_err(encode)?;
    let publication = &fact.publication;
    let original: Value = serde_json::from_str(&fact.original_body).map_err(encode)?;
    let (kind, owner) = object_identity(publication.object);
    let current_location = fact.schema == "echo.library.resource-location.v1";
    let valid_scope = if current_location {
        matches!(key.namespace, StateNamespace::Library { .. })
            && key.target
                == (StateTarget::ResourceLocation {
                    object: publication.object,
                })
            && publication.completed
    } else {
        fact.schema == "echo.sync.resource-publication.v2"
            && key.target
                == (StateTarget::Publication {
                    operation_id: fact.operation,
                })
            && key.namespace
                == (StateNamespace::LegacyEvidence {
                    source_root: publication.root,
                })
    };
    if !valid_scope
        || fact.state_key != *key
        || publication.root.as_uuid().is_nil()
        || owner.is_nil()
        || publication.operation != fact.operation
        || fact.completed != publication.completed
        || original != payload["publication"]
        || publication.items.is_empty()
        || publication.items.len() > 32
    {
        return Err(invalid());
    }
    let targets: Vec<_> = publication.items.iter().map(|item| &item.target).collect();
    let version = blake3::hash(&serde_json::to_vec(&targets).map_err(encode)?)
        .to_hex()
        .to_string();
    let mut logical_paths = BTreeSet::new();
    let mut local_paths = BTreeSet::new();
    let mut staging_paths = BTreeSet::new();
    for item in &publication.items {
        let manifest = &item.target.manifest;
        let logical = &manifest.relative_path.normalized;
        SharedResourceManifest::new(
            manifest.kind,
            manifest.digest,
            manifest.length,
            manifest.media_type.clone(),
            logical,
        )?;
        let logical_path = canonical_path(logical)?;
        let local_path = canonical_path(&item.local_path)?;
        let staging = canonical_path(&item.staging_path)?;
        if !logical_paths.insert(logical_path.identity_key().to_owned())
            || !local_paths.insert(local_path.identity_key().to_owned())
            || !staging_paths.insert(staging.identity_key().to_owned())
            || !valid_location(&item.local_path, logical, kind, owner, Some(&version))
            || !owned_staging(&item.staging_path, fact.operation)
            || publication.completed != (item.stage == Stage::Completed)
            || item.target.expected_old_digest.is_some() != item.old_path.is_some()
        {
            return Err(invalid());
        }
        if let Some(old) = &item.old_path {
            canonical_path(old)?;
            if !valid_location(old, logical, kind, owner, None) {
                return Err(invalid());
            }
        }
    }
    Ok(())
}
pub(super) fn validate_resources(
    payload: &Value,
    resources: &[super::DurableResource],
) -> Result<(), Error> {
    let fact: Fact = serde_json::from_value(payload.clone()).map_err(encode)?;
    if fact.publication.items.iter().any(|item| {
        !resources.iter().any(|resource| {
            resource.relative_path == item.local_path
                && resource.content_hash == item.target.manifest.digest.to_hex()
                && resource.byte_length == item.target.manifest.length
        })
    }) {
        return Err(Error::conflict(
            "resource location has no complete verified resource set",
        ));
    }
    Ok(())
}
fn canonical_path(path: &str) -> Result<RelativeMediaPath, Error> {
    let parsed = RelativeMediaPath::new(path)?;
    if parsed.normalized() != path || path.chars().any(char::is_control) {
        return Err(invalid());
    }
    Ok(parsed)
}
fn owned_staging(path: &str, operation: Uuid) -> bool {
    [operation.to_string(), operation.simple().to_string()]
        .iter()
        .any(|id| {
            path.strip_prefix(&format!("echo/tmp/{id}/"))
                .is_some_and(|suffix| !suffix.is_empty())
        })
}
fn valid_location(
    path: &str,
    logical: &str,
    kind: &str,
    owner: Uuid,
    version: Option<&str>,
) -> bool {
    if path == logical {
        return true;
    }
    let Some(suffix) = path.strip_prefix(&format!("echo/media/{kind}/{owner}/")) else {
        return false;
    };
    let Some((digest, tail)) = suffix.split_once('/') else {
        return false;
    };
    valid_digest(digest) && version.is_none_or(|expected| digest == expected) && tail == logical
}
