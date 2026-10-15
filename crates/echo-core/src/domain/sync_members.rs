//! Typed playlist-member de-duplication and historical alias resolution.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use super::ids::{PlaylistId, PlaylistItemId, SongId};

#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct MemberAliasKey {
    pub playlist_id: PlaylistId,
    pub alias_member_id: PlaylistItemId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PlaylistMemberAlias {
    pub playlist_id: PlaylistId,
    pub alias_member_id: PlaylistItemId,
    pub canonical_member_id: PlaylistItemId,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AliasError {
    SelfAlias,
    AlreadyMapped,
    CyclicAlias,
}

/// Historical aliases are scoped to one playlist and immutable. Removing an
/// aliased member never redirects the old identity to a later re-add.
#[derive(Clone, Debug, Default)]
pub struct MemberAliasRegistry {
    aliases: BTreeMap<MemberAliasKey, PlaylistItemId>,
}

impl MemberAliasRegistry {
    /// Persist an immutable mapping to a canonical member.
    ///
    /// # Errors
    /// Rejects self aliases, a changed mapping, or an aliased target.
    pub fn insert(&mut self, alias: PlaylistMemberAlias) -> Result<(), AliasError> {
        if alias.alias_member_id == alias.canonical_member_id {
            return Err(AliasError::SelfAlias);
        }
        let key = MemberAliasKey {
            playlist_id: alias.playlist_id,
            alias_member_id: alias.alias_member_id,
        };
        let target_key = MemberAliasKey {
            playlist_id: alias.playlist_id,
            alias_member_id: alias.canonical_member_id,
        };
        if self.aliases.contains_key(&target_key) {
            return Err(AliasError::CyclicAlias);
        }
        match self.aliases.get(&key) {
            Some(existing) if *existing != alias.canonical_member_id => {
                Err(AliasError::AlreadyMapped)
            }
            Some(_) => Ok(()),
            None => {
                self.aliases.insert(key, alias.canonical_member_id);
                Ok(())
            }
        }
    }

    #[must_use]
    pub fn resolve(&self, playlist: PlaylistId, member: PlaylistItemId) -> PlaylistItemId {
        let mut current = member;
        // Bound traversal defensively; insertion rules normally prevent cycles
        // once aliases are checked by the persistent adapter.
        for _ in 0..self.aliases.len() {
            let Some(next) = self.aliases.get(&MemberAliasKey {
                playlist_id: playlist,
                alias_member_id: current,
            }) else {
                break;
            };
            if *next == current {
                break;
            }
            current = *next;
        }
        current
    }

    pub fn aliases(&self) -> impl Iterator<Item = PlaylistMemberAlias> + '_ {
        self.aliases
            .iter()
            .map(|(key, canonical)| PlaylistMemberAlias {
                playlist_id: key.playlist_id,
                alias_member_id: key.alias_member_id,
                canonical_member_id: *canonical,
            })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ConfirmedMember {
    pub playlist_id: PlaylistId,
    pub member_id: PlaylistItemId,
    pub song_id: SongId,
    pub confirmed_order: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MemberAppendDecision {
    Added(ConfirmedMember),
    Duplicate {
        canonical_member_id: PlaylistItemId,
        alias_member_id: PlaylistItemId,
    },
}

/// Resolve a concurrent append for a unique `(playlist, song)` pair. The
/// caller presents the earliest host-confirmed member as `existing`.
#[must_use]
pub fn decide_member_append(
    existing: &[ConfirmedMember],
    candidate: ConfirmedMember,
) -> MemberAppendDecision {
    existing
        .iter()
        .find(|member| {
            member.playlist_id == candidate.playlist_id && member.song_id == candidate.song_id
        })
        .map_or(MemberAppendDecision::Added(candidate), |canonical| {
            MemberAppendDecision::Duplicate {
                canonical_member_id: canonical.member_id,
                alias_member_id: candidate.member_id,
            }
        })
}
