//! Apply confirmed remote state and resolve only authorized local resources.

#[allow(clippy::wildcard_imports)]
use super::*;

pub(super) struct ScopedLocalQueue<'a> {
    pub(super) database: &'a SqliteDatabase,
    pub(super) root: LibraryRootId,
}

/// Re-open only the verified private content-addressed blob. Core independently
/// rehashes this stream before changing the complete object's local association.
pub(super) struct PrivateResourceReader {
    pub(super) store: Arc<FileResourceStore>,
    pub(super) binding: SyncBinding,
    pub(super) hash: String,
    pub(super) length: u64,
    pub(super) offset: u64,
}

impl std::io::Read for PrivateResourceReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        if buffer.is_empty() || self.offset == self.length {
            return Ok(0);
        }
        let length = buffer
            .len()
            .min(MAX_RESOURCE_CHUNK_BYTES)
            .min(usize::try_from(self.length - self.offset).unwrap_or(usize::MAX));
        let chunk = self
            .store
            .download(self.binding, &self.hash, self.offset, length)
            .map_err(|_| std::io::Error::other("verified private sync resource is unavailable"))?;
        if chunk.bytes.len() != length {
            return Err(std::io::Error::other(
                "verified private sync resource length changed",
            ));
        }
        buffer[..length].copy_from_slice(&chunk.bytes);
        self.offset += length as u64;
        Ok(length)
    }
}

/// Host-authenticated apply boundary for incremental commit pages.
///
/// Media completeness is deliberately **not** a precondition. Core materializes
/// a confirmed remote song as unavailable and records the shared manifests, so
/// the catalog is readable as soon as the objects arrive; publication then
/// attaches the verified bytes from those durable manifests and restores
/// availability. This is the same order snapshot enrollment uses, and the
/// reverse (bytes before objects) parks every change behind a file transfer.
pub(super) struct PreparedRemoteApply<'a> {
    pub(super) database: &'a SqliteDatabase,
    pub(super) scan_deps: &'a echo_core::application::scan::ScanDeps,
    pub(super) root: LibraryRootId,
    pub(super) host: DeviceId,
    /// Set when the page no longer matches the authenticated host, so the
    /// caller re-pulls instead of treating it as a durable conflict.
    pub(super) changed: std::sync::atomic::AtomicBool,
    pub(super) catalog_changed: &'a AtomicBool,
}

impl RemoteApplyPort for PreparedRemoteApply<'_> {
    fn apply_remote_batch(
        &self,
        binding: SyncBinding,
        peer_device_id: DeviceId,
        anchor: CommitCheckpoint,
        commits: &[RemoteCommit],
    ) -> Result<AppliedRemoteCursor, echo_core::error::Error> {
        // Only the provisioned host may advance this replica's cursor; Core
        // separately proves the anchor and the commit chain.
        if peer_device_id != self.host || anchor.binding != binding {
            self.changed
                .store(true, std::sync::atomic::Ordering::Relaxed);
            return Err(echo_core::error::Error::conflict(
                "remote apply does not match the authenticated host",
            ));
        }
        // The page has passed the authenticated client protocol validation. If
        // it extends the current cursor, let the directory arbitrate local undo
        // against the host's confirmed final tombstone before SQLite projects
        // it. Replayed pages are applied first (Core verifies exact history),
        // then reconcile the directory intent idempotently.
        let current = self.database.replica_checkpoint(peer_device_id)?;
        let extends_current = current == anchor;
        if extends_current {
            self.supersede_local_delete_intents(commits)?;
        }
        let cursor = self
            .database
            .apply_remote_batch(binding, peer_device_id, anchor, commits)?;
        if !extends_current && cursor.applied == 0 {
            self.supersede_local_delete_intents(commits)?;
        }
        if cursor.applied > 0 {
            self.catalog_changed.store(true, Ordering::Release);
        }
        Ok(cursor)
    }
}

impl PreparedRemoteApply<'_> {
    fn supersede_local_delete_intents(
        &self,
        commits: &[RemoteCommit],
    ) -> Result<(), echo_core::error::Error> {
        let now_ms = self
            .scan_deps
            .clock
            .now_wall()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|source| {
                echo_core::error::Error::io(
                    "clock",
                    std::io::Error::other(source),
                    std::path::PathBuf::new(),
                )
            })?
            .as_millis();
        let now_ms = u64::try_from(now_ms).unwrap_or(u64::MAX);
        for commit in commits.iter().filter(|commit| commit.is_tombstone) {
            if matches!(commit.object, ObjectKey::Song(_)) {
                echo_core::application::delete::intent::supersede_object_on_remote_tombstone(
                    self.scan_deps,
                    self.root,
                    commit.object,
                    now_ms,
                )?;
            }
        }
        Ok(())
    }
}

