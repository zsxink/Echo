//! `RuntimeStateStore`, `DeviceIdProvider` and `SyncStateReader` views.

use super::*;

impl RuntimeStateStore for MemoryDatabase {
    fn load(&self, key: &str) -> Result<Option<String>, Error> {
        Ok(self.lock().runtime_state.get(key).cloned())
    }
}

impl crate::application::ports::DeviceIdProvider for MemoryDatabase {
    fn current_device_id(&self) -> DeviceId {
        self.lock().device_id
    }
}

impl crate::application::ports::SyncStateReader for MemoryDatabase {
    fn outbox_revision(&self, object_type: &str, object_uuid: &str) -> Result<Revision, Error> {
        let store = self.lock();
        Ok(Revision::from_u64(
            store
                .outbox_revisions
                .get(&(object_type.to_owned(), object_uuid.to_owned()))
                .copied()
                .unwrap_or(0)
                .max(0) as u64,
        ))
    }

    fn object_hlc(
        &self,
        object_type: &str,
        object_uuid: &str,
    ) -> Result<Option<HybridLogicalClock>, Error> {
        let store = self.lock();
        Ok(store
            .object_hlcs
            .get(&(object_type.to_owned(), object_uuid.to_owned()))
            .copied()
            .filter(|hlc| *hlc != HybridLogicalClock::default()))
    }
}
