//! Single-file import source for the "import current temporary playback item"
//! flow (task 11.7).
//!
//! A temporary item (a file opened outside the active library) has no library
//! `SongId`. To adopt it into the library the desktop imports its absolute path
//! directly — without a picker dialog and **without ever passing that path to
//! the WebView**. This module owns the absolute path on the desktop side and
//! presents it to Core's `PlanImport` through the [`ImportSourceReader`]
//! contract (design §10: sources are logical, content is streamed).
//!
//! `#![forbid(unsafe_code)]` is inherited from `platform/mod.rs`.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use echo_core::application::ports::{
    ImportSource, ImportSourceInfo, ImportSourceReader, SidecarInfo,
};
use echo_core::error::Error;

/// A single external file (by absolute, desktop-owned path) exposed as one
/// [`ImportSource`]. Reads are streamed straight through Core's controlled
/// staging; the path stays inside this struct and never appears in a DTO.
///
/// The logical source key is derived from the leaf file name, so Core sees a
/// stable, non-path handle (`ImportSource::new` rejects path syntax).
#[derive(Debug)]
pub struct SingleFileImport {
    path: PathBuf,
    source: ImportSource,
    display_name: String,
}

impl SingleFileImport {
    /// Build a single-file import from a desktop-owned absolute path.
    ///
    /// # Errors
    ///
    /// `Validation` if the path has no usable file name or cannot yield a
    /// non-path logical source key; `Unavailable` if the path is not a readable
    /// regular file.
    pub fn new(path: impl Into<PathBuf>, display_name: impl Into<String>) -> Result<Self, Error> {
        let path = path.into();
        let display_name = display_name.into();
        let key = path
            .file_name()
            .and_then(|n| n.to_str())
            .map(str::to_owned)
            .ok_or_else(|| {
                Error::validation(echo_core::error::Subject::Other, "path", "no file name")
            })?;
        let file = fs::File::open(&path)
            .map_err(|e| Error::unavailable("file", format!("cannot open source: {e}")))?;
        let meta = file
            .metadata()
            .map_err(|e| Error::unavailable("file", format!("cannot stat source: {e}")))?;
        if !meta.is_file() {
            return Err(Error::validation(
                echo_core::error::Subject::Other,
                "path",
                "source is not a regular file",
            ));
        }
        Ok(Self {
            path,
            source: ImportSource::new(key)?,
            display_name,
        })
    }

    /// The logical handle consumed by `PlanImport`.
    #[must_use]
    pub const fn source(&self) -> &ImportSource {
        &self.source
    }

    /// Try to resolve a same-basename `.lrc` sidecar beside `audio_path`.
    /// Returns the absolute path to the sidecar and its display metadata if
    /// found and is a regular file; `None` when no sidecar exists.
    fn resolve_sidecar(audio_path: &Path, audio_display: &str) -> Option<(PathBuf, SidecarInfo)> {
        let stem = match audio_display.rfind('.') {
            Some(i) if i > 0 => &audio_display[..i],
            _ => audio_display,
        };
        for ext in ["lrc", "LRC"] {
            let candidate = audio_path.with_file_name(format!("{stem}.{ext}"));
            if let Ok(meta) = fs::metadata(&candidate) {
                if meta.is_file() {
                    let name = candidate
                        .file_name()
                        .and_then(|n| n.to_str())
                        .unwrap_or("sidecar.lrc")
                        .to_owned();
                    return Some((
                        candidate,
                        SidecarInfo {
                            display_name: name,
                            size: meta.len(),
                        },
                    ));
                }
            }
        }
        None
    }
}

impl ImportSourceReader for SingleFileImport {
    fn describe(&self, source: &ImportSource) -> Result<ImportSourceInfo, Error> {
        if source != &self.source {
            return Err(Error::validation(
                echo_core::error::Subject::Other,
                "source",
                "unknown source",
            ));
        }
        let meta = fs::metadata(&self.path)
            .map_err(|e| Error::unavailable("file", format!("cannot stat source: {e}")))?;
        if !meta.is_file() {
            return Err(Error::validation(
                echo_core::error::Subject::Other,
                "source",
                "source is not a regular file",
            ));
        }
        Ok(ImportSourceInfo {
            display_name: self.display_name.clone(),
            size: meta.len(),
        })
    }

