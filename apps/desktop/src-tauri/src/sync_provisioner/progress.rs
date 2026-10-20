//! Transfer progress uses acknowledged chunk checkpoints, including resumed
//! chunks. Totals cover resources discovered in the current cycle only.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use echo_core::domain::sync::{DeviceId, SyncBinding};
use echo_sync::protocol::ResourceRefDto;
use echo_sync::resource_transfer::{
    DownloadChunk, FileResourceStore, PrepareResource, ResourceTransferPort, TransferError,
    TransferProgress,
};

use crate::sync_runtime::{ProgressNotifier, SyncProgress};

const PROGRESS_INTERVAL: Duration = Duration::from_millis(100);
type ByteTotals = (u64, u64, u64, u64);

#[derive(Default)]
pub(super) struct TransferTelemetry {
    uploads: BTreeMap<String, (u64, u64)>,
    downloads: BTreeMap<String, (u64, u64)>,
    observer: Option<ProgressNotifier>,
    last_emitted: Option<(Instant, ByteTotals)>,
    sequence: u64,
}

impl TransferTelemetry {
    pub fn new(observer: Option<ProgressNotifier>) -> Self {
        Self {
            observer,
            ..Self::default()
        }
    }

    fn notification(&mut self, now: Instant) -> Option<(ProgressNotifier, u64, SyncProgress)> {
        let observer = self.observer.as_ref()?;
        let totals = self.byte_totals();
        if totals.0 == 0 && totals.2 == 0 {
            return None;
        }
        if let Some((last, previous)) = self.last_emitted {
            let complete = totals.0 == totals.1 && totals.2 == totals.3;
            if previous == totals || (!complete && now.duration_since(last) < PROGRESS_INTERVAL) {
                return None;
            }
        }
        self.last_emitted = Some((now, totals));
        self.sequence += 1;
        Some((
            Arc::clone(observer),
            self.sequence,
            SyncProgress {
                total_upload_bytes: Some(totals.0),
                uploaded_bytes: Some(totals.1),
                total_download_bytes: Some(totals.2),
                downloaded_bytes: Some(totals.3),
                ..SyncProgress::default()
            },
        ))
    }

    pub fn declare_upload(&mut self, resource: &ResourceRefDto) {
        self.uploads
            .entry(resource.content_hash.clone())
            .or_insert((resource.length, 0));
    }

    pub fn upload(&mut self, progress: &TransferProgress) {
        record(&mut self.uploads, progress);
    }

    pub fn declare_download(&mut self, resource: &ResourceRefDto) {
        self.downloads
            .entry(resource.content_hash.clone())
            .or_insert((resource.length, 0));
    }

    fn download(&mut self, progress: &TransferProgress) {
        record(&mut self.downloads, progress);
    }

    pub fn byte_totals(&self) -> (u64, u64, u64, u64) {
        let sum = |entries: &BTreeMap<String, (u64, u64)>| {
            entries
                .values()
                .fold((0_u64, 0_u64), |(total, confirmed), (length, bytes)| {
                    (
                        total.saturating_add(*length),
                        confirmed.saturating_add(*bytes),
                    )
                })
        };
        let (upload_total, uploaded) = sum(&self.uploads);
        let (download_total, downloaded) = sum(&self.downloads);
        (upload_total, uploaded, download_total, downloaded)
    }
}

/// Never invoke an application callback while holding the telemetry lock.
/// Two download store workers can publish concurrently; sequence numbers let
/// the consumer reject an earlier snapshot delivered after a later one.
pub(super) fn observe_transfer(
    telemetry: &Arc<Mutex<TransferTelemetry>>,
    update: impl FnOnce(&mut TransferTelemetry),
) {
    let notification = {
        let mut telemetry = telemetry
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        update(&mut telemetry);
        telemetry.notification(Instant::now())
    };
    if let Some((observer, sequence, progress)) = notification {
        observer(sequence, progress);
    }
}

fn record(entries: &mut BTreeMap<String, (u64, u64)>, progress: &TransferProgress) {
    let length = progress.resource.length;
    let bytes = if progress.complete {
        length
    } else {
        progress
            .confirmed_chunks
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .map(|index| {
                let offset = u64::from(index).saturating_mul(progress.chunk_size as u64);
                length
                    .saturating_sub(offset)
                    .min(progress.chunk_size as u64)
            })
            .sum()
    };
    let entry = entries
        .entry(progress.resource.content_hash.clone())
        .or_insert((length, 0));
    entry.1 = entry.1.max(bytes).min(entry.0);
}

