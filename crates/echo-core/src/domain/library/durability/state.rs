//! Stable private-fact identities. Evidence of an old context never authorizes it.
use super::{valid_digest, StateKind};
use crate::domain::{
    ids::{LibraryRootId, PlaylistId, PlaylistItemId, SongId},
    library::{LibraryId, PortableSerialize},
    sync::{DeviceId, MutationId, ObjectKey, SyncBinding},
};
use crate::error::Error;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// A real freeze-time scope, or explicitly unproven historical provenance.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "scope", rename_all = "snake_case", deny_unknown_fields)]
pub enum StateNamespace {
    Bound {
        binding: SyncBinding,
    },
    LocalUnbound {
        library_id: LibraryId,
        author: DeviceId,
    },
    /// Old SQL queues/publications did not capture their original binding.
    /// The source root distinguishes evidence; the current binding is no proof.
    LegacyEvidence {
        source_root: LibraryRootId,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "fact", rename_all = "snake_case", deny_unknown_fields)]
pub enum StateTarget {
    Confirmed {
        object: ObjectKey,
    },
    Mutation {
        author: DeviceId,
        mutation_id: MutationId,
        body_digest: String,
    },
    MutationDisposition {
        author: DeviceId,
        mutation_id: MutationId,
        body_digest: String,
    },
    Receipt {
        author: DeviceId,
        mutation_id: MutationId,
    },
    Ledger {
        author: DeviceId,
        mutation_id: MutationId,
    },
    SongAlias {
        alias: SongId,
    },
    MemberAlias {
        playlist: PlaylistId,
        alias: PlaylistItemId,
    },
    OriginCursor {
        author: DeviceId,
    },
    LocalOriginCursor {
        author: DeviceId,
    },
    PeerCursor {
        peer: DeviceId,
    },
    Conflict {
        conflict_id: Uuid,
    },
    Publication {
        operation_id: Uuid,
    },
}
impl StateTarget {
    #[must_use]
    pub const fn kind(&self) -> StateKind {
        match self {
            Self::Confirmed { .. } => StateKind::Confirmed,
            Self::Mutation { .. } => StateKind::Pending,
            Self::MutationDisposition { .. } => StateKind::PendingDisposition,
            Self::Receipt { .. } => StateKind::Receipt,
            Self::Ledger { .. } => StateKind::SyncLedger,
            Self::SongAlias { .. } | Self::MemberAlias { .. } => StateKind::Alias,
            Self::OriginCursor { .. }
            | Self::LocalOriginCursor { .. }
            | Self::PeerCursor { .. } => StateKind::Cursor,
            Self::Conflict { .. } => StateKind::Conflict,
            Self::Publication { .. } => StateKind::ResourceLocation,
        }
    }
}

/// Serialized in full in both the envelope and its payload. The storage UUID is
/// derived from this key; it is deliberately distinct from a LAN mutation ID.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StateKey {
    pub namespace: StateNamespace,
    pub target: StateTarget,
}
impl StateKey {
    /// # Errors
    /// Rejects empty identities or an invalid namespace.
    pub fn validate(&self) -> Result<(), Error> {
        match self.namespace {
            StateNamespace::Bound { binding }
                if binding.library_id.as_uuid().is_nil()
                    || binding.authority_id.as_uuid().is_nil()
                    || binding.epoch.as_u64() == 0 =>
            {
                return Err(Error::conflict("invalid bound state namespace"))
            }
            StateNamespace::LocalUnbound { library_id, author }
                if library_id.as_uuid().is_nil() || author.as_uuid().is_nil() =>
            {
                return Err(Error::conflict("invalid unbound state namespace"))
            }
            StateNamespace::LegacyEvidence { source_root } if source_root.as_uuid().is_nil() => {
                return Err(Error::conflict("invalid legacy evidence source"))
            }
            _ => {}
        }
        let identities_valid = match &self.target {
            StateTarget::Confirmed { object } => !object_identity(*object).1.is_nil(),
            StateTarget::Mutation {
                author,
                mutation_id,
                ..
            }
            | StateTarget::MutationDisposition {
                author,
                mutation_id,
                ..
            }
            | StateTarget::Receipt {
                author,
                mutation_id,
            }
            | StateTarget::Ledger {
                author,
                mutation_id,
            } => !author.as_uuid().is_nil() && !mutation_id.as_uuid().is_nil(),
            StateTarget::SongAlias { alias } => !alias.as_uuid().is_nil(),
            StateTarget::MemberAlias { playlist, alias } => {
                !playlist.as_uuid().is_nil() && !alias.as_uuid().is_nil()
            }
            StateTarget::OriginCursor { author } | StateTarget::LocalOriginCursor { author } => {
                !author.as_uuid().is_nil()
            }
            StateTarget::PeerCursor { peer } => !peer.as_uuid().is_nil(),
            StateTarget::Conflict { conflict_id } => !conflict_id.is_nil(),
            StateTarget::Publication { operation_id } => !operation_id.is_nil(),
        };
        if !identities_valid {
            return Err(Error::conflict("nil typed state target"));
        }
        if let StateNamespace::LocalUnbound { author, .. } = self.namespace {
            match &self.target {
                StateTarget::Mutation { author: target, .. }
                | StateTarget::MutationDisposition { author: target, .. }
                | StateTarget::LocalOriginCursor { author: target }
                    if author == *target => {}
                _ => {
                    return Err(Error::conflict(
                        "unbound state requires its actual local author",
                    ))
                }
            }
        }
        if let StateTarget::Mutation { body_digest, .. }
        | StateTarget::MutationDisposition { body_digest, .. } = &self.target
        {
            digest_valid(body_digest)?;
        }
        Ok(())
    }
    /// # Errors
    /// Reports canonical key serialization failure.
    pub fn canonical(&self) -> Result<String, Error> {
        self.to_canonical_json()
    }
    /// # Errors
    /// Rejects invalid keys or serialization failures.
    pub fn storage_id(&self) -> Result<Uuid, Error> {
        self.validate()?;
        let mut digest = blake3::Hasher::new();
        digest.update(b"echo.durable-state-key.v1\0");
        digest.update(self.canonical()?.as_bytes());
        let mut bytes = [0; 16];
        bytes.copy_from_slice(&digest.finalize().as_bytes()[..16]);
        Ok(Uuid::from_bytes(bytes))
    }
}

