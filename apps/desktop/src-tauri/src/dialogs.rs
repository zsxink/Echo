//! Real system dialog/reveal adapters for the desktop shell (task 7.5 wired).
//!
//! `echo-desktop`'s `SystemDialogs` contract keeps the [`WebView`] free of any
//! filesystem capability. The shell owns the real UI: native directory/file
//! pickers via `tauri-plugin-dialog` (Rust-side `DialogExt`, which does not go
//! through the [`WebView`] capability system) and reveal-in-folder via
//! `tauri-plugin-opener`. Nothing here returns an absolute path to the
//! frontend — picked paths are converted to logical `ImportSource` handles and
//! consumed desktop-side.
//!
//! `#![forbid(unsafe_code)]` is inherited from `platform/mod.rs`.

use echo_core::application::ports::{ImportSource, ImportSourceReader};
use echo_core::error::Error;
use echo_desktop::platform::dialogs::{PickedImport, RevealOutcome, SystemDialogs};
use echo_desktop::platform::import::{single_file_import, SingleFileImport};
use tauri::AppHandle;
use tauri_plugin_dialog::{DialogExt, FilePath};

/// The real `SystemDialogs` implementation: every entry point opens an OS
/// native dialog or reveal. Construction requires an `AppHandle` so plugins
/// resolve against the running app.
pub struct TauriDialogs {
    handle: AppHandle,
}

impl TauriDialogs {
    /// Bind to the running app handle.
    #[must_use]
    pub const fn new(handle: AppHandle) -> Self {
        Self { handle }
    }
}

impl SystemDialogs for TauriDialogs {
    fn pick_library_directory(&self) -> Result<Option<std::path::PathBuf>, Error> {
        self.handle
            .dialog()
            .file()
            .blocking_pick_folder()
            .map(|file| {
                file.into_path().map_err(|source| {
                    Error::unavailable(
                        "picked directory",
                        format!("cannot resolve picked directory: {source}"),
                    )
                })
            })
            .transpose()
    }

    fn pick_audio_files(&self) -> Result<Option<PickedImport>, Error> {
        let extensions: Vec<&str> = {
            use echo_desktop::platform::dialogs::AUDIO_DIALOG_EXTENSIONS;
            AUDIO_DIALOG_EXTENSIONS.to_vec()
        };
        let some = self
            .handle
            .dialog()
            .file()
            .add_filter("音频", &extensions)
            .blocking_pick_files();
        let Some(files) = some else {
            return Ok(None);
        };
        let batch = MultiFileImport::from_picked(files)?;
        let sources = batch.files.iter().map(|f| f.source().clone()).collect();
        Ok(Some(PickedImport {
            sources,
            reader: Box::new(batch),
        }))
    }

    fn reveal(&self, absolute: &std::path::Path) -> Result<RevealOutcome, Error> {
        match tauri_plugin_opener::reveal_item_in_dir(absolute) {
            Ok(()) => Ok(RevealOutcome::Revealed),
            Err(_) => Ok(RevealOutcome::Unavailable),
        }
    }
}

/// The Gate-only dialog boundary: instead of opening an OS picker, it reads the
/// exact root/import paths from env vars, so a native run can be scripted to a
/// hermetic temp library and be killed/restarted safely. Production never sets
/// these vars — they are the native-driver equivalent of
/// `ECHO_GATE_OPEN_LOG`/`ECHO_GATE_QUIT_AFTER_MS` (task 13.2/13.3).
pub struct GateDialogs;

impl SystemDialogs for GateDialogs {
    fn pick_library_directory(&self) -> Result<Option<std::path::PathBuf>, Error> {
        // No root scripted = treat as a "cancelled" dialog (Ok(None)), so a
        // driver that does not pick a root behaves like a user who declined.
        std::env::var("ECHO_GATE_ROOT")
            .map_or_else(|_| Ok(None), |dir| Ok(Some(std::path::PathBuf::from(dir))))
    }

    fn pick_audio_files(&self) -> Result<Option<PickedImport>, Error> {
        let Ok(list) = std::env::var("ECHO_GATE_IMPORT") else {
            return Ok(None);
        };
        let paths = list
            .split('\n')
            .filter(|p| !p.trim().is_empty())
            .map(std::path::PathBuf::from)
            .collect::<Vec<_>>();
        if paths.is_empty() {
            return Ok(None);
        }
        let batch = MultiFileImport::from_paths(paths)?;
        let sources: Vec<ImportSource> = batch.files.iter().map(|f| f.source().clone()).collect();
        Ok(Some(PickedImport {
            sources,
            reader: Box::new(batch),
        }))
    }

    fn reveal(&self, _absolute: &std::path::Path) -> Result<RevealOutcome, Error> {
        // The Gate never opens a real file manager; the reveal adapter logic is
        // covered by its own unit tests (task 9.5).
        Ok(RevealOutcome::Revealed)
    }
}

/// A multi-file `ImportSourceReader` over real picked paths. Each picked file
/// is wrapped as a [`SingleFileImport`]; the reader dispatches by logical
/// source key so `PlanImport` sees a stable, path-free handle per resource.
#[derive(Debug)]
struct MultiFileImport {
    files: Vec<SingleFileImport>,
}

