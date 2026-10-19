//! Local resource enumeration + verification helpers, shared by publication paths.
//! [`super::resource_candidates`] fns are re-exported from the parent file.

#[allow(clippy::wildcard_imports, unused_imports)]
use super::*;

#[allow(clippy::redundant_pub_crate)]
pub(crate) fn local_resource_candidates(
    database: &SqliteDatabase,
    library: &LibraryRoot,
    resource: &SharedResourceManifest,
) -> Result<Vec<File>, SyncRuntimeError> {
    let catalog_path = if resource.kind == SharedResourceKind::Audio {
        database
            .local_library_audio_path_for_resource(
                library.id(),
                &resource.digest.to_hex(),
                resource.length,
            )
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?
    } else {
        None
    };
    let published_path = database
        .local_library_resource_path_for_manifest(library.id(), resource)
        .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
    let paths = catalog_path
        .iter()
        .chain(published_path.iter())
        .map(String::as_str)
        .chain(std::iter::once(resource.relative_path.as_str()));
    let mut files = Vec::new();
    for relative_path in paths {
        let Some(path) = authorized_source(library.absolute_path(), relative_path)? else {
            continue;
        };
        match File::open(path) {
            Ok(file) => {
                let metadata = file
                    .metadata()
                    .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
                if metadata.is_file() && metadata.len() == resource.length {
                    files.push(file);
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(SyncRuntimeError::LibraryUnavailable),
        }
    }
    Ok(files)
}

#[allow(clippy::redundant_pub_crate)]
pub(crate) fn verified_local_resource(
    candidates: Vec<File>,
    expected_hash: &str,
    expected_length: u64,
) -> Result<Option<File>, SyncRuntimeError> {
    for mut file in candidates {
        if verify_local_resource(&mut file, expected_hash, expected_length)? {
            return Ok(Some(file));
        }
    }
    Ok(None)
}

#[allow(clippy::redundant_pub_crate)]
pub(crate) fn verify_local_resource(
    file: &mut File,
    expected_hash: &str,
    expected_length: u64,
) -> Result<bool, SyncRuntimeError> {
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0; MAX_RESOURCE_CHUNK_BYTES];
    let mut total = 0_u64;
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
        if read == 0 {
            break;
        }
        total = total
            .checked_add(read as u64)
            .ok_or(SyncRuntimeError::LibraryUnavailable)?;
        hasher.update(&buffer[..read]);
    }
    file.seek(SeekFrom::Start(0))
        .map_err(|_| SyncRuntimeError::LibraryUnavailable)?;
    Ok(total == expected_length && hasher.finalize().to_hex().as_str() == expected_hash)
}

#[allow(clippy::redundant_pub_crate)]
pub(crate) async fn wait_for_sync_cancel(cancelled: Arc<AtomicBool>) {
    while !cancelled.load(Ordering::Acquire) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
