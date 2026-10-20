//! Atomic local snapshots are independent from resumable network chunks.

use std::fs::{self, OpenOptions};
use std::io::Read;
use std::path::Path;
use std::time::{Duration, SystemTime};

use echo_core::domain::sync::DeviceId;

use super::{
    regular_file, sync_directory, validate_hash, verify_file, Checkpoint, FileResourceStore,
    PrepareResource, TransferError, TransferProgress, MAX_RESOURCE_BYTES,
};
use crate::protocol::{validate_resource, MAX_RESOURCE_CHUNK_BYTES};

impl FileResourceStore {
    /// Import a mutable local reader without changing resumable network chunks
    /// until a private snapshot has passed full length and digest validation.
    ///
    /// # Errors
    /// Rejects unsafe declarations, changed source content, or storage failures.
    /// A failed snapshot leaves existing upload checkpoints and parts untouched.
    pub fn import_local_reader(
        &self,
        owner: DeviceId,
        request: &PrepareResource,
        reader: &mut dyn Read,
    ) -> Result<TransferProgress, TransferError> {
        validate_resource(&request.resource).map_err(|_| TransferError::Invalid)?;
        if request.resource.length > MAX_RESOURCE_BYTES
            || (request.resource.kind != echo_core::domain::sync::SharedResourceKind::Audio
                && request.resource.length > 32 * 1024 * 1024)
        {
            return Err(TransferError::Capacity);
        }
        let _lock = self.lock.lock().map_err(|_| TransferError::Conflict)?;
        let directory = self.directory(request.binding)?;
        cleanup_stale_snapshots(&directory, Duration::from_secs(24 * 60 * 60))?;
        let nonce = blake3::hash(DeviceId::new().as_uuid().as_bytes());
        let temporary = directory.join(format!("local-{}.tmp", nonce.to_hex()));
        let result = (|| {
            let mut snapshot = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            let copied =
                std::io::copy(&mut reader.take(request.resource.length + 1), &mut snapshot)?;
            if copied != request.resource.length {
                return Err(TransferError::Conflict);
            }
            snapshot.sync_all()?;
            drop(snapshot);
            verify_file(&temporary, &request.resource)?;
            let blob = directory.join(format!("{}.blob", request.resource.content_hash));
            if blob.exists() {
                verify_file(&blob, &request.resource)?;
            } else {
                match fs::hard_link(&temporary, &blob) {
                    Ok(()) => sync_directory(&directory)?,
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                        verify_file(&blob, &request.resource)?;
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            let encoded =
                serde_json::to_vec(&(owner, request)).map_err(|_| TransferError::Invalid)?;
            let progress = TransferProgress {
                upload_id: blake3::hash(&encoded).to_hex().to_string(),
                resource: request.resource.clone(),
                chunk_size: MAX_RESOURCE_CHUNK_BYTES,
                confirmed_chunks: Vec::new(),
                complete: true,
            };
            Self::save(
                &directory,
                &Checkpoint {
                    owner,
                    binding: request.binding,
                    progress: progress.clone(),
                },
            )?;
            let part = directory.join(format!("{}.part", progress.upload_id));
            if part.exists() {
                regular_file(&part)?;
                fs::remove_file(part)?;
                sync_directory(&directory)?;
            }
            Ok(progress)
        })();
        // This attempt owns only this random temporary path. Failed imports
        // never reset a network session or discard another source's progress.
        if temporary.exists() {
            fs::remove_file(&temporary)?;
            sync_directory(&directory)?;
        }
        result
    }
}

/// Reclaim only stale snapshots owned by this module. Recent files may belong
/// to a different app instance; network `.json.tmp` files are never selected.
pub(super) fn cleanup_stale_snapshots(
    directory: &Path,
    max_age: Duration,
) -> Result<(), TransferError> {
    let now = SystemTime::now();
    let mut removed = false;
    for entry in fs::read_dir(directory)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let Some(nonce) = name
            .strip_prefix("local-")
            .and_then(|name| name.strip_suffix(".tmp"))
        else {
            continue;
        };
        if validate_hash(nonce).is_err() {
            continue;
        }
        regular_file(&path)?;
        let old = fs::metadata(&path)?
            .modified()
            .ok()
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age >= max_age);
        if old {
            fs::remove_file(&path)?;
            removed = true;
        }
    }
    if removed {
        sync_directory(directory)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::ResourceRefDto;
    use crate::resource_transfer::ResourceTransferPort;
    use echo_core::domain::sync::{AuthorityId, Epoch, LibraryId, SharedResourceKind, SyncBinding};
    use std::io::Cursor;

    fn request(bytes: &[u8]) -> PrepareResource {
        PrepareResource {
            binding: SyncBinding {
                library_id: LibraryId::new(),
                authority_id: AuthorityId::new(),
                epoch: Epoch::INITIAL,
            },
            resource: ResourceRefDto {
                kind: SharedResourceKind::Audio,
                content_hash: blake3::hash(bytes).to_hex().to_string(),
                length: bytes.len() as u64,
                media_type: "audio/flac".into(),
                relative_path: "media/local.flac".into(),
            },
        }
    }

    fn assert_no_temporary_files(directory: &Path) {
        assert!(fs::read_dir(directory).unwrap().all(|entry| {
            entry
                .unwrap()
                .path()
                .extension()
                .is_none_or(|extension| extension != "tmp")
        }));
    }

    #[test]
    fn changed_local_snapshot_preserves_network_chunks_and_correct_retry_completes() {
        let directory = tempfile::tempdir().unwrap();
        let store = FileResourceStore::new(directory.path().canonicalize().unwrap()).unwrap();
        let bytes = vec![b'A'; MAX_RESOURCE_CHUNK_BYTES + 7];
        let request = request(&bytes);
        let owner = DeviceId::new();
        let network = store.prepare(owner, &request).unwrap();
        let chunk = &bytes[..MAX_RESOURCE_CHUNK_BYTES];
        store
            .write_chunk(
                request.binding,
                owner,
                &network.upload_id,
                0,
                blake3::hash(chunk).to_hex().as_ref(),
                chunk,
            )
            .unwrap();
        let partition = store.directory(request.binding).unwrap();
        let checkpoint_path = partition.join(format!("{}.json", network.upload_id));
        let part_path = partition.join(format!("{}.part", network.upload_id));
        let checkpoint = fs::read(&checkpoint_path).unwrap();
        let partial = fs::read(&part_path).unwrap();
        // The original file supplied the correct prefix, then was rewritten
        // during the copy. These bytes must never become confirmed chunks.
        let mut changing =
            Cursor::new(&bytes[..32]).chain(Cursor::new(vec![b'B'; bytes.len() - 32]));
        assert!(matches!(
            store.import_local_reader(owner, &request, &mut changing),
            Err(TransferError::Conflict)
        ));
        assert_eq!(fs::read(&checkpoint_path).unwrap(), checkpoint);
        assert_eq!(fs::read(&part_path).unwrap(), partial);
        assert_no_temporary_files(&partition);
        let complete = store
            .import_local_reader(owner, &request, &mut Cursor::new(&bytes))
            .unwrap();
        assert!(complete.complete);
        store
            .verify_complete(request.binding, &request.resource)
            .unwrap();
        assert!(!part_path.exists());
        assert_eq!(
            store
                .download(
                    request.binding,
                    &request.resource.content_hash,
                    MAX_RESOURCE_CHUNK_BYTES as u64,
                    7
                )
                .unwrap()
                .bytes,
            b"AAAAAAA"
        );
    }

    struct FailedReader;
    impl Read for FailedReader {
        fn read(&mut self, _buffer: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("local source changed during copy"))
        }
    }

