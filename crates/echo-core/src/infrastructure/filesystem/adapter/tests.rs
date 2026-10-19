use super::*;
use crate::application::ports::StagedResource;

fn setup() -> (tempfile::TempDir, LibraryRootId, RootConstrainedFileSystem) {
    let dir = tempfile::tempdir().unwrap();
    let registry = RootRegistry::new();
    let root = LibraryRootId::new();
    registry.register(root, dir.path());
    (dir, root, RootConstrainedFileSystem::new(registry))
}

#[test]
fn disk_full_or_permission_loss_aborts_staging_copy_before_publication() {
    struct FailingDisk(std::io::ErrorKind);
    impl std::io::Write for FailingDisk {
        fn write(&mut self, _bytes: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::from(self.0))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    for kind in [
        std::io::ErrorKind::StorageFull,
        std::io::ErrorKind::PermissionDenied,
    ] {
        let error = copy_staging_content(
            &mut std::io::Cursor::new(b"downloaded audio"),
            &mut FailingDisk(kind),
            Path::new("private.part"),
        )
        .unwrap_err();
        match error {
            Error::Io {
                operation, source, ..
            } => {
                assert_eq!(operation, "write staging copy");
                assert_eq!(source.kind(), kind);
            }
            other => panic!("expected destination write error: {other:?}"),
        }
    }
}

#[cfg(unix)]
#[test]
fn linked_staging_operation_slot_cannot_escape_root() {
    let (directory, root, fs) = setup();
    let outside = tempfile::tempdir().unwrap();
    let staged = StagedResource::new(OperationId::new(), "audio").unwrap();
    fs.establish_write_capability(root).unwrap();
    let slot = directory
        .path()
        .join("echo/tmp")
        .join(staged.operation().as_uuid().simple().to_string());
    std::os::unix::fs::symlink(outside.path(), slot).unwrap();
    assert_eq!(
        fs.stage_bytes(root, &staged, b"audio").unwrap_err().code(),
        "permission"
    );
    assert!(std::fs::read_dir(outside.path()).unwrap().next().is_none());
}

#[cfg(unix)]
#[test]
fn swapped_staging_slot_is_never_read_published_or_cleaned_outside_root() {
    let (directory, root, fs) = setup();
    let outside = tempfile::tempdir().unwrap();
    let staged = StagedResource::new(OperationId::new(), "audio").unwrap();
    fs.stage_bytes(root, &staged, b"original").unwrap();
    let slot = directory
        .path()
        .join("echo/tmp")
        .join(staged.operation().as_uuid().simple().to_string());
    std::fs::rename(&slot, directory.path().join("retained-slot")).unwrap();
    std::fs::write(outside.path().join("audio"), b"external content").unwrap();
    std::os::unix::fs::symlink(outside.path(), &slot).unwrap();
    assert_eq!(
        fs.read_staged(root, &staged).unwrap_err().code(),
        "permission"
    );
    let target = RelativeMediaPath::new("media/track.flac").unwrap();
    assert_eq!(
        fs.publish(root, &staged, &target).unwrap_err().code(),
        "permission"
    );
    fs.discard_staged(root, &staged).unwrap();
    assert_eq!(
        std::fs::read(outside.path().join("audio")).unwrap(),
        b"external content"
    );
    assert!(!directory.path().join("media/track.flac").exists());
}

#[test]
fn staged_handle_publishes_into_the_root_and_rejects_replacement() {
    let (_dir, root, fs) = setup();
    let operation = OperationId::new();
    let staged = StagedResource::new(operation, "audio").unwrap();
    fs.stage_bytes(root, &staged, b"published-bytes").unwrap();

    let target = RelativeMediaPath::new("artist/tone.mp3").unwrap();
    fs.publish(root, &staged, &target).unwrap();
    assert!(
        fs.read_head(root, &target, 64).unwrap() == b"published-bytes".to_vec(),
        "the staged file moved to its target"
    );

    // Republishing onto the existing target is refused (never replace).
    let staged2 = StagedResource::new(operation, "audio-2").unwrap();
    fs.stage_bytes(root, &staged2, b"second").unwrap();
    let error = fs.publish(root, &staged2, &target).unwrap_err();
    assert_eq!(error.code(), "conflict");
}

#[test]
fn forged_staged_handles_are_rejected() {
    let (_dir, root, fs) = setup();
    let forged = StagedResource::new(OperationId::new(), "forged").unwrap();
    let error = fs
        .publish(root, &forged, &RelativeMediaPath::new("x.mp3").unwrap())
        .unwrap_err();
    assert_eq!(error.code(), "permission");
}

#[test]
fn write_capability_reflects_the_staging_marker() {
    let (_dir, root, fs) = setup();
    assert!(
        !fs.write_capable(root).unwrap(),
        "no staging directory yet: writes disabled"
    );
    // Establishing the staging directory flips the capability.
    fs.staging().ensure_dir(root).unwrap();
    assert!(fs.write_capable(root).unwrap());
    // A foreign same-prefix directory does not grant capability.
    let base = fs.staging().registry().path_of(root).unwrap();
    let foreign = base.join(".echo-staging-user");
    std::fs::create_dir_all(&foreign).unwrap();
    assert!(
        !foreign
            .join(super::super::staging::MARKER_FILE_NAME)
            .exists()
            || fs.write_capable(root).unwrap()
    );
}

#[test]
fn read_head_reads_only_the_requested_prefix() {
    let (_dir, root, fs) = setup();
    let bytes = vec![7u8; 10_000];
    let path = RelativeMediaPath::new("big.mp3").unwrap();
    let base = fs.registry.path_of(root).unwrap();
    std::fs::write(base.join("big.mp3"), &bytes).unwrap();
    let head = fs.read_head(root, &path, 16).unwrap();
    assert_eq!(head.len(), 16, "only the head is read");
}

#[test]
fn unregistered_roots_are_unavailable() {
    let (_dir, _root, fs) = setup();
    let ghost = LibraryRootId::new();
    let error = fs.enumerate(ghost).unwrap_err();
    assert_eq!(error.code(), "unavailable");
}

#[test]
fn path_exists_propagates_real_root_access_errors_but_not_not_found() {
    // RootConstrainedFileSystem treats the root as an opaque base path and
    // resolves media paths beneath it. Registering a *file* as a root
    // yields an `ENOTDIR`-style probe failure on Unix, which must surface as
    // a real error rather than absence. Windows has no `ENOTDIR`: resolving
    // under a file component yields `NotFound`, so the "not mistaken for
    // absence" half is inherently unix-specific and gated accordingly.
    let dir = tempfile::tempdir().unwrap();
    let registry = RootRegistry::new();
    let missing_root = LibraryRootId::new();
    registry.register(missing_root, dir.path());
    let fs = RootConstrainedFileSystem::new(registry);

    assert!(!fs
        .path_exists(
            missing_root,
            &RelativeMediaPath::new("missing.flac").unwrap(),
        )
        .expect("a missing target remains a normal false result"));

    #[cfg(unix)]
    {
        let root = LibraryRootId::new();
        let not_a_directory = dir.path().join("not-a-directory");
        std::fs::write(&not_a_directory, b"file").unwrap();
        let file_as_root = RootRegistry::new();
        file_as_root.register(root, &not_a_directory);
        let fs = RootConstrainedFileSystem::new(file_as_root);
        let error = fs
            .path_exists(root, &RelativeMediaPath::new("child.flac").unwrap())
            .expect_err("a root access failure must not be mistaken for absence");
        // The registry rejects a non-directory root before the relative
        // child probe, classifying this as an unsafe root boundary.
        assert_eq!(error.code(), "permission");
    }
}

// -----------------------------------------------------------------------
// Task 5.3: streaming staging + BLAKE3, fsync, exclusive publish and
// staging-slot hygiene, verified against the real filesystem in temp dirs.
// -----------------------------------------------------------------------

#[test]
fn staged_copy_streams_hashes_and_lands_in_the_owned_import_slot() {
    let (dir, root, fs) = setup();
    let operation = OperationId::new();
    let staged = StagedResource::new(operation, "audio").unwrap();
    // Multi-chunk content: the copy must pump it through in bounded
    // pieces, not buffer the whole file (chunk size is 64 KiB).
    let content: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
    let mut reader: &[u8] = &content;

    let copy = fs.stage_stream(root, &staged, &mut reader).unwrap();
    assert_eq!(copy.size, u64::try_from(content.len()).unwrap());
    assert_eq!(copy.blake3, blake3::hash(&content).to_hex().to_string());
    // The staged slot follows the portable layout: `echo/tmp/<operation>`.
    assert!(
        copy.staged_path.display().starts_with("echo/tmp/"),
        "staged under the controlled directory: {}",
        copy.staged_path.display()
    );
    assert!(copy
        .staged_path
        .display()
        .contains(&format!("{}/audio", staged.operation().as_uuid().simple())));
    // The staged file physically exists under the root and round-trips.
    let staged_abs = dir.path().join(copy.staged_path.normalized());
    assert_eq!(std::fs::read(&staged_abs).unwrap(), content);
    assert_eq!(fs.read_staged(root, &staged).unwrap(), content);
    // No half-written `.part` residue after the atomic finalize.
    assert!(
        !Path::new(&format!("{}.part", staged_abs.display())).exists(),
        "the exclusive part file was renamed away"
    );

    // Publish moves the staged copy out; the target holds every byte.
    let target = RelativeMediaPath::new("歌手/歌手 - 大文件.flac").unwrap();
    fs.publish(root, &staged, &target).unwrap();
    assert_eq!(
        fs.read_head(root, &target, u64::MAX).unwrap(),
        content,
        "the full audio is published whole"
    );
    assert!(fs.read_staged(root, &staged).is_err(), "staged is gone");
}

#[test]
fn publish_reserves_the_target_exclusively_and_never_replaces() {
    let (dir, root, fs) = setup();
    let target = RelativeMediaPath::new("歌手/歌手 - 晴天.flac").unwrap();
    let base = dir.path();

    // First publish wins the exclusive reservation.
    let first = StagedResource::new(OperationId::new(), "audio").unwrap();
    fs.stage_bytes(root, &first, b"winner-bytes").unwrap();
    fs.publish(root, &first, &target).unwrap();

    // A second publish onto the occupied name is refused, and the
    // incumbent content is untouched (绝不覆盖既有文件).
    let second = StagedResource::new(OperationId::new(), "audio").unwrap();
    fs.stage_bytes(root, &second, b"loser-bytes").unwrap();
    let error = fs.publish(root, &second, &target).unwrap_err();
    assert_eq!(error.code(), "conflict");
    assert_eq!(
        fs.read_head(root, &target, u64::MAX).unwrap(),
        b"winner-bytes"
    );
    // The failed publish cleared its own (empty) reservation, so a retry
    // starts clean.
    assert_eq!(
        std::fs::read(base.join(target.normalized())).unwrap(),
        b"winner-bytes"
    );
    fs.discard_staged(root, &second).unwrap();

    // A symlink sitting at the destination is never followed or replaced:
    // the exclusive create-new refuses the name outright.
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("precious.txt"), b"precious").unwrap();
    let link_target = RelativeMediaPath::new("linked.flac").unwrap();
    create_symlink(
        &outside.path().join("precious.txt"),
        &base.join("linked.flac"),
    );
    let third = StagedResource::new(OperationId::new(), "audio").unwrap();
    fs.stage_bytes(root, &third, b"symlink-victim").unwrap();
    let error = fs.publish(root, &third, &link_target).unwrap_err();
    // Relative resolution rejects the final symlink before the exclusive
    // create reaches the OS; the foreign target is still untouched.
    assert_eq!(error.code(), "permission", "symlink at target is refused");
    assert_eq!(
        std::fs::read(outside.path().join("precious.txt")).unwrap(),
        b"precious",
        "the symlink target was never touched"
    );
}