    fn open<'a>(&'a self, source: &ImportSource) -> Result<Box<dyn Read + 'a>, Error> {
        if source != &self.source {
            return Err(Error::validation(
                echo_core::error::Subject::Other,
                "source",
                "unknown source",
            ));
        }
        let file = fs::File::open(&self.path)
            .map_err(|e| Error::unavailable("file", format!("cannot open source: {e}")))?;
        Ok(Box::new(file))
    }

    fn sidecar(&self, source: &ImportSource) -> Result<Option<SidecarInfo>, Error> {
        if source != &self.source {
            return Err(Error::validation(
                echo_core::error::Subject::Other,
                "source",
                "unknown source",
            ));
        }
        Ok(Self::resolve_sidecar(&self.path, &self.display_name).map(|(_p, info)| info))
    }

    fn open_sidecar<'a>(
        &'a self,
        source: &ImportSource,
    ) -> Result<Option<Box<dyn Read + 'a>>, Error> {
        if source != &self.source {
            return Err(Error::validation(
                echo_core::error::Subject::Other,
                "source",
                "unknown source",
            ));
        }
        let Some((side_path, _info)) = Self::resolve_sidecar(&self.path, &self.display_name) else {
            return Ok(None);
        };
        let file = fs::File::open(&side_path)
            .map_err(|e| Error::unavailable("file", format!("cannot open sidecar: {e}")))?;
        Ok(Some(Box::new(file)))
    }
}

/// Convenience for the import command: resolve the absolute path plus a display
/// name into a ready single-file import.
///
/// # Errors
///
/// Propagates [`SingleFileImport::new`] errors.
pub fn single_file_import(
    path: impl Into<PathBuf>,
    display_name: impl Into<String>,
) -> Result<SingleFileImport, Error> {
    SingleFileImport::new(path, display_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;

    fn write_temp(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, bytes).expect("write temp");
        p
    }

    fn tempdir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn describes_and_reads_a_real_file() {
        let dir = tempdir();
        let path = write_temp(dir.path(), "song.flac", b"hello-audio");
        let import = single_file_import(&path, "song.flac").expect("import");
        let info = import.describe(import.source()).expect("describe");
        assert_eq!(info.display_name, "song.flac");
        assert_eq!(info.size, 11);
        let mut buf = String::new();
        import
            .open(import.source())
            .expect("open")
            .read_to_string(&mut buf)
            .expect("read");
        assert_eq!(buf, "hello-audio");
    }

    #[test]
    fn refuses_a_directory() {
        let err = SingleFileImport::new("/tmp", "tmp").expect_err("must refuse a directory");
        // unavailable for non-regular-file paths (metadata.ok but not a file)
        // or validation. The implementation uses Unavailable for stat errors,
        // Validation for non-file. /tmp is a directory: validation error.
        match err {
            Error::Validation { .. } | Error::Unavailable { .. } => {}
            other => panic!("unexpected: {other}"),
        }
    }

    #[test]
    fn picks_up_lrc_sidecar() {
        let dir = tempdir();
        let audio = write_temp(dir.path(), "track.mp3", b"audio");
        write_temp(dir.path(), "track.lrc", b"sidecar");
        let import = single_file_import(&audio, "track.mp3").expect("import");
        let side = import
            .sidecar(import.source())
            .expect("sidecar")
            .expect("found");
        assert_eq!(side.display_name, "track.lrc");
        assert_eq!(side.size, 7);
        let mut buf = String::new();
        import
            .open_sidecar(import.source())
            .expect("open_sidecar")
            .expect("found")
            .read_to_string(&mut buf)
            .expect("read sidecar");
        assert_eq!(buf, "sidecar");
    }

    #[test]
    fn unknown_source_key_is_rejected() {
        let dir = tempdir();
        let path = write_temp(dir.path(), "a.mp3", b"data");
        let import = single_file_import(&path, "a.mp3").expect("import");
        let other = ImportSource::new("something-else.mp3").expect("other key");
        import.describe(&other).expect_err("must reject");
        assert!(import.open(&other).is_err(), "must reject unknown source");
    }
}