/// Observe only successfully persisted download checkpoints. Failed chunks
/// never advance the displayed value, and retries retain the greatest durable count.
pub(super) struct TrackedDownloadStore {
    pub store: Arc<FileResourceStore>,
    pub telemetry: Arc<Mutex<TransferTelemetry>>,
}

impl TrackedDownloadStore {
    fn observe(&self, progress: TransferProgress) -> TransferProgress {
        observe_transfer(&self.telemetry, |telemetry| telemetry.download(&progress));
        progress
    }
}

impl ResourceTransferPort for TrackedDownloadStore {
    fn prepare(
        &self,
        owner: DeviceId,
        request: &PrepareResource,
    ) -> Result<TransferProgress, TransferError> {
        self.store
            .prepare(owner, request)
            .map(|progress| self.observe(progress))
    }

    fn write_chunk(
        &self,
        binding: SyncBinding,
        owner: DeviceId,
        upload: &str,
        index: u32,
        hash: &str,
        bytes: &[u8],
    ) -> Result<TransferProgress, TransferError> {
        self.store
            .write_chunk(binding, owner, upload, index, hash, bytes)
            .map(|progress| self.observe(progress))
    }

    fn complete(
        &self,
        binding: SyncBinding,
        owner: DeviceId,
        upload: &str,
    ) -> Result<TransferProgress, TransferError> {
        self.store
            .complete(binding, owner, upload)
            .map(|progress| self.observe(progress))
    }

    fn verify_complete(
        &self,
        binding: SyncBinding,
        resource: &ResourceRefDto,
    ) -> Result<(), TransferError> {
        self.store.verify_complete(binding, resource)
    }