    #[test]
    fn invalid_or_failed_local_snapshots_leave_no_checkpoint_and_can_retry() {
        let bytes = b"expected local audio";
        let sources: Vec<Box<dyn Read>> = vec![
            Box::new(Cursor::new(vec![b'X'; bytes.len()])),
            Box::new(Cursor::new(&bytes[..bytes.len() - 1])),
            Box::new(Cursor::new([bytes.as_slice(), b"extra"].concat())),
            Box::new(Cursor::new(&bytes[..4]).chain(FailedReader)),
        ];
        for mut source in sources {
            let directory = tempfile::tempdir().unwrap();
            let store = FileResourceStore::new(directory.path().canonicalize().unwrap()).unwrap();
            let request = request(bytes);
            let owner = DeviceId::new();
            assert!(store
                .import_local_reader(owner, &request, source.as_mut())
                .is_err());
            let partition = store.directory(request.binding).unwrap();
            assert_eq!(fs::read_dir(&partition).unwrap().count(), 0);
            let complete = store
                .import_local_reader(owner, &request, &mut Cursor::new(bytes))
                .unwrap();
            assert!(complete.complete);
            store
                .verify_complete(request.binding, &request.resource)
                .unwrap();
            assert_no_temporary_files(&partition);
        }
    }
    #[test]
    fn completed_cleanup_recovers_part_left_after_local_snapshot_completion() {
        let directory = tempfile::tempdir().unwrap();
        let store = FileResourceStore::new(directory.path().canonicalize().unwrap()).unwrap();
        let bytes = b"verified audio";
        let request = request(bytes);
        let owner = DeviceId::new();
        let progress = store
            .import_local_reader(owner, &request, &mut Cursor::new(bytes))
            .unwrap();
        let partition = store.directory(request.binding).unwrap();
        let part = partition.join(format!("{}.part", progress.upload_id));
        // Simulate a crash after the completed checkpoint was written but
        // before reclaiming the now redundant resumable network part.
        fs::write(&part, bytes).unwrap();
        store.cleanup_completed(request.binding).unwrap();
        assert_eq!(fs::read_dir(partition).unwrap().count(), 0);
    }

    #[test]
    fn stale_snapshot_cleanup_keeps_unrelated_temporary_files() {
        let directory = tempfile::tempdir().unwrap();
        let store = FileResourceStore::new(directory.path().canonicalize().unwrap()).unwrap();
        let request = request(b"audio");
        let partition = store.directory(request.binding).unwrap();
        let orphan = partition.join(format!("local-{}.tmp", blake3::hash(b"orphan").to_hex()));
        let checkpoint_temporary =
            partition.join(format!("{}.json.tmp", blake3::hash(b"network").to_hex()));
        let unrelated = partition.join("local-foreign.tmp");
        fs::write(&orphan, b"partial snapshot").unwrap();
        fs::write(&checkpoint_temporary, b"durable network checkpoint").unwrap();
        fs::write(&unrelated, b"not a snapshot").unwrap();
        cleanup_stale_snapshots(&partition, Duration::from_secs(24 * 60 * 60)).unwrap();
        assert!(
            orphan.exists(),
            "recent snapshots may belong to another process"
        );
        store
            .cleanup_stale(request.binding, &[], Duration::ZERO)
            .unwrap();
        assert!(!orphan.exists());
        assert!(checkpoint_temporary.exists());
        assert!(unrelated.exists());
    }
}