pub(super) fn encode(error: serde_json::Error) -> Error {
    Error::unavailable_with_source("durable state", "cannot encode typed fact", error)
}

/// Validate the original digest syntax without treating it as authorization.
pub(super) fn digest_valid(value: &str) -> Result<(), Error> {
    if valid_digest(value) {
        Ok(())
    } else {
        Err(Error::conflict("invalid frozen mutation digest"))
    }
}

pub(super) fn validate_payload(key: &StateKey, payload: &serde_json::Value) -> Result<(), Error> {
    if payload.get("schema").and_then(serde_json::Value::as_str)
        == Some("echo.sync.resource-publication.v2")
    {
        super::publication::validate(key, payload)?;
    }
    if payload.get("schema").and_then(serde_json::Value::as_str) == Some("echo.sync.confirmed.v2") {
        super::confirmed::validate(key, payload)?;
    }
    if payload.get("schema").and_then(serde_json::Value::as_str) == Some("echo.sync.receipt.v2") {
        super::receipt::validate(key, payload)?;
    }
    let uuid = |field: &str| -> Result<Uuid, Error> {
        payload
            .get(field)
            .and_then(serde_json::Value::as_str)
            .and_then(|v| v.parse().ok())
            .ok_or_else(|| Error::conflict("scoped state payload has no typed identity"))
    };
    let matches = match &key.target {
        StateTarget::Confirmed { object } => {
            let (kind, id) = object_identity(*object);
            payload
                .get("object_type")
                .and_then(serde_json::Value::as_str)
                == Some(kind)
                && uuid("object_uuid")? == id
        }
        StateTarget::Receipt {
            author,
            mutation_id,
        }
        | StateTarget::Ledger {
            author,
            mutation_id,
        } => uuid("author")? == author.as_uuid() && uuid("mutation_id")? == mutation_id.as_uuid(),
        StateTarget::SongAlias { alias } => uuid("alias")? == alias.as_uuid(),
        StateTarget::MemberAlias { playlist, alias } => {
            uuid("playlist")? == playlist.as_uuid() && uuid("alias")? == alias.as_uuid()
        }
        StateTarget::OriginCursor { author } | StateTarget::LocalOriginCursor { author } => {
            uuid("author")? == author.as_uuid()
        }
        StateTarget::PeerCursor { peer } => uuid("peer")? == peer.as_uuid(),
        StateTarget::Conflict { conflict_id } => uuid("id")? == *conflict_id,
        StateTarget::Publication { operation_id } => uuid("operation")? == *operation_id,
        StateTarget::Mutation { .. } | StateTarget::MutationDisposition { .. } => false,
    };
    if !matches {
        return Err(Error::conflict(
            "scoped state payload identity differs from its typed key",
        ));
    }
    if let StateTarget::Ledger { .. } = &key.target {
        let StateNamespace::Bound { binding } = key.namespace else {
            return Err(Error::conflict(
                "commit ledger requires its original binding",
            ));
        };
        if uuid("library")? != binding.library_id.as_uuid()
            || uuid("authority")? != binding.authority_id.as_uuid()
            || payload.get("epoch").and_then(serde_json::Value::as_u64)
                != Some(binding.epoch.as_u64())
        {
            return Err(Error::conflict("commit ledger payload crosses bindings"));
        }
    }
    Ok(())
}

pub(super) const fn object_identity(object: ObjectKey) -> (&'static str, Uuid) {
    match object {
        ObjectKey::Song(id) => ("song", id.as_uuid()),
        ObjectKey::Playlist(id) => ("playlist", id.as_uuid()),
        ObjectKey::PlaylistItem(id) => ("playlist-item", id.as_uuid()),
        ObjectKey::LibraryRoot(id) => ("library-root", id),
        ObjectKey::Favorite(id) => ("favorite", id.as_uuid()),
        ObjectKey::Override(id) => ("override", id.as_uuid()),
        ObjectKey::PlayStats(id) => ("play-stats", id.as_uuid()),
    }
}