impl LocalSyncQueuePort for ScopedLocalQueue<'_> {
    fn advance_rejected_local_mutations(&self) -> Result<usize, echo_core::error::Error> {
        self.database.advance_rejected_local_mutations()
    }

    fn pending_local_mutations(
        &self,
        limit: usize,
    ) -> Result<Vec<MutationCandidate>, echo_core::error::Error> {
        let mutations = self.database.pending_local_mutations(limit)?;
        for mutation in &mutations {
            ensure_candidate_root(self.database, self.root, mutation).map_err(|_| {
                echo_core::error::Error::conflict(
                    "local mutation is outside the bound library root",
                )
            })?;
        }
        Ok(mutations)
    }
    fn reject_local_mutation(
        &self,
        mutation_id: MutationId,
        outcome_json: String,
    ) -> Result<(), echo_core::error::Error> {
        self.database
            .reject_local_mutation(mutation_id, outcome_json)
    }
    fn replica_checkpoint(
        &self,
        host_device_id: DeviceId,
    ) -> Result<CommitCheckpoint, echo_core::error::Error> {
        self.database.replica_checkpoint(host_device_id)
    }

    /// Forwarded explicitly: the trait default refuses, and the caller folds
    /// that refusal into a library-unavailable error. The peer cycle reaches
    /// this method on every attempt once the host reports a position this
    /// chain does not continue, so a missing forward turns the one repair that
    /// could end the stall into a wrong instruction about a music directory.
    fn rebase_local_chain(
        &self,
        anchor: echo_core::domain::sync::OriginAnchor,
    ) -> Result<usize, echo_core::error::Error> {
        // The queue is scoped to one root; rebasing another root's chain would
        // renumber events this cycle never sends.
        if self
            .database
            .current_sync_configuration()?
            .is_some_and(|configuration| configuration.local_root != self.root)
        {
            return Err(echo_core::error::Error::conflict(
                "sync queue is scoped to a different library root",
            ));
        }
        self.database.rebase_local_chain(anchor)
    }

    /// Forwarded explicitly: the trait default returns `None`, which is
    /// indistinguishable from "this adapter keeps no edit time". The host then
    /// falls back to the payload's legacy HLC and every replica edit compares
    /// as zero, so the newest edit loses to the host's current value on every
    /// push — a silent rollback rather than an error.
    fn local_mutation_changed_at_ms(
        &self,
        mutation_id: MutationId,
    ) -> Result<Option<u64>, echo_core::error::Error> {
        self.database.local_mutation_changed_at_ms(mutation_id)
    }
}

pub(super) fn ensure_candidate_root(
    database: &SqliteDatabase,
    root: LibraryRootId,
    mutation: &MutationCandidate,
) -> Result<(), SyncRuntimeError> {
    use echo_core::application::ports::{PlaylistRepository, SongRepository};
    // A typed deletion removes its object row on purpose, so a live lookup can
    // never answer whether it belongs here: `SongRepository::by_id` and the
    // playlist list are both already empty by the time it is pushed. Candidate
    // selection scopes every mutation by the root recorded at enqueue time
    // (`sync_mutations.local_root_uuid`), and this function is only ever called
    // on rows that selection returned, so a deletion is authorized by its own
    // durable queue entry instead of by a vanished object. Rejecting it here
    // would wedge the whole push loop on the first delete.
    if mutation.payload == serde_json::json!({"tombstone": true}) {
        return Ok(());
    }
    let belongs = match mutation.object {
        ObjectKey::Song(id)
        | ObjectKey::Favorite(id)
        | ObjectKey::Override(id)
        | ObjectKey::PlayStats(id) => SongRepository::by_id(database, id)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
            .is_some_and(|song| song.root() == root),
        ObjectKey::Playlist(id) => PlaylistRepository::list(database, root)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
            .contains(&id),
        ObjectKey::PlaylistItem(_) => {
            let id = mutation
                .payload
                .get("playlist_uuid")
                .cloned()
                .and_then(|id| serde_json::from_value(id).ok())
                .ok_or(SyncRuntimeError::LibraryUnavailable)?;
            PlaylistRepository::list(database, root)
                .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
                .contains(&id)
        }
        ObjectKey::LibraryRoot(_) => false,
    };
    if belongs {
        Ok(())
    } else {
        Err(SyncRuntimeError::LibraryUnavailable)
    }
}

