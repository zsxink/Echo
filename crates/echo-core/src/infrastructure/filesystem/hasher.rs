//! The BLAKE3 full-file [`ContentHasher`] (task 4.8).

use std::io::Read;
use std::path::PathBuf;

use crate::application::ports::ContentHasher;
use crate::domain::ids::{LibraryRootId, RelativeMediaPath};
use crate::error::Error;

use super::registry::RootRegistry;

/// Streams a file through BLAKE3 in fixed chunks and returns the hex digest.
///
/// The registry is shared with the file-system adapter so hashing resolves a
/// root to the same directory the enumeration walked — a hash can never
/// silently cross roots.
#[derive(Clone, Debug)]
pub struct Blake3ContentHasher {
    registry: RootRegistry,
    chunk_bytes: u64,
}

impl Blake3ContentHasher {
    #[must_use]
    pub const fn new(registry: RootRegistry) -> Self {
        Self {
            registry,
            chunk_bytes: 64 * 1024,
        }
    }

    fn abs(&self, root: LibraryRootId, path: &RelativeMediaPath) -> Result<PathBuf, Error> {
        Ok(self.registry.path_of(root)?.join(path.normalized()))
    }
}

impl ContentHasher for Blake3ContentHasher {
    fn hash(&self, root: LibraryRootId, path: &RelativeMediaPath) -> Result<String, Error> {
        let abs = self.abs(root, path)?;
        let mut file = std::fs::File::open(&abs)
            .map_err(|source| Error::io("open for hash", source, abs.clone()))?;
        let mut hasher = blake3::Hasher::new();
        let mut buffer = vec![0u8; usize::try_from(self.chunk_bytes).unwrap_or(64 * 1024)];
        loop {
            let read = file
                .read(&mut buffer)
                .map_err(|source| Error::io("hash read", source, abs.clone()))?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
        Ok(hasher.finalize().to_hex().to_string())
    }

    fn hash_of_bytes(&self, bytes: &[u8]) -> String {
        blake3::hash(bytes).to_hex().to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::filesystem::registry::RootRegistry;

    #[test]
    fn hasher_streams_files_and_is_content_keyed() {
        let dir = tempfile::tempdir().unwrap();
        let registry = RootRegistry::new();
        let root = LibraryRootId::new();
        registry.register(root, dir.path());
        let hasher = Blake3ContentHasher::new(registry);
        let path = RelativeMediaPath::new("folder/tone.mp3").unwrap();
        std::fs::create_dir_all(dir.path().join("folder")).unwrap();
        std::fs::write(dir.path().join("folder/tone.mp3"), vec![7u8; 200_000]).unwrap();

        let hash = hasher.hash(root, &path).unwrap();
        assert_eq!(hash, hasher.hash_of_bytes(&vec![7u8; 200_000]));
        // Content change → hash change.
        std::fs::write(dir.path().join("folder/tone.mp3"), vec![8u8; 200_000]).unwrap();
        assert_ne!(hasher.hash(root, &path).unwrap(), hash);
    }
}
