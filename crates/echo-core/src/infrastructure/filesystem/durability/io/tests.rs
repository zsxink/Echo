use super::*;

#[test]
fn exclusive_publication_preserves_existing_and_racing_targets() {
    for preexisting in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let root = LibraryRootId::new();
        let registry = RootRegistry::new();
        registry.register(root, directory.path());
        let relative = "echo/journal/prepared.json";
        let target = directory.path().join(relative);
        if preexisting {
            std::fs::create_dir_all(target.parent().unwrap()).unwrap();
            std::fs::write(&target, b"original evidence").unwrap();
        }
        let mut observed = false;
        let result = publish(
            &registry,
            root,
            relative,
            b"replacement payload",
            true,
            "prepared",
            &mut |phase, _| {
                if phase == "prepared:file_sync" {
                    observed = true;
                    if !preexisting {
                        assert!(
                            !target.exists(),
                            "temporary must not expose a partial target"
                        );
                        std::fs::write(&target, b"racing evidence").unwrap();
                    }
                }
                Ok(())
            },
        );
        assert!(
            observed,
            "the conflict must occur after the temporary is durable"
        );
        assert!(
            result.is_err(),
            "exclusive publication must never replace a target"
        );
        let expected: &[u8] = if preexisting {
            b"original evidence"
        } else {
            b"racing evidence"
        };
        assert_eq!(std::fs::read(&target).unwrap(), expected);
        assert_eq!(
            std::fs::read_dir(target.parent().unwrap()).unwrap().count(),
            1
        );
    }
}

#[test]
fn exclusive_publication_exposes_complete_payload_after_file_sync() {
    let directory = tempfile::tempdir().unwrap();
    let root = LibraryRootId::new();
    let registry = RootRegistry::new();
    registry.register(root, directory.path());
    let relative = "echo/journal/prepared.json";
    let target = directory.path().join(relative);
    let bytes = b"complete durable prepared payload";
    let mut published = false;
    publish(
        &registry,
        root,
        relative,
        bytes,
        true,
        "prepared",
        &mut |phase, _| {
            if phase == "prepared:file_sync" {
                assert!(!target.exists());
            }
            if phase == "prepared:rename" {
                published = true;
                assert_eq!(std::fs::read(&target).unwrap(), bytes);
            }
            Ok(())
        },
    )
    .unwrap();
    assert!(published);
    assert_eq!(
        std::fs::read_dir(target.parent().unwrap()).unwrap().count(),
        1
    );
}

#[test]
fn phase_directory_set_keeps_unique_ancestors_and_syncs_children_first() {
    let directory = tempfile::tempdir().unwrap();
    let registry = RootRegistry::new();
    let root = LibraryRootId::new();
    registry.register(root, directory.path());
    let mut proof = DirectorySyncSet::default();
    for suffix in [
        "echo/state/operations/aa",
        "echo/state",
        "echo/state/operations/bb",
        "echo/state/operations/aa",
    ] {
        let path = directory.path().join(suffix);
        std::fs::create_dir_all(&path).unwrap();
        proof.include_ancestors(&registry, root, &path).unwrap();
    }
    let expected: BTreeSet<_> = [
        "",
        "echo",
        "echo/state",
        "echo/state/operations",
        "echo/state/operations/aa",
        "echo/state/operations/bb",
    ]
    .into_iter()
    .map(|suffix| directory.path().join(suffix))
    .collect();
    assert_eq!(proof.0, expected);
    let mut order: Vec<_> = proof.0.iter().cloned().collect();
    order.sort_by(|a, b| {
        b.components()
            .count()
            .cmp(&a.components().count())
            .then_with(|| a.cmp(b))
    });
    let mut trace = Vec::new();
    proof
        .sync(&registry, root, "proof", &mut |p, i| {
            trace.push((p.to_owned(), order[i].clone()));
            // Removing the successfully synced directory makes an ancestor-first or
            // duplicate flush fail at its later native open, not merely in counters.
            std::fs::remove_dir(&order[i]).unwrap();
            Ok(())
        })
        .unwrap();
    assert_eq!(trace.len(), 6);
    assert_eq!(trace.last().unwrap().1, directory.path());
}

#[cfg(unix)]
#[test]
fn phase_directory_set_rechecks_path_after_an_earlier_sync_hook() {
    let directory = tempfile::tempdir().unwrap();
    let outside = tempfile::tempdir().unwrap();
    let root = LibraryRootId::new();
    let registry = RootRegistry::new();
    registry.register(root, directory.path());
    let mut proof = DirectorySyncSet::default();
    for suffix in ["echo/a", "echo/b"] {
        let path = directory.path().join(suffix);
        std::fs::create_dir_all(&path).unwrap();
        proof.include_ancestors(&registry, root, &path).unwrap();
    }
    let mut seen = Vec::new();
    let result = proof.sync(&registry, root, "proof", &mut |_, index| {
        seen.push(index);
        if index == 0 {
            let victim = directory.path().join("echo/b");
            std::fs::remove_dir(&victim).unwrap();
            std::os::unix::fs::symlink(outside.path(), &victim).unwrap();
        }
        Ok(())
    });
    assert!(
        result.is_err(),
        "the next native sync must reject a replaced ancestor"
    );
    assert_eq!(seen, [0]);
}
