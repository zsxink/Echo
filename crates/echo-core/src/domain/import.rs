//! Identity and target-conflict rules shared by import clients.
//!
//! This is deliberately independent of filesystems and repositories: adapters
//! supply the snapshot, while the domain service owns the decisions that make
//! a batch deduplicated and target-safe.

use std::collections::{HashMap, HashSet};

use crate::domain::ids::SongId;

/// Snapshot plus reservations accumulated while importing one batch.
#[derive(Default)]
pub struct ImportConflictIndex {
    holders: HashMap<String, SongId>,
    occupied_targets: HashSet<String>,
}

impl ImportConflictIndex {
    /// Records an existing content holder. The first holder wins so repeated
    /// catalogue rows cannot make a deduplication result non-deterministic.
    pub fn record_content(&mut self, hash: impl Into<String>, song: SongId) {
        self.holders.entry(hash.into()).or_insert(song);
    }

    /// Records a target that may not be overwritten by this batch.
    pub fn occupy_target(&mut self, identity_key: impl Into<String>) {
        self.occupied_targets.insert(identity_key.into());
    }

    /// Returns the existing song with identical content, if any.
    #[must_use]
    pub fn duplicate_of(&self, hash: &str) -> Option<SongId> {
        self.holders.get(hash).copied()
    }

    /// Whether a root-relative target identity is occupied or reserved.
    #[must_use]
    pub fn target_is_occupied(&self, identity_key: &str) -> bool {
        self.occupied_targets.contains(identity_key)
    }

    /// Reserves an imported content hash after a successful commit.
    pub fn record_import(&mut self, hash: impl Into<String>, song: SongId) {
        self.record_content(hash, song);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_content_holder_and_target_reservation_are_stable() {
        let first = SongId::new();
        let second = SongId::new();
        let mut index = ImportConflictIndex::default();
        index.record_content("same", first);
        index.record_content("same", second);
        index.occupy_target("media/a.flac");

        assert_eq!(index.duplicate_of("same"), Some(first));
        assert!(index.target_is_occupied("media/a.flac"));
    }
}
