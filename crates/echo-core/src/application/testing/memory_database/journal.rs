//! `OperationJournalRepository` — the operation/undo bookkeeping.

use super::*;

impl OperationJournalRepository for MemoryDatabase {
    fn ensure_operation(
        &self,
        operation: OperationId,
        root: LibraryRootId,
        kind: &str,
        reserved_song: Option<SongId>,
    ) -> Result<(), Error> {
        self.lock()
            .envelopes
            .entry(operation)
            .or_insert_with(|| (root, kind.to_owned(), reserved_song));
        Ok(())
    }
    fn item_state(
        &self,
        operation: OperationId,
        item: &str,
    ) -> Result<Option<OperationItem>, Error> {
        Ok(self
            .lock()
            .operations
            .get(&(operation, item.to_owned()))
            .cloned())
    }
    fn upsert_item(&self, operation: OperationId, item: OperationItem) -> Result<(), Error> {
        self.lock()
            .operations
            .insert((operation, item.item_key.clone()), item);
        Ok(())
    }
    fn items(&self, operation: OperationId) -> Result<Vec<OperationItem>, Error> {
        Ok(self
            .lock()
            .operations
            .iter()
            .filter(|((op, _), _)| *op == operation)
            .map(|(_, item)| item.clone())
            .collect())
    }
    fn release_claims(&self, operation: OperationId) -> Result<(), Error> {
        self.lock().released_claims.push(operation);
        Ok(())
    }

    fn set_undo_deadline(&self, operation: OperationId, deadline_ms: i64) -> Result<(), Error> {
        self.lock().undo_deadlines.insert(operation, deadline_ms);
        Ok(())
    }

    fn undo_deadline(&self, operation: OperationId) -> Result<Option<i64>, Error> {
        Ok(self.lock().undo_deadlines.get(&operation).copied())
    }

    fn incomplete_items(
        &self,
        root: LibraryRootId,
    ) -> Result<Vec<(OperationId, String, OperationItem)>, Error> {
        let store = self.lock();
        let mut out = Vec::new();
        for ((operation, _item_key), item) in &store.operations {
            // Only items whose envelope belongs to `root`.
            let Some((op_root, kind, _)) = store.envelopes.get(operation) else {
                continue;
            };
            if *op_root != root {
                continue;
            }
            if item.state.is_terminal() {
                continue;
            }
            out.push((*operation, kind.clone(), item.clone()));
        }
        Ok(out)
    }
}