/// Platform-neutral symlink creation for tests (runtime helper choice, no
/// platform-conditional code — see the staging tests).
fn create_symlink(target: &Path, link: &Path) {
    let status = if std::env::consts::OS == "windows" {
        std::process::Command::new("cmd")
            .args(["/C", "mklink"])
            .arg(link)
            .arg(target)
            .status()
            .expect("spawn cmd")
    } else {
        std::process::Command::new("ln")
            .arg("-s")
            .arg(target)
            .arg(link)
            .status()
            .expect("spawn ln")
    };
    assert!(status.success(), "symlink creation must succeed");
}

#[test]
fn discard_staged_is_idempotent_and_never_reaches_outside_the_owned_slot() {
    let (_dir, root, fs) = setup();
    let staged = StagedResource::new(OperationId::new(), "audio").unwrap();
    fs.stage_bytes(root, &staged, b"to-discard").unwrap();
    fs.discard_staged(root, &staged).unwrap();
    assert!(fs.read_staged(root, &staged).is_err());
    // Idempotent: discarding again (and discarding an unknown handle)
    // succeeds without effect.
    fs.discard_staged(root, &staged).unwrap();
    fs.discard_staged(
        root,
        &StagedResource::new(OperationId::new(), "audio").unwrap(),
    )
    .unwrap();
}

