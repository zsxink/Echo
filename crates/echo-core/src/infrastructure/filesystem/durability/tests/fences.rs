//! Repeated crashes cannot turn visible redo/coverage files into durable proof.
use super::*;
use std::collections::BTreeSet;

fn traced_recovery(registry: &RootRegistry, root: LibraryRootId) -> Vec<(String, usize)> {
    let mut trace = Vec::new();
    recover_with_hook(registry, root, &mut |phase, index| {
        trace.push((phase.to_owned(), index));
        Ok(())
    })
    .unwrap();
    trace
}

fn assert_flush_before(trace: &[(String, usize)], label: &str, last_directory: usize, next: &str) {
    let later = trace.iter().position(|(phase, _)| phase == next).unwrap();
    for boundary in std::iter::once((format!("{label}:file_sync"), 0))
        .chain((0..=last_directory).map(|index| (format!("{label}:directory_sync"), index)))
    {
        let earlier = trace.iter().position(|entry| entry == &boundary).unwrap();
        assert!(
            earlier < later,
            "{boundary:?} must precede {next}: {trace:?}"
        );
    }
}

#[test]
fn library_durability_repeated_marker_sync_failure_never_materializes_or_advances() {
    for label in ["prepared_recovery", "marker_recovery"] {
        for (suffix, stop) in [
            ("file_sync", 0),
            ("directory_sync", 0),
            ("directory_sync", 3),
        ] {
            let (dir, registry, root, library, _) = fixture();
            let change = batch(library, uuid::Uuid::new_v4(), "marker fence");
            commit_with_hook(&registry, root, &change, &mut |phase, _| {
                if phase == "marker:rename" {
                    Err(fail())
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
            assert!(!dir.path().join(CHECKPOINT).exists());
            let target = format!("{label}:{suffix}");
            let mut trace = Vec::new();
            recover_with_hook(&registry, root, &mut |phase, index| {
                trace.push((phase.to_owned(), index));
                if phase == target && index == stop {
                    Err(fail())
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
            assert!(trace.contains(&(target, stop)));
            assert!(!trace.iter().any(|(phase, _)| matches!(
                phase.as_str(),
                "record" | "state" | "checkpoint" | "gc:remove"
            )));
            assert!(!dir.path().join(CHECKPOINT).exists());
            let path = portable::record_relative(change.records[0].record()).unwrap();
            assert!(!dir.path().join(path).exists());
            assert!(marker_present(dir.path(), change.operation_id));
            let trace = traced_recovery(&registry, root);
            assert_flush_before(&trace, "prepared_recovery", 3, "record");
            assert_flush_before(&trace, "marker_recovery", 3, "record");
            assert_eq!(
                read_operation(&registry, root, change.operation_id)
                    .unwrap()
                    .unwrap()
                    .batch,
                change
            );
            // A persisted materialization watermark proves earlier fence order;
            // normal subsequent recovery must not repeatedly sync old markers.
            assert!(!traced_recovery(&registry, root)
                .iter()
                .any(|(phase, _)| phase.starts_with("marker_recovery:")
                    || phase.starts_with("prepared_recovery:")));
        }
    }
}

#[test]
fn library_durability_repeated_prepared_sync_failure_never_publishes_marker() {
    let (dir, registry, root, library, _) = fixture();
    let change = batch(library, uuid::Uuid::new_v4(), "prepared fence");
    commit_with_hook(&registry, root, &change, &mut |phase, _| {
        if phase == "prepared:rename" {
            Err(fail())
        } else {
            Ok(())
        }
    })
    .unwrap_err();
    let mut trace = Vec::new();
    commit_with_hook(&registry, root, &change, &mut |phase, index| {
        trace.push((phase.to_owned(), index));
        if phase == "prepared_retry:directory_sync" && index == 0 {
            Err(fail())
        } else {
            Ok(())
        }
    })
    .unwrap_err();
    assert!(!marker_present(dir.path(), change.operation_id));
    assert!(!dir.path().join(CHECKPOINT).exists());
    trace.clear();
    commit_with_hook(&registry, root, &change, &mut |phase, index| {
        trace.push((phase.to_owned(), index));
        Ok(())
    })
    .unwrap();
    assert_flush_before(&trace, "prepared_retry", 3, "marker:rename");
    assert!(
        !trace
            .iter()
            .any(|(phase, _)| phase.starts_with("marker_recovery:")),
        "this call already completed marker publication"
    );
}

#[test]
fn library_durability_repeated_coverage_sync_failure_never_collects_redo() {
    for label in ["coverage_gc", "receipt_gc"] {
        for suffix in ["file_sync", "directory_sync"] {
            let (dir, registry, root, library, _) = fixture();
            let change = batch(library, uuid::Uuid::new_v4(), "coverage fence");
            commit(&registry, root, &change).unwrap();
            let replay = chain(&registry, root).unwrap();
            checkpoint::compact(
                &registry,
                &ValidationCache::default(),
                root,
                &replay,
                &mut |phase, _| {
                    if phase == "coverage:rename" {
                        Err(fail())
                    } else {
                        Ok(())
                    }
                },
            )
            .unwrap_err();
            let target = if suffix == "directory_sync" {
                "gc_proof:directory_sync".to_owned()
            } else {
                format!("{label}:{suffix}")
            };
            let mut trace = Vec::new();
            recover_with_hook(&registry, root, &mut |phase, index| {
                trace.push((phase.to_owned(), index));
                if phase == target {
                    Err(fail())
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
            assert!(trace.iter().any(|(phase, _)| phase == &target));
            assert!(!trace.iter().any(|(phase, _)| phase == "gc:remove"));
            assert!(marker_present(dir.path(), change.operation_id));
            assert!(dir
                .path()
                .join(relative(change.operation_id, "prepared.json"))
                .exists());
            let trace = traced_recovery(&registry, root);
            super::gc_performance::assert_gc_proof(&trace, 1, 5);
            assert_eq!(
                read_operation(&registry, root, change.operation_id)
                    .unwrap()
                    .unwrap()
                    .batch,
                change
            );
            assert_eq!(
                recover(&registry, root)
                    .unwrap()
                    .checkpoint
                    .unwrap()
                    .library_commit_seq,
                1
            );
        }
    }
}

#[test]
fn library_durability_repeated_receipt_sync_failure_blocks_coverage_and_gc() {
    for suffix in ["file_sync", "directory_sync"] {
        let (dir, registry, root, library, _) = fixture();
        let change = batch(library, uuid::Uuid::new_v4(), "receipt fence");
        commit(&registry, root, &change).unwrap();
        let replay = chain(&registry, root).unwrap();
        checkpoint::compact(
            &registry,
            &ValidationCache::default(),
            root,
            &replay,
            &mut |phase, _| {
                if phase == "receipt:rename" {
                    Err(fail())
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();
        assert!(!dir.path().join(ANCHOR).exists());
        let target = format!("receipt_retry:{suffix}");
        let mut trace = Vec::new();
        checkpoint::compact(
            &registry,
            &ValidationCache::default(),
            root,
            &replay,
            &mut |phase, index| {
                trace.push((phase.to_owned(), index));
                if phase == target {
                    Err(fail())
                } else {
                    Ok(())
                }
            },
        )
        .unwrap_err();
        assert!(trace.iter().any(|(phase, _)| phase == &target));
        assert!(!trace
            .iter()
            .any(|(phase, _)| phase == "coverage" || phase == "gc:remove"));
        assert!(!dir.path().join(ANCHOR).exists());
        assert!(marker_present(dir.path(), change.operation_id));
        trace.clear();
        checkpoint::compact(
            &registry,
            &ValidationCache::default(),
            root,
            &replay,
            &mut |phase, index| {
                trace.push((phase.to_owned(), index));
                Ok(())
            },
        )
        .unwrap();
        assert_flush_before(&trace, "receipt_retry", 4, "coverage");
        super::gc_performance::assert_gc_proof(&trace, 1, 5);
        assert_eq!(
            read_operation(&registry, root, change.operation_id)
                .unwrap()
                .unwrap()
                .batch,
            change
        );
    }
}

#[test]
fn library_durability_coverage_redo_must_sync_before_materializing() {
    let (dir, registry, root, library, _) = fixture();
    let change = batch(library, uuid::Uuid::new_v4(), "anchor redo fence");
    commit(&registry, root, &change).unwrap();
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
    std::fs::remove_file(dir.path().join(CHECKPOINT)).unwrap();
    let path = portable::record_relative(change.records[0].record()).unwrap();
    std::fs::remove_file(dir.path().join(&path)).unwrap();
    let mut trace = Vec::new();
    recover_with_hook(&registry, root, &mut |phase, _| {
        trace.push(phase.to_owned());
        if phase == "coverage_recovery:directory_sync" {
            Err(fail())
        } else {
            Ok(())
        }
    })
    .unwrap_err();
    assert!(!trace
        .iter()
        .any(|phase| matches!(phase.as_str(), "record" | "checkpoint" | "gc:remove")));
    assert!(!dir.path().join(CHECKPOINT).exists());
    assert!(!dir.path().join(path).exists());
    let trace = traced_recovery(&registry, root);
    assert_flush_before(&trace, "coverage_recovery", 2, "record");
    super::gc_performance::assert_gc_proof(&trace, 1, 5);
    assert_eq!(
        read_operation(&registry, root, change.operation_id)
            .unwrap()
            .unwrap()
            .batch,
        change
    );
}

#[test]
fn library_durability_later_receipt_sync_failure_preserves_every_covered_journal() {
    let (dir, registry, root, library, _) = fixture();
    let changes = [
        batch(library, uuid::Uuid::new_v4(), "first receipt"),
        batch(library, uuid::Uuid::new_v4(), "second receipt"),
    ];
    for change in &changes {
        commit(&registry, root, change).unwrap();
    }
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
    let mut receipts = 0;
    let mut trace = Vec::new();
    recover_with_hook(&registry, root, &mut |phase, _| {
        trace.push(phase.to_owned());
        if phase == "receipt_gc:file_sync" {
            receipts += 1;
            if receipts == 2 {
                return Err(fail());
            }
        }
        Ok(())
    })
    .unwrap_err();
    assert_eq!(receipts, 2);
    assert!(!trace.iter().any(|phase| phase == "gc:remove"));
    for change in &changes {
        assert!(marker_present(dir.path(), change.operation_id));
        assert!(dir
            .path()
            .join(relative(change.operation_id, "prepared.json"))
            .exists());
    }
    let trace = traced_recovery(&registry, root);
    let shards: BTreeSet<_> = changes
        .iter()
        .map(|change| change.operation_id.to_string()[..2].to_owned())
        .collect();
    super::gc_performance::assert_gc_proof(&trace, changes.len(), 4 + shards.len());
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

#[test]
fn library_durability_completed_coverage_without_leftovers_does_not_resync() {
    let (_dir, registry, root, library, _) = fixture();
    let change = batch(library, uuid::Uuid::new_v4(), "complete coverage");
    commit(&registry, root, &change).unwrap();
    checkpoint::compact(
        &registry,
        &ValidationCache::default(),
        root,
        &chain(&registry, root).unwrap(),
        &mut |_, _| Ok(()),
    )
    .unwrap();
    assert!(!traced_recovery(&registry, root)
        .iter()
        .any(|(phase, _)| phase.starts_with("coverage_") || phase.starts_with("receipt_")));
}