    fn download(
        &self,
        binding: SyncBinding,
        hash: &str,
        offset: u64,
        length: usize,
    ) -> Result<DownloadChunk, TransferError> {
        self.store.download(binding, hash, offset, length)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checkpoint_totals_deduplicate_retries_and_count_short_final_chunks() {
        let resource = ResourceRefDto {
            kind: echo_core::domain::sync::SharedResourceKind::Audio,
            relative_path: "song.flac".into(),
            content_hash: "a".repeat(64),
            length: 10,
            media_type: "audio/flac".into(),
        };
        let mut telemetry = TransferTelemetry::default();
        telemetry.declare_upload(&resource);
        telemetry.declare_upload(&resource);
        let mut progress = TransferProgress {
            upload_id: "b".repeat(64),
            resource,
            chunk_size: 4,
            confirmed_chunks: vec![0, 2],
            complete: false,
        };
        telemetry.upload(&progress);
        telemetry.upload(&progress);
        assert_eq!(telemetry.byte_totals(), (10, 6, 0, 0));
        progress.confirmed_chunks = vec![0, 1, 2];
        progress.complete = true;
        telemetry.download(&progress);
        assert_eq!(telemetry.byte_totals(), (10, 6, 10, 10));
    }

    #[test]
    fn checkpoint_notifications_are_throttled_but_flush_completion() {
        let resource = ResourceRefDto {
            kind: echo_core::domain::sync::SharedResourceKind::Audio,
            relative_path: "song.flac".into(),
            content_hash: "a".repeat(64),
            length: 10,
            media_type: "audio/flac".into(),
        };
        let mut telemetry = TransferTelemetry::new(Some(Arc::new(|_, _| {})));
        telemetry.declare_upload(&resource);
        telemetry.declare_download(&resource);
        let now = Instant::now();
        let (_, first_sequence, first) = telemetry.notification(now).unwrap();
        assert_eq!(first_sequence, 1);
        assert_eq!(first.uploaded_bytes, Some(0));
        assert_eq!(first.total_download_bytes, Some(10));
        let mut progress = TransferProgress {
            upload_id: "b".repeat(64),
            resource,
            chunk_size: 4,
            confirmed_chunks: vec![0],
            complete: false,
        };
        telemetry.upload(&progress);
        telemetry.download(&progress);
        assert!(telemetry
            .notification(now + Duration::from_millis(99))
            .is_none());
        let (_, next_sequence, next) = telemetry.notification(now + PROGRESS_INTERVAL).unwrap();
        assert_eq!(next_sequence, 2);
        assert_eq!(next.uploaded_bytes, Some(4));
        assert_eq!(next.downloaded_bytes, Some(4));
        // A duplicate acknowledged index and a late retry checkpoint cannot
        // overcount or move the visible byte count backwards.
        progress.confirmed_chunks = vec![0, 0, 1];
        telemetry.upload(&progress);
        progress.confirmed_chunks = vec![0];
        telemetry.upload(&progress);
        assert_eq!(telemetry.byte_totals().1, 8);
        progress.complete = true;
        telemetry.upload(&progress);
        telemetry.download(&progress);
        let (_, sequence, complete) = telemetry
            .notification(now + PROGRESS_INTERVAL + Duration::from_millis(1))
            .unwrap();
        assert_eq!(sequence, 3);
        assert_eq!(complete.uploaded_bytes, Some(10));
        assert_eq!(complete.downloaded_bytes, Some(10));
        assert!(telemetry
            .notification(now + Duration::from_secs(1))
            .is_none());
    }

    #[test]
    fn observer_runs_after_the_checkpoint_lock_is_released() {
        let collector = Arc::new(Mutex::new(TransferTelemetry::default()));
        let observed = Arc::clone(&collector);
        collector.lock().unwrap().observer = Some(Arc::new(move |_, progress| {
            assert!(observed.try_lock().is_ok());
            assert_eq!(progress.total_upload_bytes, Some(10));
        }));
        observe_transfer(&collector, |telemetry| {
            telemetry.declare_upload(&ResourceRefDto {
                kind: echo_core::domain::sync::SharedResourceKind::Audio,
                relative_path: "song.flac".into(),
                content_hash: "a".repeat(64),
                length: 10,
                media_type: "audio/flac".into(),
            });
        });
        // Break the deliberately circular callback capture used by this test.
        collector.lock().unwrap().observer = None;
    }

    #[test]
    fn download_observer_reports_only_persisted_chunks_and_recovers_checkpoints() {
        use echo_core::domain::sync::{AuthorityId, Epoch, LibraryId};
        use std::sync::mpsc;

        let directory = tempfile::tempdir().unwrap();
        let store =
            Arc::new(FileResourceStore::new(directory.path().canonicalize().unwrap()).unwrap());
        let binding = SyncBinding {
            library_id: LibraryId::new(),
            authority_id: AuthorityId::new(),
            epoch: Epoch::INITIAL,
        };
        let owner = DeviceId::new();
        let bytes = b"abc";
        let hash = blake3::hash(bytes).to_hex().to_string();
        let request = PrepareResource {
            binding,
            resource: ResourceRefDto {
                kind: echo_core::domain::sync::SharedResourceKind::Audio,
                relative_path: "song.flac".into(),
                content_hash: hash.clone(),
                length: bytes.len() as u64,
                media_type: "audio/flac".into(),
            },
        };
        let (events, received) = mpsc::channel();
        let collector = Arc::new(Mutex::new(TransferTelemetry::new(Some(Arc::new(
            move |_, progress| events.send(progress).unwrap(),
        )))));
        let tracked = TrackedDownloadStore {
            store: Arc::clone(&store),
            telemetry: Arc::clone(&collector),
        };
        let prepared = tracked.prepare(owner, &request).unwrap();
        assert_eq!(received.recv().unwrap().downloaded_bytes, Some(0));
        assert!(tracked
            .write_chunk(
                binding,
                owner,
                &prepared.upload_id,
                0,
                &"0".repeat(64),
                bytes
            )
            .is_err());
        assert_eq!(collector.lock().unwrap().byte_totals().3, 0);
        assert!(received.try_recv().is_err());
        tracked
            .write_chunk(binding, owner, &prepared.upload_id, 0, &hash, bytes)
            .unwrap();
        assert_eq!(received.recv().unwrap().downloaded_bytes, Some(3));
        // A new cycle derives its initial byte count from the persisted store,
        // even though the final whole-resource completion was not yet requested.
        let resumed_collector = Arc::new(Mutex::new(TransferTelemetry::default()));
        let resumed = TrackedDownloadStore {
            store,
            telemetry: Arc::clone(&resumed_collector),
        };
        resumed.prepare(owner, &request).unwrap();
        assert_eq!(
            resumed_collector.lock().unwrap().byte_totals(),
            (0, 0, 3, 3)
        );
    }
}
