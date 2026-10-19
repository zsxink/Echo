//! Host-side lifecycle methods split from the former monolithic
//! `sync_provisioner.rs`: remote-delete isolation retry, published
//! metadata refresh and leave-sync teardown.

#[allow(clippy::wildcard_imports)] // 拆分自单文件：继承父模块导入域
use super::*;

impl NativeSyncProvisioner {
    /// Finish the LAN-sync remote-delete isolation retry pass (task 4.5).
    /// Tolerant per intent: one violated/missing intent stays pending with a
    /// warning and never fails the sync cycle that ran this pass.
    pub(super) fn reconcile_remote_delete_isolations(&self, root: LibraryRootId) {
        if let Ok(results) =
            echo_core::application::remote_delete::recover_remote_deletes(&self.scan_deps, root)
        {
            for (operation, result) in results {
                if let Err(error) = result {
                    tracing::warn!(
                        target: "echo_desktop::sync",
                        %operation,
                        error = %error,
                        "remote-delete isolation retry kept pending"
                    );
                }
            }
        }
    }

    pub(super) fn refresh_published_metadata(&self, root: LibraryRootId) {
        match self.database.repair_published_song_projections(root) {
            Ok(count) if count > 0 => self.pending_catalog_changed.store(true, Ordering::Release),
            Ok(_) => {}
            Err(error) => {
                tracing::warn!(target: "echo_desktop::sync", %error, "confirmed song tags will retry next cycle");
                return;
            }
        }
        let songs = match self.database.pending_remote_song_metadata(root) {
            Ok(songs) => songs,
            Err(error) => {
                tracing::warn!(
                    target: "echo_desktop::sync",
                    error = %error,
                    "published song metadata indexing will retry next cycle"
                );
                return;
            }
        };
        let indexer = StartScan::new(&self.scan_deps, &self.scan_supervisor);
        for song in songs {
            if let Err(error) = indexer.refresh_published_song(root, song) {
                // File transfer has already committed. Keep the NULL scan
                // stamp and retry next cycle, without reporting a transport
                // failure or blocking other songs on one bad metadata read.
                tracing::warn!(
                    target: "echo_desktop::sync",
                    song = %song,
                    error = %error,
                    "published song metadata indexing will retry next cycle"
                );
            } else {
                self.pending_catalog_changed.store(true, Ordering::Release);
            }
        }
    }

    pub(super) fn leave_sync_locked(&self, root: LibraryRootId) -> Result<(), SyncRuntimeError> {
        self.database
            .select_sync_context(root)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        let configuration = self
            .database
            .current_sync_configuration()
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        let detached_binding = configuration
            .as_ref()
            .map(|configuration| configuration.binding);
        let mut credentials = Vec::new();
        if let Some(configuration) = configuration.as_ref() {
            if configuration.host {
                for (_, handle) in self
                    .database
                    .authorized_sync_peer_handles()
                    .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
                {
                    credentials.push(
                        SecretHandle::new(handle).map_err(|_| SyncRuntimeError::Unconfigured)?,
                    );
                }
                credentials.push(identity_handle(configuration.binding)?);
                credentials.push(
                    SecretHandle::new(format!(
                        "checkpoint-{}-{}-{}",
                        configuration.binding.library_id.as_uuid(),
                        configuration.binding.authority_id.as_uuid(),
                        configuration.binding.epoch.as_u64()
                    ))
                    .map_err(|_| SyncRuntimeError::Unconfigured)?,
                );
            } else {
                credentials.push(peer_identity_handle(configuration.binding, root)?);
            }
        }
        // Revoke database authorization before deleting credentials. If secure
        // cleanup fails, stale credentials still cannot authenticate.
        self.database
            .detach_lan_sync_context(root)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        if let Some(binding) = detached_binding {
            if self
                .local_resource_store(binding)
                .and_then(|store| {
                    store
                        .clear_binding(binding)
                        .map_err(|_| SyncRuntimeError::LibraryUnavailable)
                })
                .is_err()
            {
                tracing::warn!(
                    target: "echo_desktop::sync",
                    "detached sync resource cache cleanup failed"
                );
            }
        }
        for handle in credentials {
            match OsSecureStore.delete(&handle) {
                Ok(()) => {}
                // The vault delete API reports missing and unreadable alike;
                // the binding is already detached, so leave remains safe.
                Err(_) => {
                    tracing::warn!(target: "echo_desktop::sync", "stale sync credential cleanup failed");
                }
            }
        }
        Ok(())
    }
}