pub(super) fn peer_identity_handle(
    binding: SyncBinding,
    root: LibraryRootId,
) -> Result<SecretHandle, SyncRuntimeError> {
    SecretHandle::new(format!(
        "peer-identity-{}-{}-{}-{}",
        root,
        binding.library_id.as_uuid(),
        binding.authority_id.as_uuid(),
        binding.epoch.as_u64()
    ))
    .map_err(|_| SyncRuntimeError::Unconfigured)
}

pub(super) fn validate_peer_endpoint(endpoint: &str) -> Result<(), SyncRuntimeError> {
    let address = endpoint
        .strip_prefix("https://")
        .ok_or(SyncRuntimeError::InvalidAddress)?;
    let address = address.strip_suffix('/').unwrap_or(address);
    if endpoint.len() > 256 || address.contains(['/', '?', '#', '@']) {
        return Err(SyncRuntimeError::InvalidAddress);
    }
    let address: std::net::SocketAddr = address
        .parse()
        .map_err(|_| SyncRuntimeError::InvalidAddress)?;
    let private = match address.ip() {
        std::net::IpAddr::V4(ip) => ip.is_private() && !ip.is_unspecified(),
        std::net::IpAddr::V6(ip) => ip.segments()[0] & 0xfe00 == 0xfc00,
    };
    if !private || address.port() == 0 {
        return Err(SyncRuntimeError::InvalidAddress);
    }
    Ok(())
}

pub(super) fn identity_handle(binding: SyncBinding) -> Result<SecretHandle, SyncRuntimeError> {
    SecretHandle::new(format!(
        "host-identity-{}-{}-{}",
        binding.library_id.as_uuid(),
        binding.authority_id.as_uuid(),
        binding.epoch.as_u64(),
    ))
    .map_err(|_| SyncRuntimeError::Unconfigured)
}

#[cfg(test)]
pub(super) fn seed_audio_resources(
    store: &FileResourceStore,
    library: &LibraryRoot,
    binding: SyncBinding,
    device: DeviceId,
    manifests: &[SharedResourceManifest],
) -> Result<(), SyncRuntimeError> {
    for manifest in manifests
        .iter()
        .filter(|manifest| manifest.kind == SharedResourceKind::Audio)
    {
        let request = PrepareResource {
            binding,
            resource: ResourceRefDto::from(manifest),
        };
        let cached = store
            .prepare(device, &request)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        if cached.complete {
            // Frozen older events may refer to bytes already replaced in the
            // current library. Their previously verified cache is authoritative.
            continue;
        }
        let Some(path) =
            authorized_source(library.absolute_path(), manifest.relative_path.as_str())?
        else {
            // Missing local media does not make the library or its metadata
            // unavailable. The song record can still sync without its bytes.
            continue;
        };
        let mut source = match File::open(path) {
            Ok(source) => source,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(SyncRuntimeError::LibraryUnavailable),
        };
        let metadata = source
            .metadata()
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        if !metadata.is_file() || metadata.len() != manifest.length {
            return Err(SyncRuntimeError::LibraryUnavailable);
        }
        store
            .import_reader(device, &request, &mut source)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
    }
    Ok(())
}

pub(super) fn authorized_source(
    root: &Path,
    relative: &str,
) -> Result<Option<PathBuf>, SyncRuntimeError> {
    let root = root
        .canonicalize()
        .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
    let mut source = root.clone();
    for component in Path::new(relative).components() {
        let Component::Normal(component) = component else {
            return Err(SyncRuntimeError::LibraryUnavailable);
        };
        source.push(component);
        let metadata = match std::fs::symlink_metadata(&source) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(SyncRuntimeError::LibraryUnavailable),
        };
        if metadata.file_type().is_symlink() {
            return Err(SyncRuntimeError::LibraryUnavailable);
        }
    }
    let source = match source.canonicalize() {
        Ok(source) => source,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(SyncRuntimeError::LibraryUnavailable),
    };
    if source == root || !source.starts_with(&root) {
        return Err(SyncRuntimeError::LibraryUnavailable);
    }
    Ok(Some(source))
}

pub(super) fn validate_startup_head(
    independent: CommitCheckpoint,
    published: CommitCheckpoint,
    pending: &[CommitCheckpoint],
) -> Result<(), SyncRuntimeError> {
    // Published rows must already have reached the OS checkpoint. A checkpoint
    // ahead of them is allowed only when its exact staged commit survived the
    // crash between the secure-store write and SQLite publication marking.
    if independent.binding != published.binding
        || independent.sequence < published.sequence
        || (independent != published && !pending.contains(&independent))
        || pending.iter().any(|head| head.binding != published.binding)
    {
        return Err(SyncRuntimeError::LibraryUnavailable);
    }
    Ok(())
}