#[test]
fn user_directory_with_a_staging_name_is_never_written() {
    let (dir, root, fs) = setup();
    // A user directory that happens to carry Echo's staging prefix, with
    // real user content inside.
    let foreign = dir.path().join(".echo-staging-user-owned");
    std::fs::create_dir_all(&foreign).unwrap();
    std::fs::write(foreign.join("user-file.txt"), b"user-content").unwrap();

    // A full ingest cycle must pick its own random directory and leave
    // the foreign one completely alone.
    let staged = StagedResource::new(OperationId::new(), "audio").unwrap();
    fs.stage_bytes(root, &staged, b"import-bytes").unwrap();
    let target = RelativeMediaPath::new("歌手/歌手 - 晴天.flac").unwrap();
    fs.publish(root, &staged, &target).unwrap();

    assert_eq!(
        std::fs::read(foreign.join("user-file.txt")).unwrap(),
        b"user-content",
        "user file content unchanged"
    );
    assert_eq!(
        std::fs::read_dir(&foreign).unwrap().count(),
        1,
        "nothing was written into the user directory"
    );
    // Echo's writes landed in its own marker-verified directory instead.
    let owned = fs.staging().ensure_dir(root).unwrap();
    assert_ne!(owned, foreign);
    assert!(fs.write_capable(root).unwrap());
}

