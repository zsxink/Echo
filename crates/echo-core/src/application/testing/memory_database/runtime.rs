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

impl ResourceRetentionPort for MemoryDatabase {
    fn remote_delete_isolations(
        &self,
        root: LibraryRootId,
    ) -> Result<Vec<RemoteDeleteIsolation>, Error> {
        let store = self.lock();
        Ok(store
            .remote_delete_isolations
            .iter()
            .filter(|((candidate, operation), _)| {
                *candidate == root
                    && !store
                        .finished_isolations
                        .contains(&(*candidate, *operation))
            })
            .map(|(_, intent)| intent.clone())
            .collect())
    }
    fn finish_remote_delete_isolation(
        &self,
        root: LibraryRootId,
        operation: OperationId,
    ) -> Result<(), Error> {
        self.lock().finished_isolations.insert((root, operation));
        Ok(())
    }
    fn remote_publication_matches(
        &self,
        _root: LibraryRootId,
        _path: &RelativeMediaPath,
        _digest: &str,
    ) -> Result<bool, Error> {
        Ok(false)
    }
    fn rebuild_resource_holds(&self, _root: LibraryRootId) -> Result<(), Error> {
        Ok(())
    }
    fn record_retained_resource(
        &self,
        root: LibraryRootId,
        resource: RetainedResource,
    ) -> Result<(), Error> {
        self.lock()
            .retained_resources
            .insert((root, resource.digest), resource);
        Ok(())
    }
    fn retained_resource(
        &self,
        root: LibraryRootId,
        digest: crate::domain::sync::ResourceDigest,
    ) -> Result<Option<RetainedResource>, Error> {
        Ok(self.lock().retained_resources.get(&(root, digest)).cloned())
    }
}