impl MultiFileImport {
    /// Wrap the desktop-picked paths (already converted to `PathBuf`) as one
    /// import batch.
    ///
    /// # Errors
    ///
    /// `Unavailable`/`Validation` when any picked path cannot be opened as a
    /// regular file; the whole batch is refused (never a partial fake batch).
    fn from_paths(paths: Vec<std::path::PathBuf>) -> Result<Self, Error> {
        let files = paths
            .into_iter()
            .map(|path| {
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("audio")
                    .to_owned();
                single_file_import(&path, name)
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { files })
    }

    /// Convert `tauri_plugin_dialog::FilePath` values to `PathBuf`, then wrap.
    ///
    /// # Errors
    ///
    /// Same as [`Self::from_paths`].
    fn from_picked(files: Vec<FilePath>) -> Result<Self, Error> {
        let paths = files
            .into_iter()
            .map(|file| {
                file.into_path().map_err(|source| {
                    Error::unavailable(
                        "picked file",
                        format!("cannot resolve picked file: {source}"),
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Self::from_paths(paths)
    }

    fn by_key(&self, source: &ImportSource) -> Option<&SingleFileImport> {
        self.files.iter().find(|f| f.source() == source)
    }
}

impl ImportSourceReader for MultiFileImport {
    fn describe(
        &self,
        source: &ImportSource,
    ) -> Result<echo_core::application::ports::ImportSourceInfo, Error> {
        self.by_key(source)
            .ok_or_else(|| {
                Error::validation(echo_core::error::Subject::Other, "source", "unknown source")
            })
            .and_then(|f| f.describe(source))
    }

    fn open<'a>(&'a self, source: &ImportSource) -> Result<Box<dyn std::io::Read + 'a>, Error> {
        self.by_key(source)
            .ok_or_else(|| {
                Error::validation(echo_core::error::Subject::Other, "source", "unknown source")
            })
            .and_then(|f| f.open(source))
    }

    fn sidecar(
        &self,
        source: &ImportSource,
    ) -> Result<Option<echo_core::application::ports::SidecarInfo>, Error> {
        self.by_key(source)
            .ok_or_else(|| {
                Error::validation(echo_core::error::Subject::Other, "source", "unknown source")
            })
            .and_then(|f| f.sidecar(source))
    }

    fn open_sidecar<'a>(
        &'a self,
        source: &ImportSource,
    ) -> Result<Option<Box<dyn std::io::Read + 'a>>, Error> {
        self.by_key(source)
            .ok_or_else(|| {
                Error::validation(echo_core::error::Subject::Other, "source", "unknown source")
            })
            .and_then(|f| f.open_sidecar(source))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read;
    use std::path::Path;

    fn write_temp(dir: &Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
        let p = dir.join(name);
        std::fs::write(&p, bytes).expect("write temp");
        p
    }

    fn tempdir() -> tempfile::TempDir {
        tempfile::tempdir().expect("tempdir")
    }

    #[test]
    fn multi_file_import_reads_each_picked_source_by_logical_key() {
        let dir = tempdir();
        let a = write_temp(dir.path(), "a.mp3", b"audio-a");
        let b = write_temp(dir.path(), "b.flac", b"audio-b");
        let batch = MultiFileImport::from_paths(vec![a, b]).expect("batch");
        // The reader exposes real `ImportSource` handles Core can plan on.
        let sources: Vec<ImportSource> = batch.files.iter().map(|f| f.source().clone()).collect();
        assert_eq!(sources.len(), 2);
        for source in &sources {
            let info = batch.describe(source).expect("describe");
            assert!(info.size > 0);
        }
        let mut buf = String::new();
        batch
            .open(&sources[0])
            .expect("open first")
            .read_to_string(&mut buf)
            .expect("read first");
        assert_eq!(buf, "audio-a");
    }

    #[test]
    fn multi_file_import_rejects_unknown_source_key() {
        let dir = tempdir();
        let a = write_temp(dir.path(), "a.mp3", b"audio");
        let batch = MultiFileImport::from_paths(vec![a]).expect("batch");
        let unknown = ImportSource::new("other.mp3").expect("key");
        assert!(batch.describe(&unknown).is_err());
        assert!(batch.open(&unknown).is_err());
        assert!(batch.sidecar(&unknown).is_err());
    }

    #[test]
    fn multi_file_import_resolves_lrc_sidecar() {
        let dir = tempdir();
        let audio = write_temp(dir.path(), "track.mp3", b"audio");
        write_temp(dir.path(), "track.lrc", b"sidecar");
        let batch = MultiFileImport::from_paths(vec![audio]).expect("batch");
        let source = batch.files[0].source().clone();
        let side = batch.sidecar(&source).expect("sidecar").expect("found");
        assert_eq!(side.display_name, "track.lrc");
        let mut buf = String::new();
        batch
            .open_sidecar(&source)
            .expect("open sidecar")
            .expect("found")
            .read_to_string(&mut buf)
            .expect("read sidecar");
        assert_eq!(buf, "sidecar");
    }

    #[test]
    fn multi_file_import_refuses_a_directory() {
        let dir = tempdir();
        let err = MultiFileImport::from_paths(vec![dir.path().to_path_buf()]).expect_err("refuse");
        match err {
            Error::Validation { .. } | Error::Unavailable { .. } => {}
            other => panic!("unexpected: {other}"),
        }
    }
}