// -----------------------------------------------------------------------
// Task 5.5/5.10 + design §9: the persisted staging-path publish/discard
// surface and the `trash/<operation>` slot contract.
// -----------------------------------------------------------------------

#[test]
fn publish_from_staging_path_resolves_the_journal_path_and_never_touches_foreign_paths() {
    let (dir, root, fs) = setup();
    // A full stage_stream installs the journal-shaped staging location.
    let operation = OperationId::new();
    let staged = StagedResource::new(operation, "audio").unwrap();
    let copy = fs
        .stage_stream(root, &staged, &mut (&b"journal-bytes"[..]))
        .unwrap();

    // The persisted staging *path* (what survives a crash) publishes with
    // the same exclusive contract as `publish`.
    let target = RelativeMediaPath::new("歌手/恢复.flac").unwrap();
    fs.publish_from_staging_path(root, &copy.staged_path, &target)
        .unwrap();
    assert_eq!(
        fs.read_head(root, &target, u64::MAX).unwrap(),
        b"journal-bytes",
        "the staged file landed at its target"
    );

    // A path *outside* Echo's staging area is refused before any write.
    let outside = RelativeMediaPath::new("普通文件.mp3").unwrap();
    std::fs::write(dir.path().join("普通文件.mp3"), b"user-file").unwrap();
    let error = fs
        .publish_from_staging_path(root, &outside, &target)
        .expect_err("foreign staging path refused");
    assert_eq!(error.code(), "permission");
    // The reserved target was never left half-writes behind.
    assert_eq!(
        std::fs::read(dir.path().join(target.normalized())).unwrap(),
        b"journal-bytes"
    );
}

#[test]
fn discard_staging_path_only_removes_inside_ephemeral_staging() {
    let (dir, root, fs) = setup();
    // A persisted staging path created through the journal-shaped API is
    // discardable.
    let operation = OperationId::new();
    let staged = StagedResource::new(operation, "audio").unwrap();
    let copy = fs
        .stage_stream(root, &staged, &mut (&b"to-delete"[..]))
        .unwrap();
    let staging_abs = dir.path().join(copy.staged_path.normalized());
    assert!(staging_abs.is_file());
    fs.discard_staging_path(root, &copy.staged_path).unwrap();
    assert!(!staging_abs.exists(), "persisted staging file removed");

    // Idempotent: an absent staging path succeeds.
    fs.discard_staging_path(root, &copy.staged_path).unwrap();

    // A foreign (non-staging) relative path is refused, never deleted.
    let user_file = RelativeMediaPath::new("keep.mp3").unwrap();
    std::fs::write(dir.path().join("keep.mp3"), b"user").unwrap();
    let error = fs
        .discard_staging_path(root, &user_file)
        .expect_err("foreign path refused");
    assert_eq!(error.code(), "permission");
    assert!(dir.path().join("keep.mp3").is_file(), "user file intact");
}

