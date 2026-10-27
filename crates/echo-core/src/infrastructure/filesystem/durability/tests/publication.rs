//! Checkpoint-backed materialization and native publication measurements.
use super::*;

fn stable_change(library: LibraryId) -> LibraryBatch {
    let mut change = batch(library, uuid::Uuid::new_v4(), "stable record");
    change.state.push(DurableStateChange::new(
        StateKind::Pending,
        uuid::Uuid::new_v4(),
        serde_json::json!({"original": "immutable pending body"}),
    ));
    change
}

#[test]
fn library_durability_checkpointed_equal_after_images_skip_republication() {
    let (_dir, registry, root, library, _) = fixture();
    let first = stable_change(library);
    commit(&registry, root, &first).unwrap();
    let mut next = first;
    next.operation_id = uuid::Uuid::new_v4();
    let mut trace = Vec::new();
    commit_with_hook(&registry, root, &next, &mut |phase, _| {
        trace.push(phase.to_owned());
        Ok(())
    })
    .unwrap();
    assert!(trace.iter().any(|phase| phase == "record"));
    assert!(trace.iter().any(|phase| phase == "state"));
    assert!(
        !trace
            .iter()
            .any(|phase| phase.starts_with("record:") || phase.starts_with("state:"))
    );
    assert!(
        trace
            .iter()
            .any(|phase| phase == "checkpoint:directory_sync")
    );
    assert_eq!(recover(&registry, root).unwrap().head().unwrap().0, 2);
}

#[test]
fn library_durability_checkpointed_equal_after_images_reject_missing_or_tampered_files() {
    for record in [true, false] {
        for missing in [true, false] {
            let (dir, registry, root, library, _) = fixture();
            let first = stable_change(library);
            commit(&registry, root, &first).unwrap();
            let mut next = first;
            next.operation_id = uuid::Uuid::new_v4();
            commit_with_hook(&registry, root, &next, &mut |phase, _| {
                if phase == "committed" {
                    Err(fail())
                } else {
                    Ok(())
                }
            })
            .unwrap_err();
            let relative = if record {
                portable::record_relative(next.records[0].record()).unwrap()
            } else {
                checkpoint::state_path(&next.state[0])
            };
            let path = dir.path().join(relative);
            if missing {
                std::fs::remove_file(&path).unwrap();
            } else {
                std::fs::write(&path, b"tampered checkpointed bytes").unwrap();
            }
            assert!(
                recover(&registry, root).is_err(),
                "record={record}, missing={missing}"
            );
            if missing {
                assert!(!path.exists());
            } else {
                assert_eq!(std::fs::read(path).unwrap(), b"tampered checkpointed bytes");
            }
        }
    }
}

#[test]
fn library_durability_rename_without_checkpoint_repeats_file_and_directory_sync() {
    for label in ["record", "state"] {
        let (dir, registry, root, library, _) = fixture();
        let change = stable_change(library);
        let phase = format!("{label}:rename");
        commit_with_hook(&registry, root, &change, &mut |current, _| {
            if current == phase {
                Err(fail())
            } else {
                Ok(())
            }
        })
        .unwrap_err();
        assert!(!dir.path().join(CHECKPOINT).exists());
        let relative = if label == "record" {
            portable::record_relative(change.records[0].record()).unwrap()
        } else {
            checkpoint::state_path(&change.state[0])
        };
        assert!(
            dir.path().join(relative).exists(),
            "rename exposed equal bytes"
        );
        let mut trace = Vec::new();
        recover_with_hook(&registry, root, &mut |phase, index| {
            trace.push((phase.to_owned(), index));
            Ok(())
        })
        .unwrap();
        assert!(trace.contains(&(format!("{label}:file_sync"), 0)));
        assert!(trace.contains(&(format!("{label}:rename"), 0)));
        // Materialization now synchronizes the union of record and state
        // parent directories after all renames, before the checkpoint.
        let directory_sync = trace
            .iter()
            .rposition(|(phase, _)| phase == "materialization:directory_sync")
            .expect("materialization directories are synchronized");
        let checkpoint = trace
            .iter()
            .position(|(phase, _)| phase == "checkpoint:file_sync")
            .expect("checkpoint is published");
        assert!(directory_sync < checkpoint);
        assert!(dir.path().join(CHECKPOINT).exists());
    }
}

#[test]
fn library_durability_commit_checks_future_format_before_recovery() {
    let (dir, registry, root, library, plane) = fixture();
    let change = stable_change(library);
    commit_with_hook(&registry, root, &change, &mut |phase, _| {
        if phase == "committed" {
            Err(fail())
        } else {
            Ok(())
        }
    })
    .unwrap_err();
    let mut future = LibraryManifest::new(library, "future format");
    future.format_version = u64::MAX;
    std::fs::write(
        dir.path().join("echo/manifest.json"),
        serde_json::to_vec(&future).unwrap(),
    )
    .unwrap();
    let error = LibraryDurabilityPort::commit(&plane, root, &change).unwrap_err();
    assert_eq!(error.code(), "unsupported_media");
    assert!(!dir.path().join(CHECKPOINT).exists());
    let path = portable::record_relative(change.records[0].record()).unwrap();
    assert!(!dir.path().join(path).exists());
}
