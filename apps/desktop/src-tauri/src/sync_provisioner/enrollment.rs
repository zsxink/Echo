//! Library preparation for the first replica enrollment.

use std::path::Path;

#[cfg(test)]
use echo_core::application::root_switch::derive_root_id;
use echo_core::domain::ids::LibraryRootId;
use echo_core::infrastructure::filesystem::staging::{MARKER_FORMAT_VERSION, StagingMarker};

use crate::sync_runtime::SyncRuntimeError;

pub(super) const EMPTY_LIBRARY_MANIFEST: &str = "manifest.json";
pub(super) const EMPTY_LIBRARY_STAGING_MARKER: &str = ".echo-ownership-marker";

/// Adopt the selected library without touching its existing files. A normal
/// music directory does not need to be empty before it can join a host; only
/// Echo's own control files are created or updated here. Its catalog is merged
/// only after the complete host snapshot is validated.
pub(super) fn prepare_enrollment_root(
    root: &Path,
    root_id: LibraryRootId,
) -> Result<(), SyncRuntimeError> {
    let root_metadata =
        std::fs::symlink_metadata(root).map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
    if !root_metadata.is_dir() || root_metadata.file_type().is_symlink() {
        return Err(SyncRuntimeError::LibraryUnavailable);
    }

    let echo = root.join("echo");
    let mut created_echo = false;
    match std::fs::symlink_metadata(&echo) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return Err(SyncRuntimeError::LibraryUnavailable),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(&echo).map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
            created_echo = true;
        }
        Err(_) => return Err(SyncRuntimeError::LibraryUnavailable),
    }

    let manifest_path = echo.join(EMPTY_LIBRARY_MANIFEST);
    match std::fs::symlink_metadata(&manifest_path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return Err(SyncRuntimeError::LibraryUnavailable),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&manifest_path)
                .and_then(|mut file| {
                    use std::io::Write as _;
                    file.write_all(b"{}")?;
                    file.sync_all()
                })
                .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        }
        Err(_) => return Err(SyncRuntimeError::LibraryUnavailable),
    }

    // Existing Echo catalogs remain joinable when their local staging area
    // was never created or is read-only. New directories still get the
    // staging marker needed by Echo's normal import and scan flows.
    let temporary = echo.join("tmp");
    match std::fs::symlink_metadata(&temporary) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Ok(_) => return Err(SyncRuntimeError::LibraryUnavailable),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && created_echo => {
            std::fs::create_dir(&temporary).map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(SyncRuntimeError::LibraryUnavailable),
    }

    let marker_path = temporary.join(EMPTY_LIBRARY_STAGING_MARKER);
    match std::fs::symlink_metadata(&marker_path) {
        Ok(metadata) => {
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(SyncRuntimeError::LibraryUnavailable);
            }
            let contents = std::fs::read_to_string(&marker_path)
                .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
            let Some(marker) = StagingMarker::parse(&contents) else {
                return Err(SyncRuntimeError::LibraryUnavailable);
            };
            if marker.version != MARKER_FORMAT_VERSION {
                return Err(SyncRuntimeError::LibraryUnavailable);
            }
            // A valid marker can travel with a copied library. Rebind this
            // Echo-owned staging area to the path-derived identity on this
            // machine; media and catalog files remain untouched.
            if marker.root != root_id {
                std::fs::write(&marker_path, StagingMarker::new(root_id).body())
                    .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && created_echo => {
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&marker_path)
                .and_then(|mut file| {
                    use std::io::Write as _;
                    file.write_all(StagingMarker::new(root_id).body().as_bytes())?;
                    file.sync_all()
                })
                .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(SyncRuntimeError::LibraryUnavailable),
    }
    Ok(())
}

/// Only Echo's setup files and an empty media directory may be enrolled.
#[cfg(test)]
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
#[cfg(test)]
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
