//! Empty library preparation and validation for the first replica enrollment.

use std::path::Path;

use echo_core::application::root_switch::derive_root_id;
use echo_core::domain::ids::LibraryRootId;
use echo_core::infrastructure::filesystem::staging::{StagingMarker, MARKER_FORMAT_VERSION};

use crate::sync_runtime::SyncRuntimeError;

pub(super) const EMPTY_LIBRARY_MANIFEST: &str = "manifest.json";
pub(super) const EMPTY_LIBRARY_STAGING_MARKER: &str = ".echo-ownership-marker";

/// Only Echo's setup files and an empty media directory may be enrolled.
pub(super) fn is_empty_enrollment_root(root: &Path) -> bool {
    fn visit_echo(
        path: &Path,
        relative: &Path,
        library_root: &Path,
        manifest: &mut bool,
        marker: &mut bool,
    ) -> bool {
        let Ok(entries) = std::fs::read_dir(path) else {
            return false;
        };
        for entry in entries {
            let Ok(entry) = entry else { return false };
            let child = entry.path();
            let Ok(metadata) = std::fs::symlink_metadata(&child) else {
                return false;
            };
            if metadata.file_type().is_symlink() {
                return false;
            }
            let child_relative = relative.join(entry.file_name());
            if metadata.is_dir() {
                if !visit_echo(&child, &child_relative, library_root, manifest, marker) {
                    return false;
                }
                continue;
            }
            if !metadata.is_file() {
                return false;
            }
            if relative.as_os_str().is_empty()
                && child_relative == Path::new(EMPTY_LIBRARY_MANIFEST)
            {
                *manifest = true;
            } else if child_relative == Path::new("tmp").join(EMPTY_LIBRARY_STAGING_MARKER) {
                let Ok(contents) = std::fs::read_to_string(&child) else {
                    return false;
                };
                let Some(parsed) = StagingMarker::parse(&contents) else {
                    return false;
                };
                if parsed.root != derive_root_id(library_root)
                    || parsed.version != MARKER_FORMAT_VERSION
                {
                    return false;
                }
                *marker = true;
            } else {
                return false;
            }
        }
        true
    }

    let Ok(canonical_root) = root.canonicalize() else {
        return false;
    };
    let Ok(entries) = std::fs::read_dir(&canonical_root) else {
        return false;
    };
    let mut saw_echo = false;
    for entry in entries {
        let Ok(entry) = entry else { return false };
        let Ok(metadata) = std::fs::symlink_metadata(entry.path()) else {
            return false;
        };
        if metadata.file_type().is_symlink() {
            return false;
        }
        match entry.file_name().to_str() {
            Some("echo") if metadata.is_dir() => {
                saw_echo = true;
                let mut manifest = false;
                let mut marker = false;
                if !visit_echo(
                    &entry.path(),
                    Path::new(""),
                    &canonical_root,
                    &mut manifest,
                    &mut marker,
                ) || !manifest
                    || !marker
                {
                    return false;
                }
            }
            Some("media") if metadata.is_dir() => {
                let Ok(mut children) = std::fs::read_dir(entry.path()) else {
                    return false;
                };
                if children.next().is_some() {
                    return false;
                }
            }
            _ => return false,
        }
    }
    saw_echo
}

/// Initialize only a truly empty, selected library directory for enrollment.
pub(super) fn prepare_empty_enrollment_root(
    root: &Path,
    root_id: LibraryRootId,
) -> Result<(), SyncRuntimeError> {
    if is_empty_enrollment_root(root) {
        return Ok(());
    }
    let metadata =
        std::fs::symlink_metadata(root).map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
    if !metadata.file_type().is_dir()
        || std::fs::read_dir(root)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
            .next()
            .is_some()
    {
        return Err(SyncRuntimeError::LibraryUnavailable);
    }
    let echo = root.join("echo");
    let temporary = echo.join("tmp");
    std::fs::create_dir(&echo).map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
    std::fs::create_dir(&temporary).map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(echo.join(EMPTY_LIBRARY_MANIFEST))
        .and_then(|mut file| {
            use std::io::Write as _;
            file.write_all(b"{}")?;
            file.sync_all()
        })
        .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(temporary.join(EMPTY_LIBRARY_STAGING_MARKER))
        .and_then(|mut file| {
            use std::io::Write as _;
            file.write_all(StagingMarker::new(root_id).body().as_bytes())?;
            file.sync_all()
        })
        .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
    if !is_empty_enrollment_root(root) {
        return Err(SyncRuntimeError::LibraryUnavailable);
    }
    Ok(())
}
