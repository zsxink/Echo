//! Windows-specific behavior of [`echo_core::domain::paths::plain_absolute`]
//! (issue #82). Lives outside `src/` so the architecture guard — which bans
//! platform-`cfg` branches in every layer — does not scan it; the Windows-only
//! assertions here are exactly the branch the production code delegates to
//! `dunce` for.

use echo_core::domain::paths::plain_absolute;

#[cfg(windows)]
#[test]
fn verbatim_drive_paths_are_stripped() {
    use std::path::PathBuf;
    assert_eq!(
        plain_absolute(PathBuf::from(r"\\?\C:\Users\xian\Music")),
        PathBuf::from(r"C:\Users\xian\Music")
    );
}

#[cfg(windows)]
#[test]
fn verbatim_unc_paths_stay_verbatim() {
    use std::path::PathBuf;
    // dunce is conservative about UNC: a verbatim UNC path stays verbatim
    // because the plain `\\server\share` spelling cannot address every
    // directory a verbatim one can. Library roots on mapped drives (the
    // issue #82 report) are drive paths and are stripped.
    assert_eq!(
        plain_absolute(PathBuf::from(r"\\?\UNC\server\share\music")),
        PathBuf::from(r"\\?\UNC\server\share\music")
    );
}

#[test]
fn round_trip_through_real_canonicalize_is_plain() {
    let dir = tempfile::tempdir().expect("temp dir");
    let canonical = dir.path().canonicalize().expect("canonical");
    let plain = plain_absolute(&canonical);
    #[cfg(windows)]
    assert!(
        !plain.to_string_lossy().starts_with(r"\\?\"),
        "canonicalized temp dir must not stay verbatim: {plain:?}"
    );
    // The plain spelling still addresses the same directory.
    assert_eq!(plain.canonicalize().unwrap(), canonical);
}