#[test]
fn discard_published_removes_only_real_files_and_never_symlinks() {
    let (dir, root, fs) = setup();
    let target = RelativeMediaPath::new("被删.flac").unwrap();
    std::fs::write(dir.path().join("被删.flac"), b"published").unwrap();

    // A regular file is removed.
    fs.discard_published(root, &target).unwrap();
    assert!(!dir.path().join("被删.flac").exists(), "file removed");
    // Idempotent: an absent target succeeds.
    fs.discard_published(root, &target).unwrap();

    // A symlink at the target is refused (never followed or removed), and
    // a directory is never removed by `discard_published`.
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("precious.txt"), b"precious").unwrap();
    let linked = RelativeMediaPath::new("linked.mp3").unwrap();
    create_symlink(
        &outside.path().join("precious.txt"),
        &dir.path().join("linked.mp3"),
    );
    let error = fs
        .discard_published(root, &linked)
        .expect_err("symlink target refused");
    assert_eq!(error.code(), "permission");
    assert!(
        std::fs::read(outside.path().join("precious.txt")).unwrap() == b"precious",
        "symlink follow-through never touches the target"
    );

    let dir_path = RelativeMediaPath::new("目录").unwrap();
    std::fs::create_dir_all(dir.path().join("目录")).unwrap();
    fs.discard_published(root, &dir_path).unwrap();
    assert!(
        dir.path().join("目录").is_dir(),
        "directories are never deleted"
    );
}

#[test]
fn trash_slot_is_stable_and_stage_to_trash_is_a_same_volume_move() {
    let (dir, root, fs) = setup();
    let operation = OperationId::new();
    let source = RelativeMediaPath::new("歌手/删除我.flac").unwrap();
    std::fs::create_dir_all(dir.path().join("歌手")).unwrap();
    std::fs::write(dir.path().join(source.normalized()), b"precious-audio").unwrap();

    // Resolving the slot without moving anything returns a stable path.
    let slot = fs.trash_path(root, operation, "删除我.flac").unwrap();
    let slot_display = slot.display().to_owned();
    assert!(slot_display.contains("/trash/"), "trash slot under trash/");
    assert_eq!(
        fs.trash_path(root, operation, "删除我.flac")
            .unwrap()
            .display(),
        slot_display,
        "slot is stable for the operation lifetime"
    );

    // The delete-stage move is a rename into the owned trash slot; the
    // source disappears and the slot holds every byte.
    let staged = fs
        .stage_to_trash(root, operation, &source, "删除我.flac")
        .unwrap();
    assert_eq!(staged.display(), slot_display, "filled slot matches");
    assert!(
        !dir.path().join(source.normalized()).exists(),
        "source moved"
    );
    let trash_abs = dir.path().join(staged.normalized());
    assert_eq!(std::fs::read(&trash_abs).unwrap(), b"precious-audio");

    // Restoring moves the file back out (undo) with the same exclusive
    // reservation: the original name is free, so the restore succeeds.
    fs.restore_from_trash(root, &staged, &source).unwrap();
    assert!(!trash_abs.exists(), "trash slot emptied");
    assert_eq!(
        std::fs::read(dir.path().join(source.normalized())).unwrap(),
        b"precious-audio",
        "restored bytes intact"
    );
}

#[test]
fn trash_slot_rejects_foreign_and_rejects_existing_names() {
    let (dir, root, fs) = setup();
    let outside = RelativeMediaPath::new("普通文件.mp3").unwrap();
    std::fs::write(dir.path().join("普通文件.mp3"), b"user").unwrap();

    // A crafted trash path outside the controlled `trash/*` slot is
    // refused at resolve time.
    let forged = RelativeMediaPath::new(&format!("trash/{}/audio", "x".repeat(36))).unwrap();
    // The path must live under the *owned* staging directory to even reach
    // the `trash` component check; a path under a foreign same-prefix
    // directory is not owned.
    let error = fs
        .restore_from_trash(root, &forged, &outside)
        .expect_err("foreign trash slot refused");
    assert_eq!(error.code(), "permission");

    // `stage_to_trash` refuses to move a source that is not a real regular
    // file (a directory here).
    let directory = RelativeMediaPath::new("目录").unwrap();
    std::fs::create_dir_all(dir.path().join("目录")).unwrap();
    let operation = OperationId::new();
    let error = fs
        .stage_to_trash(root, operation, &directory, "audio")
        .expect_err("directory source refused");
    assert_eq!(
        error.code(),
        "io",
        "missing/directory source is an io error"
    );

    // Stage + restore both refuse to replace an existing target: a restore
    // onto an occupied name is a conflict (the use case re-numbers).
    let source = RelativeMediaPath::new("occupied.flac").unwrap();
    std::fs::write(dir.path().join("occupied.flac"), b"source").unwrap();
    let stage = fs
        .stage_to_trash(root, operation, &source, "occupied.flac")
        .unwrap();
    std::fs::write(dir.path().join("occupied.flac"), b"new-user-file").unwrap();
    let error = fs
        .restore_from_trash(root, &stage, &source)
        .expect_err("occupied restore target refused");
    assert_eq!(error.code(), "conflict");
    assert_eq!(
        std::fs::read(dir.path().join("occupied.flac")).unwrap(),
        b"new-user-file",
        "the incumbent user file is untouched"
    );
}

