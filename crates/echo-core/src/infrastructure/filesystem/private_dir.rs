//! Narrow OS-specific primitive for owner-only scratch directories.
//!
//! Used by adapters outside every library root (for example the SQLite
//! projection-rebuild candidate, which holds a full copy of library data in
//! the application data directory): the directory must never be wider than
//! its owner, and the restriction must hold from the moment it exists.

use crate::error::Error;
use std::path::Path;

#[cfg(unix)]
pub fn create_owner_only_dir(path: &Path) -> Result<(), Error> {
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(path)
        .map_err(|error| Error::io("create private directory", error, path))
}

#[cfg(not(unix))]
pub fn create_owner_only_dir(path: &Path) -> Result<(), Error> {
    std::fs::create_dir(path).map_err(|error| Error::io("create private directory", error, path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_only_dir_is_created_owner_restricted() {
        let path = std::env::temp_dir().join(format!("echo-private-dir-{}", uuid::Uuid::new_v4()));
        create_owner_only_dir(&path).expect("create private directory");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path)
                .expect("private directory metadata")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o700);
        }
        #[cfg(not(unix))]
        {
            assert!(path.is_dir());
        }
        std::fs::remove_dir_all(&path).expect("cleanup private directory");
    }
}
