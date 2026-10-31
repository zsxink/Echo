//! Real OS permits, including a separately executed test process.
use super::*;

#[test]
fn writer_permit_child_process() {
    let Some(path) = std::env::var_os("ECHO_TEST_PERMIT_ROOT") else {
        return;
    };
    let expected = std::env::var("ECHO_TEST_PERMIT_ALLOWED").unwrap() == "yes";
    let registry = RootRegistry::new();
    let root = LibraryRootId::new();
    registry.register(root, std::path::Path::new(&path));
    let plane = RootControlPlane::new(registry);
    assert_eq!(plane.prepare_write(root, true).is_ok(), expected);
}

fn child(path: &std::path::Path, allowed: bool) {
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "infrastructure::filesystem::control_plane::lifecycle_tests::writer_permit_child_process", "--nocapture"])
        .env("ECHO_TEST_PERMIT_ROOT", path)
        .env("ECHO_TEST_PERMIT_ALLOWED", if allowed { "yes" } else { "no" })
        .status().unwrap();
    assert!(status.success());
}

#[test]
fn writer_permit_is_cross_process_and_does_not_pollute_independent_roots() {
    let directory = tempfile::tempdir().unwrap();
    let first = directory.path().join("库一");
    let second = directory.path().join("库二");
    std::fs::create_dir_all(&first).unwrap();
    std::fs::create_dir_all(&second).unwrap();
    let registry = RootRegistry::new();
    let root = LibraryRootId::new();
    let other = LibraryRootId::new();
    registry.register(root, &first);
    registry.register(other, &second);
    let plane = RootControlPlane::new(registry.clone());
    let guard = plane.prepare_write(root, true).unwrap();
    let independent = RootControlPlane::new(registry);
    let other_guard = independent.prepare_write(other, true).unwrap();
    child(&first, false);
    drop(other_guard);
    child(&second, true);
    drop(guard);
    child(&first, true);
}
