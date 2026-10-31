//! Proofs shared by one recovery pass; never retained across database changes.
use super::{storage, DurableStateChange, Error, FrozenMutation, StateTarget};
use crate::domain::library::durability::StateKey;
use rusqlite::Connection;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use uuid::Uuid;

pub(in super::super) struct AllocationIndex<'a> {
    states: &'a [DurableStateChange],
    by_key: BTreeMap<String, usize>,
    authors: BTreeMap<Uuid, AuthorEvidence>,
    decoded: BTreeMap<Uuid, Arc<FrozenMutation>>,
    validated: BTreeSet<usize>,
    local_author: Option<String>,
}

#[derive(Default)]
pub(super) struct AuthorEvidence {
    conflicting_sequences: BTreeSet<u64>,
    conflicting_mutations: BTreeSet<Uuid>,
    predecessors: BTreeSet<(u64, Uuid)>,
}

impl AuthorEvidence {
    pub(super) fn conflicts(&self, frozen: &FrozenMutation) -> bool {
        self.conflicting_sequences
            .contains(&frozen.candidate.origin_seq)
            || self
                .conflicting_mutations
                .contains(&frozen.candidate.mutation_id.as_uuid())
    }

    pub(super) fn contains(&self, sequence: u64, mutation: Uuid) -> bool {
        self.predecessors.contains(&(sequence, mutation))
    }
}

impl<'a> AllocationIndex<'a> {
    pub(in super::super) fn new(states: &'a [DurableStateChange]) -> Result<Self, Error> {
        let mut by_key = BTreeMap::new();
        for (position, state) in states.iter().enumerate() {
            if let Some(key) = &state.key {
                // Match the first occurrence just as the original linear search
                // did. Validation below still checks the persisted storage id.
                by_key
                    .entry(serde_json::to_string(key).map_err(storage)?)
                    .or_insert(position);
            }
        }
        Ok(Self {
            states,
            by_key,
            authors: BTreeMap::new(),
            decoded: BTreeMap::new(),
            validated: BTreeSet::new(),
            local_author: None,
        })
    }

    pub(super) fn paired(&self, key: &StateKey) -> Result<Option<&'a DurableStateChange>, Error> {
        let key = serde_json::to_string(key).map_err(storage)?;
        Ok(self
            .by_key
            .get(&key)
            .map(|position| &self.states[*position]))
    }

    pub(super) fn local_author(&mut self, connection: &Connection) -> Result<String, Error> {
        if self.local_author.is_none() {
            self.local_author = Some(
                connection
                    .query_row(
                        "SELECT value FROM device_state WHERE key='device_id'",
                        [],
                        |row| row.get(0),
                    )
                    .map_err(storage)?,
            );
        }
        Ok(self.local_author.as_ref().unwrap().clone())
    }

    #[allow(clippy::set_contains_or_insert)] // Failed validation must never populate the cache.
    pub(super) fn validate(&mut self, state: &DurableStateChange) -> Result<(), Error> {
        let address = std::ptr::from_ref(state) as usize;
        if !self.validated.contains(&address) {
            state.validate()?;
            self.validated.insert(address);
        }
        Ok(())
    }

    pub(super) fn frozen(
        &mut self,
        state: &DurableStateChange,
    ) -> Result<Arc<FrozenMutation>, Error> {
        self.validate(state)?;
        if let std::collections::btree_map::Entry::Vacant(entry) = self.decoded.entry(state.id) {
            let frozen =
                serde_json::from_value(state.payload["frozen"].clone()).map_err(storage)?;
            entry.insert(Arc::new(frozen));
        }
        Ok(Arc::clone(&self.decoded[&state.id]))
    }

    pub(super) fn author(&mut self, author: Uuid) -> Result<&AuthorEvidence, Error> {
        if !self.authors.contains_key(&author) {
            let mut evidence = AuthorEvidence::default();
            let mut sequences = BTreeMap::new();
            let mut mutations = BTreeMap::<Uuid, Arc<FrozenMutation>>::new();
            for state in self.states {
                if !state.key.as_ref().is_some_and(|key| {
                    matches!(key.target, StateTarget::Mutation { author: other, .. } if other.as_uuid() == author)
                }) {
                    continue;
                }
                let frozen = self.frozen(state)?;
                let candidate = &frozen.candidate;
                let mutation = candidate.mutation_id.as_uuid();
                if sequences
                    .insert(candidate.origin_seq, mutation)
                    .is_some_and(|previous| previous != mutation)
                {
                    evidence.conflicting_sequences.insert(candidate.origin_seq);
                }
                if mutations
                    .get(&mutation)
                    .is_some_and(|previous| previous != &frozen)
                {
                    evidence.conflicting_mutations.insert(mutation);
                }
                mutations
                    .entry(mutation)
                    .or_insert_with(|| Arc::clone(&frozen));
                evidence
                    .predecessors
                    .insert((candidate.origin_seq, mutation));
            }
            self.authors.insert(author, evidence);
        }
        Ok(&self.authors[&author])
    }

    #[cfg(test)]
    pub(super) fn proof_counts(&self) -> (usize, usize) {
        (self.validated.len(), self.authors.len())
    }
}
