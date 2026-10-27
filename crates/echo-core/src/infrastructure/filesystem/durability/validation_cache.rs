//! Bounded certificates for intrinsic validation of exact persisted document bytes.
//! Reads, parsing, ownership and relationships are always checked by the caller.
use crate::domain::ids::LibraryRootId;
use crate::error::Error;
use std::collections::VecDeque;
use std::sync::Mutex;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};

const CAPACITY: usize = 128;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DocumentKind {
    Commit,
    Coverage,
}

#[derive(Debug, Eq, PartialEq)]
struct Certificate {
    kind: DocumentKind,
    root: LibraryRootId,
    digest: blake3::Hash,
}

/// Stores no payloads, errors, replay results or filesystem metadata. FIFO
/// eviction bounds memory independently of document size and history length.
#[derive(Debug, Default)]
pub(in crate::infrastructure::filesystem) struct ValidationCache {
    certificates: Mutex<VecDeque<Certificate>>,
    #[cfg(test)]
    full: AtomicUsize,
    #[cfg(test)]
    hits: AtomicUsize,
}

impl ValidationCache {
    pub(super) fn validate(
        &self,
        kind: DocumentKind,
        root: LibraryRootId,
        raw: &str,
        validate: impl FnOnce() -> Result<(), Error>,
    ) -> Result<(), Error> {
        let certificate = Certificate {
            kind,
            root,
            digest: blake3::hash(raw.as_bytes()),
        };
        if self
            .certificates
            .try_lock()
            .is_ok_and(|entries| entries.contains(&certificate))
        {
            #[cfg(test)]
            self.hits.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }
        // Never hold a cache lock while validating. Poisoning or contention
        // merely disables the optimization; it cannot authorize a document.
        #[cfg(test)]
        self.full.fetch_add(1, Ordering::Relaxed);
        validate()?;
        if let Ok(mut entries) = self.certificates.try_lock() {
            if !entries.contains(&certificate) {
                if entries.len() == CAPACITY {
                    entries.pop_front();
                }
                entries.push_back(certificate);
            }
        }
        Ok(())
    }

    #[cfg(test)]
    pub(super) fn counts(&self) -> (usize, usize) {
        (
            self.full.load(Ordering::Relaxed),
            self.hits.load(Ordering::Relaxed),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fail() -> Error {
        Error::conflict("cache test rejected")
    }
    #[test]
    fn library_durability_cache_evicts_and_unavailable_lock_runs_full_validation() {
        let cache = ValidationCache::default();
        let root = LibraryRootId::new();
        cache
            .validate(DocumentKind::Commit, root, "first", || Ok(()))
            .unwrap();
        for i in 0..128 {
            cache
                .validate(DocumentKind::Commit, root, &i.to_string(), || Ok(()))
                .unwrap();
        }
        // A cold/evicted body is fully validated, even if its old key is repeated.
        assert!(
            cache
                .validate(DocumentKind::Commit, root, "first", || Err(fail()))
                .is_err()
        );
        cache
            .validate(DocumentKind::Commit, root, "warm", || Ok(()))
            .unwrap();
        let guard = cache.certificates.lock().unwrap();
        assert!(
            cache
                .validate(DocumentKind::Commit, root, "warm", || Err(fail()))
                .is_err()
        );
        drop(guard);
        assert!(
            cache
                .validate(DocumentKind::Coverage, root, "warm", || Err(fail()))
                .is_err()
        );
        assert_eq!(cache.certificates.lock().unwrap().len(), 128);
    }
}
