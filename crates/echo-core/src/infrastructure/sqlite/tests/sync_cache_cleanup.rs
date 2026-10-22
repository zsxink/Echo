//! Durable retry markers for retiring a detached binding's transfer cache.

use super::*;
use crate::domain::sync::{AuthorityId, Epoch, LibraryId, SyncBinding};

fn binding(epoch: u64) -> SyncBinding {
    SyncBinding {
        library_id: LibraryId::new(),
        authority_id: AuthorityId::new(),
        epoch: Epoch::from_u64(epoch),
    }
}

#[test]
fn cache_cleanup_markers_are_durable_and_idempotent() {
    let (_directory, database, _root) = database();
    assert!(database.pending_sync_cache_cleanups().unwrap().is_empty());

    let first = binding(1);
    let second = binding(2);
    database.record_sync_cache_cleanup(second).unwrap();
    database.record_sync_cache_cleanup(first).unwrap();
    // Re-recording must not duplicate or reorder an existing intent.
    database.record_sync_cache_cleanup(first).unwrap();

    let pending = database.pending_sync_cache_cleanups().unwrap();
    assert_eq!(pending.len(), 2);
    assert!(pending.contains(&first));
    assert!(pending.contains(&second));

    database.complete_sync_cache_cleanup(first).unwrap();
    assert_eq!(
        database.pending_sync_cache_cleanups().unwrap(),
        vec![second]
    );
}

#[test]
fn failed_cache_cleanup_attempts_keep_the_marker_and_are_counted() {
    let (_directory, database, _root) = database();
    let binding = binding(3);
    database.record_sync_cache_cleanup(binding).unwrap();

    for expected in 1..=3 {
        assert_eq!(
            database.retry_sync_cache_cleanup(binding).unwrap(),
            expected
        );
        assert_eq!(
            database.pending_sync_cache_cleanups().unwrap(),
            vec![binding],
            "a failed cleanup must stay retryable"
        );
    }

    database.complete_sync_cache_cleanup(binding).unwrap();
    assert!(database.pending_sync_cache_cleanups().unwrap().is_empty());
    assert_eq!(database.retry_sync_cache_cleanup(binding).unwrap(), 0);
}

#[test]
fn cache_cleanup_markers_survive_reopening_the_database() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("echo.sqlite");
    let binding = binding(4);
    {
        let database = SqliteDatabase::open(&path).unwrap();
        database.record_sync_cache_cleanup(binding).unwrap();
    }

    let reopened = SqliteDatabase::open(&path).unwrap();
    assert_eq!(
        reopened.pending_sync_cache_cleanups().unwrap(),
        vec![binding]
    );
}