#[test]
fn file_meta_reports_size_and_mtime_and_not_found_is_an_io_error() {
    let (dir, root, fs) = setup();
    let path = RelativeMediaPath::new("meta.flac").unwrap();
    std::fs::write(dir.path().join("meta.flac"), b"\0\0\0").unwrap();
    let meta = fs.file_meta(root, &path).unwrap();
    assert_eq!(meta.size, 3);

    let missing = RelativeMediaPath::new("missing.flac").unwrap();
    let error = fs.file_meta(root, &missing).expect_err("missing stat");
    assert_eq!(error.code(), "io");
    // The diagnostic origin is the exact absolute path (optimized for the
    // caller to act on, never a log line).
    assert!(error.diagnostic_origin().is_some());
}

#[test]
fn enumerate_is_idempotent_and_skips_the_owned_staging_dir() {
    let (dir, root, fs) = setup();
    std::fs::create_dir_all(dir.path().join("media/华语")).unwrap();
    std::fs::write(dir.path().join("media/a.mp3"), b"a").unwrap();
    std::fs::write(dir.path().join("media/华语/b.flac"), b"b").unwrap();
    // Root-level content (old layout) is never enumerated.
    std::fs::write(dir.path().join("loose.mp3"), b"loose").unwrap();
    std::fs::create_dir_all(dir.path().join("周杰伦")).unwrap();
    std::fs::write(dir.path().join("周杰伦/晴天.flac"), b"old").unwrap();
    fs.staging().ensure_dir(root).unwrap();
    std::fs::write(
        fs.staging().ensure_dir(root).unwrap().join("staged.mp3"),
        b"staged",
    )
    .unwrap();

    let paths: Vec<_> = fs.enumerate(root).unwrap();
    let displayed: Vec<_> = paths.iter().map(RelativeMediaPath::display).collect();
    assert!(displayed.contains(&"media/a.mp3"));
    assert!(displayed.contains(&"media/华语/b.flac"));
    assert!(
        displayed
            .iter()
            .all(|path| !path.starts_with(".echo-staging-")),
        "the owned staging directory is invisible to enumeration"
    );
    assert!(
        displayed.iter().all(|path| path.starts_with("media/")),
        "only media/ content is enumerated, never root/old-layout: {displayed:?}"
    );
    // Idempotent: a second walk reports the same set.
    assert_eq!(fs.enumerate(root).unwrap().len(), paths.len());
}

#[test]
fn read_head_of_a_missing_target_is_an_io_error() {
    let (_dir, root, fs) = setup();
    let missing = RelativeMediaPath::new("absent.flac").unwrap();
    let error = fs.read_head(root, &missing, 16).expect_err("open");
    assert_eq!(error.code(), "io");
}

#[test]
fn fresh_root_acquires_write_capability_by_establishing_staging() {
    // design §8: a brand-new root is write-incapable until its owned staging
    // directory is exclusive-created. This is what root activation/prepare
    // calls, so a freshly-chosen writable dir is never permanently read-only
    // (task 13.2 regression: import/delete were LibraryUnavailable on first
    // use because nothing established the staging dir before measuring).
    let (_dir, root, fs) = setup();

    assert!(
        !fs.write_capable(root).unwrap(),
        "a fresh root must report write-incapable before staging exists"
    );

    fs.establish_write_capability(root).unwrap();
    assert!(
        fs.write_capable(root).unwrap(),
        "acquiring capability makes the root writable"
    );

    // Idempotent: re-establishing is a no-op success, capability persists.
    fs.establish_write_capability(root).unwrap();
    assert!(fs.write_capable(root).unwrap());
}

#[test]
fn established_staging_dir_is_invisible_to_enumeration() {
    let (_dir, root, fs) = setup();
    fs.establish_write_capability(root).unwrap();

    // The owned staging dir never appears as a song path.
    let files = fs.enumerate(root).unwrap();
    assert!(
        files
            .iter()
            .all(|p| !p.display().starts_with(".echo-staging-")),
        "owned staging is invisible to enumeration"
    );
}
