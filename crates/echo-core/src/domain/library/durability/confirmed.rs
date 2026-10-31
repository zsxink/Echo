//! Complete confirmation evidence; validating it does not authorize a session.
use super::{state::encode, valid_digest, StateKey, StateNamespace, StateTarget};
use crate::domain::sync::ObjectKey;
use crate::domain::sync::{DeviceId, MutationCandidate, MutationId, SyncBinding};
use crate::error::Error;
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeSet;
use uuid::Uuid;

#[derive(Deserialize)]
struct Confirmation {
    author: DeviceId,
    mutation_id: MutationId,
    version: u64,
    sequence: u64,
    payload: Value,
    tombstone: bool,
    changed_at_ms: u64,
    original_body: String,
    previous_digest: String,
    digest: String,
}

#[derive(Deserialize)]
struct CommitBody {
    protocol_version: u16,
    binding: SyncBinding,
    commit_seq: u64,
    previous_digest: [u8; 32],
}

// LAN resources use a hexadecimal content_hash; persisted domain manifests use
// a digest byte array. Decode the wire shape explicitly without rewriting the
// exact original body that the commit digest authenticates.
#[derive(Deserialize)]
struct WireResource {
    kind: crate::domain::sync::SharedResourceKind,
    content_hash: String,
    length: u64,
    media_type: String,
    relative_path: String,
}

#[derive(Deserialize)]
#[serde(tag = "kind", content = "alias", rename_all = "snake_case")]
enum WireAlias {
    Song {
        alias: Uuid,
        canonical: Uuid,
    },
    PlaylistMember {
        playlist_id: Uuid,
        alias: Uuid,
        canonical: Uuid,
    },
}
fn aliases(raw: &Value, object: ObjectKey) -> Result<(), Error> {
    let aliases: Vec<WireAlias> = serde_json::from_value(raw["aliases"].clone()).map_err(encode)?;
    let mut seen = BTreeSet::new();
    for alias in aliases {
        let (kind, alias, canonical, matches) = match alias {
            WireAlias::Song { alias, canonical } => (
                "song",
                alias,
                canonical,
                matches!(object,ObjectKey::Song(id) if id.as_uuid() == canonical),
            ),
            WireAlias::PlaylistMember {
                playlist_id,
                alias,
                canonical,
            } => (
                "playlist-item",
                alias,
                canonical,
                matches!(object,ObjectKey::Playlist(id) if id.as_uuid() == playlist_id),
            ),
        };
        if !matches
            || alias.is_nil()
            || canonical.is_nil()
            || alias == canonical
            || !seen.insert((kind, alias))
        {
            return Err(Error::conflict("invalid confirmation typed alias"));
        }
    }
    Ok(())
}

fn mutation(raw: &Value) -> Result<MutationCandidate, Error> {
    let mut domain = raw["mutation"].clone();
    let fields = domain
        .as_object_mut()
        .ok_or_else(|| Error::conflict("confirmation mutation is not an object"))?;
    if !["previous_mutation_id", "base_object_version"]
        .iter()
        .all(|field| fields.contains_key(*field))
        || !raw.get("aliases").is_some_and(Value::is_array)
    {
        return Err(Error::conflict("confirmation original body is incomplete"));
    }
    let resources: Vec<WireResource> =
        serde_json::from_value(raw["resource_refs"].clone()).map_err(encode)?;
    let nested = fields
        .get("resource_refs")
        .cloned()
        .unwrap_or_else(|| serde_json::json!([]));
    if nested != raw["resource_refs"] {
        return Err(Error::conflict("confirmation resource declarations differ"));
    }
    let mut manifests = Vec::with_capacity(resources.len());
    for resource in resources {
        if !valid_digest(&resource.content_hash) {
            return Err(Error::conflict("invalid confirmation resource digest"));
        }
        let hash = blake3::Hash::from_hex(&resource.content_hash)
            .map_err(|_| Error::conflict("invalid confirmation resource digest"))?;
        manifests.push(crate::domain::sync::SharedResourceManifest::new(
            resource.kind,
            crate::domain::sync::ResourceDigest::from_bytes(*hash.as_bytes()),
            resource.length,
            resource.media_type,
            &resource.relative_path,
        )?);
    }
    fields.insert(
        "resource_refs".to_owned(),
        serde_json::to_value(manifests).map_err(encode)?,
    );
    serde_json::from_value(domain).map_err(encode)
}

pub(super) fn validate(key: &StateKey, payload: &Value) -> Result<(), Error> {
    let fact: Confirmation = serde_json::from_value(payload.clone()).map_err(encode)?;
    let body: CommitBody = serde_json::from_str(&fact.original_body).map_err(encode)?;
    let raw: Value = serde_json::from_str(&fact.original_body).map_err(encode)?;
    let mutation = mutation(&raw)?;
    aliases(&raw, mutation.object)?;
    let StateNamespace::Bound { binding } = key.namespace else {
        return Err(Error::conflict(
            "complete confirmation requires its original binding",
        ));
    };
    let tombstone = mutation.payload == serde_json::json!({"tombstone":true});
    let previous = blake3::Hash::from_bytes(body.previous_digest)
        .to_hex()
        .to_string();
    let length = u32::try_from(fact.original_body.len())
        .map_err(|_| Error::conflict("confirmation body is too large"))?;
    let mut hash = blake3::Hasher::new();
    hash.update(b"echo.lan.commit.v1\0");
    hash.update(&1u16.to_be_bytes());
    hash.update(&fact.sequence.to_be_bytes());
    hash.update(&body.previous_digest);
    hash.update(&length.to_be_bytes());
    hash.update(fact.original_body.as_bytes());
    if key.target
        != (StateTarget::Confirmed {
            object: mutation.object,
        })
        || body.protocol_version != 1
        || body.binding != binding
        || raw
            .pointer("/mutation/protocol_version")
            .and_then(Value::as_u64)
            != Some(1)
        || raw.pointer("/mutation/binding") != Some(&serde_json::to_value(binding).map_err(encode)?)
        || fact.sequence == 0
        || body.commit_seq != fact.sequence
        || fact.version != fact.sequence
        || fact.author != mutation.origin_device_id
        || fact.mutation_id != mutation.mutation_id
        || fact.payload != mutation.payload
        || fact.tombstone != tombstone
        || !valid_digest(&fact.previous_digest)
        || fact.previous_digest != previous
        || !valid_digest(&fact.digest)
        || hash.finalize().to_hex().as_str() != fact.digest
    {
        return Err(Error::conflict("invalid complete confirmation evidence"));
    }
    // This is the recorded SQL baseline time, not a reconstructed ledger clock.
    let _ = fact.changed_at_ms;
    Ok(())
}
