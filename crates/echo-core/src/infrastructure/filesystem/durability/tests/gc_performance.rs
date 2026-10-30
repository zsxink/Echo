//! Real filesystem GC barriers: every file is flushed, shared directories once.
use super::*;

fn pending_gc() -> (
    tempfile::TempDir,
    RootRegistry,
    LibraryRootId,
    Vec<LibraryBatch>,
) {
    let (directory, registry, root, library, _) = fixture();
    let changes: Vec<_> = [0xaa_u128, 0xaa, 0xbb]
        .into_iter()
        .enumerate()
        .map(|(index, prefix)| {
            let mut change = batch(library, uuid::Uuid::new_v4(), "GC boundaries");
            change.operation_id = uuid::Uuid::from_u128((prefix << 120) | (index as u128 + 1));
            commit(&registry, root, &change).unwrap();
            change
        })
        .collect();
    checkpoint::compact(
        &registry,
        &ValidationCache::default(),
        root,
        &chain(&registry, root).unwrap(),
        &mut |phase, _| {
            if phase == "coverage:rename" {
                Err(fail())
            } else {
                Ok(())
            }
        },
    )
    .unwrap_err();
    (directory, registry, root, changes)
}

fn cleanup(
    registry: &RootRegistry,
    cache: &ValidationCache,
    root: LibraryRootId,
    hook: &mut Hook<'_>,
) -> Result<(), Error> {
    checkpoint::cleanup_covered(registry, cache, root, 3, hook)
}

fn assert_journals(directory: &std::path::Path, changes: &[LibraryBatch]) {
    for change in changes {
        assert!(marker_present(directory, change.operation_id));
        assert!(directory
            .join(relative(change.operation_id, "prepared.json"))
            .is_file());
    }
}

pub(super) fn assert_gc_proof(trace: &[(String, usize)], receipts: usize, directories: usize) {
    let first_remove = trace
        .iter()
        .position(|(phase, _)| phase == "gc:remove")
        .unwrap();
    let proof = &trace[..first_remove];
    assert_eq!(
        proof
            .iter()
            .filter(|(p, _)| p == "coverage_gc:file_sync")
            .count(),
        1
    );
    assert_eq!(
        proof
            .iter()
            .filter(|(p, _)| p == "receipt_gc:file_sync")
            .count(),
        receipts
    );
    let actual: Vec<_> = proof
        .iter()
        .filter(|(p, _)| p.ends_with(":directory_sync"))
        .filter(|(p, _)| {
            p == "gc_proof:directory_sync"
                || p == "coverage_gc:directory_sync"
                || p == "receipt_gc:directory_sync"
        })
        .cloned()
        .collect();
    assert_eq!(
        actual,
        (0..directories)
            .map(|index| ("gc_proof:directory_sync".to_owned(), index))
            .collect::<Vec<_>>()
    );
    let first_directory = proof
        .iter()
        .position(|(p, _)| p == "gc_proof:directory_sync")
        .unwrap();
    assert!(
        !proof[first_directory..]
            .iter()
            .any(|(p, _)| p.ends_with(":file_sync")),
        "all file proofs precede directory proofs"
    );
}

#[test]
fn library_durability_gc_deduplicates_proof_and_post_delete_ancestor_syncs() {
    let (_directory, registry, root, _) = pending_gc();
    let mut trace = Vec::new();
    cleanup(&registry, &ValidationCache::default(), root, &mut |p, i| {
        trace.push((p.to_owned(), i));
        Ok(())
    })
    .unwrap();
    eprintln!(
        "observed native GC barriers: files={}, proof_directories={}, deletion_directories={}",
        trace
            .iter()
            .filter(|(p, _)| p.ends_with(":file_sync"))
            .count(),
        trace
            .iter()
            .filter(|(p, _)| p.ends_with(":directory_sync") && p != "gc:directory_sync")
            .count(),
        trace
            .iter()
            .filter(|(p, _)| p == "gc:directory_sync")
            .count(),
    );
    // aa and bb shards, operations, state, echo, root; file flushes stay at four.
    assert_gc_proof(&trace, 3, 6);
    let last_remove = trace.iter().rposition(|(p, _)| p == "gc:remove").unwrap();
    let deletion_syncs: Vec<_> = trace
        .iter()
        .filter(|(p, _)| p == "gc:directory_sync")
        .cloned()
        .collect();
    assert_eq!(
        deletion_syncs,
        (0..3)
            .map(|i| ("gc:directory_sync".to_owned(), i))
            .collect::<Vec<_>>()
    );
    assert!(trace[..last_remove]
        .iter()
        .all(|(p, _)| p != "gc:directory_sync"));
}

#[test]
fn library_durability_gc_every_file_and_unique_directory_failure_preserves_all_redo_and_reproves() {
    let boundaries: Vec<_> = std::iter::once(("coverage_gc:file_sync", 0))
        .chain((0..3).map(|i| ("receipt_gc:file_sync", i)))
        .chain((0..6).map(|i| ("gc_proof:directory_sync", i)))
        .collect();
    for (target, stop) in boundaries {
        let (directory, registry, root, changes) = pending_gc();
        let cache = ValidationCache::default();
        let mut occurrence = 0;
        let mut reached = false;
        cleanup(&registry, &cache, root, &mut |p, index| {
            assert_ne!(p, "gc:remove", "proof failure must precede every delete");
            if p == target {
                let ordinal = if p.ends_with(":file_sync") {
                    let old = occurrence;
                    occurrence += 1;
                    old
                } else {
                    index
                };
                if ordinal == stop {
                    reached = true;
                    return Err(fail());
                }
            }
            Ok(())
        })
        .unwrap_err();
        assert!(reached, "missing failure hook {target}/{stop}");
        assert_journals(directory.path(), &changes);
        let mut retry = Vec::new();
        cleanup(&registry, &cache, root, &mut |p, i| {
            retry.push((p.to_owned(), i));
            Ok(())
        })
        .unwrap();
        assert_gc_proof(&retry, 3, 6);
    }
}

#[test]
fn library_durability_gc_delete_failure_retry_reproves_remaining_journals() {
    let (_directory, registry, root, _) = pending_gc();
    cleanup(&registry, &ValidationCache::default(), root, &mut |p, _| {
        if p == "gc:remove" {
            Err(fail())
        } else {
            Ok(())
        }
    })
    .unwrap_err();
    let mut retry = Vec::new();
    cleanup(&registry, &ValidationCache::default(), root, &mut |p, i| {
        retry.push((p.to_owned(), i));
        Ok(())
    })
    .unwrap();
    assert_gc_proof(&retry, 3, 6);
}

#[test]
fn library_durability_gc_post_delete_directory_failures_report_error_and_recover() {
    for stop in 0..3 {
        let (_directory, registry, root, changes) = pending_gc();
        let mut reached = false;
        cleanup(&registry, &ValidationCache::default(), root, &mut |p, i| {
            if p == "gc:directory_sync" && i == stop {
                reached = true;
                Err(fail())
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert!(reached);
        // The durable coverage/receipts retain every original operation even if
        // a crash leaves some empty journal directories visible again.
        let replay = recover(&registry, root).unwrap();
        assert_eq!(replay.head().unwrap().0, 3);
        for change in changes {
            assert_eq!(
                read_operation(&registry, root, change.operation_id)
                    .unwrap()
                    .unwrap()
                    .batch,
                change
            );
        }
    }
}
