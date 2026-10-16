//! Library preparation for the first replica enrollment.

use std::path::Path;

use echo_core::application::root_switch::derive_root_id;
use echo_core::domain::ids::LibraryRootId;
use echo_core::infrastructure::filesystem::staging::{MARKER_FORMAT_VERSION, StagingMarker};

use crate::sync_runtime::SyncRuntimeError;

pub(super) const EMPTY_LIBRARY_MANIFEST: &str = "manifest.json";
pub(super) const EMPTY_LIBRARY_STAGING_MARKER: &str = ".echo-ownership-marker";

/// Preserve the old media outside the scanner's media tree. The snapshot token
/// is the durable operation identity: after a crash an existing destination
/// proves the rename completed and must never be replaced or moved a second time.
pub(super) fn archive_previous_media(
    root: &Path,
    token: &str,
    library_id: echo_core::domain::sync::LibraryId,
) -> Result<(), SyncRuntimeError> {
    if token.len() != 32
        || !token
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(SyncRuntimeError::LibraryUnavailable);
    }
    let archive = root.join("echo").join("archive");
    let operation = archive.join(format!("local-before-sync-{token}"));
    for directory in [root, root.join("echo").as_path(), &archive, &operation] {
        match std::fs::symlink_metadata(directory) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(directory).map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
            }
            _ => return Err(SyncRuntimeError::LibraryUnavailable),
        }
    }
    let source = root.join("media");
    let destination = operation.join("media");
    archive_directory(&source, &destination)?;
    // Portable records are another projection of the old library. Preserve
    // them beside the old media so root activation cannot replay them later.
    archive_directory(&root.join("echo/records"), &operation.join("records"))?;
    let manifest = root.join("echo").join(EMPTY_LIBRARY_MANIFEST);
    let saved_manifest = operation.join(EMPTY_LIBRARY_MANIFEST);
    if !saved_manifest.exists() {
        std::fs::rename(&manifest, &saved_manifest)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
    }
    let saved_metadata = std::fs::symlink_metadata(&saved_manifest)
        .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
    if !saved_metadata.is_file() || saved_metadata.file_type().is_symlink() {
        return Err(SyncRuntimeError::LibraryUnavailable);
    }
    let manifest_body =
        serde_json::to_vec_pretty(&echo_core::domain::library::LibraryManifest::new(
            echo_core::domain::library::LibraryId::from_uuid(library_id.as_uuid()),
            env!("CARGO_PKG_VERSION"),
        ))
        .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
    let temporary = operation.join("new-manifest.json");
    let mut file =
        std::fs::File::create(&temporary).map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
    use std::io::Write as _;
    file.write_all(&manifest_body)
        .and_then(|()| file.sync_all())
        .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
    // `std::fs::rename` does not replace an existing destination on Windows.
    // The original is already preserved in the archive, so remove only a
    // regular retry artifact while the durable enrollment gate is still set.
    match std::fs::symlink_metadata(&manifest) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            std::fs::remove_file(&manifest).map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        _ => return Err(SyncRuntimeError::LibraryUnavailable),
    }
    std::fs::rename(&temporary, &manifest).map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
    // Directory fsync is best effort on platforms that support it, matching
    // the control-plane adapter's atomic-publication boundary.
    for path in [root, root.join("echo").as_path(), operation.as_path()] {
        if let Ok(directory) = std::fs::File::open(path) {
            let _ = directory.sync_all();
        }
    }
    Ok(())
}

fn archive_directory(source: &Path, destination: &Path) -> Result<(), SyncRuntimeError> {
    match std::fs::symlink_metadata(&destination) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            match std::fs::symlink_metadata(&source) {
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
                    std::fs::rename(source, destination)
                        .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    // Record an empty archive as proof for replay as well.
                    std::fs::create_dir(destination)
                        .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
                }
                _ => return Err(SyncRuntimeError::LibraryUnavailable),
            }
        }
        _ => return Err(SyncRuntimeError::LibraryUnavailable),
    }
    match std::fs::symlink_metadata(source) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            std::fs::create_dir(source).map_err(|_| SyncRuntimeError::LibraryUnavailable)
        }
        _ => Err(SyncRuntimeError::LibraryUnavailable),
    }
}

/// Adopt the selected Echo library without touching its existing media. Its
/// catalog is replaced only after the complete host snapshot is validated.
pub(super) fn prepare_enrollment_root(
    root: &Path,
    root_id: LibraryRootId,
) -> Result<(), SyncRuntimeError> {
    let echo = root.join("echo");
    if !echo.exists() {
        return prepare_empty_enrollment_root(root, root_id);
    }
    for directory in [root.to_path_buf(), echo.clone(), echo.join("tmp")] {
        let metadata = std::fs::symlink_metadata(directory)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        if !metadata.is_dir() || metadata.file_type().is_symlink() {
            return Err(SyncRuntimeError::LibraryUnavailable);
        }
    }
    let manifest = std::fs::symlink_metadata(echo.join(EMPTY_LIBRARY_MANIFEST))
        .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
    if !manifest.is_file() || manifest.file_type().is_symlink() {
        return Err(SyncRuntimeError::LibraryUnavailable);
    }
    let marker = echo.join("tmp").join(EMPTY_LIBRARY_STAGING_MARKER);
    let metadata =
        std::fs::symlink_metadata(&marker).map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
    let contents =
        std::fs::read_to_string(marker).map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
    let valid = StagingMarker::parse(&contents)
        .is_some_and(|marker| marker.root == root_id && marker.version == MARKER_FORMAT_VERSION);
    if !metadata.is_file() || metadata.file_type().is_symlink() || !valid {
        return Err(SyncRuntimeError::LibraryUnavailable);
    }
    Ok(())
}

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
