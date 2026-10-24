//! Path spelling rules for persisted and user-facing absolute paths.

use std::path::PathBuf;

/// Convert a verbatim Windows drive path to its user-facing plain form.
///
/// `std::fs::canonicalize` always returns a verbatim path on Windows
/// (`\\?\C:\Users\...`). That form is a valid filesystem spelling but leaks
/// into the UI and the persisted root records (issue #82), so every path Echo
/// *stores or displays* — the [`LibraryRoot`](crate::domain::entities::LibraryRoot)
/// record's absolute path and the root registry binding — uses the
/// conventional plain form instead. Two paired invariants keep the system
/// consistent:
///
/// - **Root identity stays path-bound to the raw canonical form.**
///   [`crate::application::root_switch::derive_root_id`] must keep hashing
///   exactly what `std::fs::canonicalize` produced when the record was
///   created, otherwise an existing Windows library would derive a different
///   root id after this change and lose its identity.
/// - **Containment checks canonicalize both sides**, so they stay internally
///   consistent regardless of which spelling a registry or record holds.
///
/// `\\?\C:\dir` becomes `C:\dir`; anything else (already-plain paths,
/// non-Windows paths, verbatim UNC paths, forms `dunce` cannot unambiguously
/// simplify) passes through unchanged. This is a pure lexical transform — no
/// filesystem access.
#[must_use]
pub fn plain_absolute(path: impl AsRef<std::path::Path>) -> PathBuf {
    dunce::simplified(path.as_ref()).to_path_buf()
}

#[cfg(test)]
mod tests {
    use super::plain_absolute;

    #[test]
    fn plain_and_relative_paths_pass_through_unchanged() {
        let plain = std::path::PathBuf::from("/tmp/music library");
        assert_eq!(plain_absolute(&plain), plain);
        let relative = std::path::PathBuf::from("music");
        assert_eq!(plain_absolute(&relative), relative);
    }
}
