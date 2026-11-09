//! Window-level file-drop capture (issue #117).
//!
//! Tauri emits `WindowEvent::DragDrop` for every stage of a native drag. The
//! shell owns this boundary: it is the only place that sees absolute paths,
//! and it forwards accepted paths to the *existing* OS file-open channel so a
//! drop and a double-click cannot diverge in playback semantics (D1/D4).
//!
//! The `WebView` only ever receives a boolean hover state and a readable
//! rejection message — never a filesystem path (D3). Paths stay in
//! `app://file-open-request`, already decoded by the platform.

use std::path::{Path, PathBuf};

use echo_core::application::scan::is_supported_extension;
use echo_desktop::runtime::StartupSupervisor;
use serde::Serialize;
use tauri::{AppHandle, DragDropEvent, Emitter};

use crate::file_open_gate::deliver_file_opens;

/// Shell→frontend event carrying only the drop-hover state.
pub const FILE_DROP_STATE: &str = "app://file-drop-state";

/// Shell→frontend event carrying a readable reason for a rejected drop.
pub const FILE_DROP_REJECTION: &str = "app://file-drop-rejection";

#[derive(Clone, Serialize)]
pub struct FileDropState {
    pub active: bool,
}

#[derive(Clone, Serialize)]
pub struct FileDropRejection {
    pub message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DropRejection {
    Directory,
    Unsupported,
}

#[derive(Debug, PartialEq, Eq)]
struct PartitionedDrop {
    accepted: Vec<PathBuf>,
    rejected: Vec<(PathBuf, DropRejection)>,
}

/// Whether a native drag event changes the hover overlay, and to what state.
/// Only a drag carrying at least one supported audio file may promise playback;
/// a folder-only or unsupported drag must not show "松手即播放" and then refuse
/// the drop (D1/D5). `Over` carries no paths, so it leaves the current state
/// untouched. `Drop` clears the overlay — a completed drop must never leave a
/// stale hint behind.
fn hover_transition(event: &DragDropEvent) -> Option<bool> {
    match event {
        DragDropEvent::Enter { paths, .. } => Some(!partition(paths.clone()).accepted.is_empty()),
        // `Over` (and any future variant) carries no decision: keep the state
        // the `Enter` chose instead of re-emitting it on every mouse move.
        DragDropEvent::Drop { .. } | DragDropEvent::Leave => Some(false),
        _ => None,
    }
}

/// Classify one dropped path. Directories are rejected outright (Q2); file
/// extensions use the same Core matrix as scan and import, never a second
/// allow-list (D6). The matrix is lower-case, so the comparison normalizes
/// case — Windows files routinely arrive as `.FLAC`.
fn classify(path: &Path) -> Result<(), DropRejection> {
    if path.is_dir() {
        return Err(DropRejection::Directory);
    }
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if is_supported_extension(&extension) {
        Ok(())
    } else {
        Err(DropRejection::Unsupported)
    }
}

/// Split a drop into accepted audio files and readable rejections, preserving
/// the OS-supplied order in the accepted list. No sorting and no de-duplication:
/// "按拖放顺序" is defined by the order `Drop` gives us.
fn partition(paths: Vec<PathBuf>) -> PartitionedDrop {
    let mut accepted = Vec::new();
    let mut rejected = Vec::new();
    for path in paths {
        match classify(&path) {
            Ok(()) => accepted.push(path),
            Err(reason) => rejected.push((path, reason)),
        }
    }
    PartitionedDrop { accepted, rejected }
}

/// A single readable message for all rejected items. Deliberately omits file
/// names/paths: diagnostics must not carry unnecessary user file information.
fn rejection_message(rejected: &[(PathBuf, DropRejection)]) -> Option<String> {
    if rejected.is_empty() {
        return None;
    }
    let directories = rejected
        .iter()
        .filter(|(_, reason)| *reason == DropRejection::Directory)
        .count();
    let files = rejected.len() - directories;
    Some(match (directories, files) {
        (0, count) => format!("已忽略 {count} 个不支持的音频文件"),
        (count, 0) => format!("不支持拖入文件夹（{count} 个），请逐个拖入音频文件"),
        (directory_count, file_count) => format!(
            "已忽略 {directory_count} 个文件夹与 {file_count} 个不支持的文件；请逐个拖入音频文件"
        ),
    })
}

fn emit_state(app: &AppHandle, active: bool) {
    // Losing a hover event only delays a visual hint; it must never fail the
    // drop itself. The playback path below is independent of this emit.
    let _ = app.emit(FILE_DROP_STATE, FileDropState { active });
}

/// Handle one native drag/drop event on the main window.
pub fn handle_drag_drop(app: &AppHandle, startup: &StartupSupervisor, event: &DragDropEvent) {
    if let Some(active) = hover_transition(event) {
        emit_state(app, active);
    }
    if let DragDropEvent::Drop { paths, .. } = event {
        let partitioned = partition(paths.clone());
        if let Some(message) = rejection_message(&partitioned.rejected) {
            let _ = app.emit(FILE_DROP_REJECTION, FileDropRejection { message });
        }
        // One drop = one batched delivery = one `FILE_OPEN_REQUEST` event.
        // Queued paths (frontend listener not ready) are not silently dropped:
        // the startup FIFO drains them in arrival order.
        deliver_file_opens(app, startup, partitioned.accepted);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(name: &str) -> PathBuf {
        PathBuf::from("/tmp/echo-drop-tests").join(name)
    }

    fn enter(paths: Vec<PathBuf>) -> DragDropEvent {
        DragDropEvent::Enter {
            paths,
            position: tauri::PhysicalPosition { x: 0.0, y: 0.0 },
        }
    }

    fn drop(paths: Vec<PathBuf>) -> DragDropEvent {
        DragDropEvent::Drop {
            paths,
            position: tauri::PhysicalPosition { x: 0.0, y: 0.0 },
        }
    }

    #[test]
    fn hover_overlay_follows_drag_lifecycle() {
        let over = DragDropEvent::Over {
            position: tauri::PhysicalPosition { x: 1.0, y: 2.0 },
        };
        assert_eq!(hover_transition(&enter(vec![path("a.flac")])), Some(true));
        // `Over` has no paths: it must not flip a state the `Enter` decided.
        assert_eq!(hover_transition(&over), None);
        assert_eq!(hover_transition(&drop(vec![path("a.flac")])), Some(false));
        assert_eq!(hover_transition(&DragDropEvent::Leave), Some(false));
    }

    #[test]
    fn unsupported_or_directory_only_drag_never_promises_playback() {
        assert_eq!(
            hover_transition(&enter(vec![path("notes.txt"), path("cover.png")])),
            Some(false)
        );
        let directory =
            std::env::temp_dir().join(format!("echo-drop-only-dir-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("create temp directory");
        assert_eq!(
            hover_transition(&enter(vec![directory.clone()])),
            Some(false)
        );
        std::fs::remove_dir(&directory).ok();
        // Mixed drags still show the hint: the supported items will play.
        assert_eq!(
            hover_transition(&enter(vec![path("notes.txt"), path("01.flac")])),
            Some(true)
        );
    }

    #[test]
    fn state_payload_contains_no_paths() {
        let value = serde_json::to_value(FileDropState { active: true }).expect("serialize state");
        let fields = value.as_object().expect("state is an object");
        assert_eq!(fields.len(), 1);
        assert_eq!(fields.get("active"), Some(&serde_json::json!(true)));
    }

    #[test]
    fn supported_extensions_match_core_and_are_case_insensitive() {
        for extension in ["mp3", "flac", "m4a", "mp4", "ogg", "opus", "wav", "ape"] {
            assert_eq!(
                classify(&path(&format!("track.{extension}"))),
                Ok(()),
                "extension {extension} must be accepted"
            );
        }
        assert_eq!(classify(&path("track.FLAC")), Ok(()));
        assert_eq!(
            classify(&path("track.aac")),
            Err(DropRejection::Unsupported)
        );
        assert_eq!(classify(&path("track")), Err(DropRejection::Unsupported));
    }

    #[test]
    fn directories_are_rejected_without_scanning_them() {
        let directory = std::env::temp_dir().join(format!("echo-drop-dir-{}", std::process::id()));
        std::fs::create_dir_all(&directory).expect("create temp directory");
        assert_eq!(classify(&directory), Err(DropRejection::Directory));
        std::fs::remove_dir(&directory).ok();
    }

    #[test]
    fn mixed_drop_keeps_accepted_order_and_reports_each_rejection() {
        let paths = vec![
            path("01.flac"),
            path("notes.txt"),
            path("02.mp3"),
            path("cover.png"),
            path("03.wav"),
        ];
        let partitioned = partition(paths);
        assert_eq!(
            partitioned.accepted,
            vec![path("01.flac"), path("02.mp3"), path("03.wav")]
        );
        assert_eq!(partitioned.rejected.len(), 2);
        assert_eq!(
            rejection_message(&partitioned.rejected),
            Some("已忽略 2 个不支持的音频文件".to_owned())
        );
        assert_eq!(rejection_message(&[]), None);
    }

    #[test]
    fn folder_rejection_has_its_own_readable_message() {
        let rejected = vec![(path("Album"), DropRejection::Directory)];
        assert_eq!(
            rejection_message(&rejected),
            Some("不支持拖入文件夹（1 个），请逐个拖入音频文件".to_owned())
        );
        let mixed = vec![
            (path("Album"), DropRejection::Directory),
            (path("notes.txt"), DropRejection::Unsupported),
        ];
        assert_eq!(
            rejection_message(&mixed),
            Some("已忽略 1 个文件夹与 1 个不支持的文件；请逐个拖入音频文件".to_owned())
        );
    }
}
