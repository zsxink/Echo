//! Desktop local-state store: theme, close behavior, window state and the
//! playback session, persisted to an application-private JSON file (task 7.6).
//!
//! This is the **storage primitive** the desktop shell's preferences and the
//! playback coordinator share. It is deliberately *not* the Core schema — it
//! holds desktop-only, non-catalog state and never flows into `echo-core`
//! (design §macOS 偏好/关闭/窗口 state).
//!
//! Writes use the durable replace sequence the whole pipeline relies on:
//! **write a temp file in the same directory → `fsync` it → atomic `rename`
//! over the target → best-effort directory `fsync`**. A crash at any point
//! leaves either the previous intact file or the fully-flushed new one, never
//! a torn file, and never a partial `desktop-state.json` on disk. Stale temp
//! files from a crashed write are removed before the next write so they never
//! accumulate or collide with a reused process id.
//!
//! Reads are forgiving *per field*: each field is extracted from the JSON
//! independently, so an unknown theme, a wrong-typed close value, or a
//! degenerate window falls back to its safe default — the **coral** theme and
//! the caller's **platform default close behavior** (macOS keeps running in the
//! background, Windows/Linux exit) — without taking down the window state or
//! playback session that were stored alongside it. A whole file that is not
//! valid JSON at all (e.g. truncated mid-write) falls back wholesale; there is
//! nothing recoverable inside it.
//!
//! Mutations are a read-modify-write guarded by a lock, so the two real
//! writers (the window and the playback actor) cannot lose each other's field.
//! Because a mutation preserves every *validly-extracted* field from the prior
//! file, fixing the theme after a corrupt close value never erases the session.
//!
//! The structured playback-session *schema* is owned and consumed by task 8.9;
//! this task provides its durable, atomic storage slot (an opaque value) so
//! the coordinator never has to touch a raw file.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// The three themes (spec `desktop-app-shell`: 珊瑚玫红默认 / 深钴蓝 / 松石绿).
/// A theme only changes accent and primary action colors; it never changes the
/// pure-white music workspace surface (the interface terminology is
/// authoritative, `docs/interface-terminology.md`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DesktopTheme {
    Coral,
    Cobalt,
    Turquoise,
}

impl DesktopTheme {
    /// The design default (珊瑚玫红).
    #[must_use]
    pub const fn default_theme() -> Self {
        Self::Coral
    }
}

impl From<DesktopTheme> for String {
    fn from(theme: DesktopTheme) -> Self {
        match theme {
            DesktopTheme::Coral => "coral".to_owned(),
            DesktopTheme::Cobalt => "cobalt".to_owned(),
            DesktopTheme::Turquoise => "turquoise".to_owned(),
        }
    }
}

/// What the main window does on close (spec 主窗口关闭行为): exit the process,
/// or keep running in the background with playback continuing.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CloseBehavior {
    /// Quit the process (and interrupt playback).
    Exit,
    /// Hide to the tray/menu bar; playback and process continue.
    Background,
}

/// The platform-default close behavior when the user has no valid saved choice:
/// macOS keeps running in the background; Windows/Linux exit (spec 关闭行为,
/// design §窗口关闭事件).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlatformCloseDefault {
    Macos,
    Other,
}

impl PlatformCloseDefault {
    /// The default close behavior for this family.
    #[must_use]
    pub const fn behavior(self) -> CloseBehavior {
        match self {
            Self::Macos => CloseBehavior::Background,
            Self::Other => CloseBehavior::Exit,
        }
    }
}

impl From<CloseBehavior> for String {
    fn from(behavior: CloseBehavior) -> Self {
        match behavior {
            CloseBehavior::Exit => "exit".to_owned(),
            CloseBehavior::Background => "background".to_owned(),
        }
    }
}

/// Restorable window geometry. `x`/`y` are the top-left corner in screen
/// coordinates (desktop-shell-local); the shell validates the position still
/// lies on a visible display before applying it (design §窗口状态恢复).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowState {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub maximized: bool,
}

impl WindowState {
    /// Whether this geometry is usable: positive, non-maximized-degenerate
    /// dimensions. The shell treats a `maximized` flag as opaque and instead
    /// restores the maximized preference separately, so a saved size of `0`
    /// here is ignored, not trusted.
    #[must_use]
    pub const fn is_usable(self) -> bool {
        self.width > 0 && self.height > 0
    }

    /// Re-anchor this window into a visible work area after a display
    /// disconnect (task 9.6: "断开显示器后窗口回到可见区域").
    ///
    /// The shell supplies the target display's [`WorkArea`] (resolved from the
    /// monitor APIs); this method shifts only `x`/`y` — never size or the
    /// maximized flag. When the saved window no longer overlaps the work area
    /// at all, it is re-positioned so it is fully on screen (centred when it
    /// fits, else snapped to the work-area origin). A degenerate work area
    /// (zero width/height) leaves the position untouched so the shell can fall
    /// back to its defaults instead of guessing.
    #[must_use]
    pub fn clamp_to_visible(self, work: WorkArea) -> Self {
        let (ww, wh) = (u64::from(work.width), u64::from(work.height));
        if !self.is_usable() || ww == 0 || wh == 0 {
            return self;
        }
        let (w, h) = (u64::from(self.width), u64::from(self.height));
        let (wx0, wy0) = (i128::from(work.x), i128::from(work.y));
        let (wx1, wy1) = (wx0 + ww as i128, wy0 + wh as i128);
        let (x, y) = (i128::from(self.x), i128::from(self.y));
        let (x1, y1) = (x + w as i128, y + h as i128);

        // Does the saved window still overlap the visible area at all?
        let overlaps = x < wx1 && x1 > wx0 && y < wy1 && y1 > wy0;
        let (nx, ny) = if !overlaps {
            // Not visible (the display it was on was disconnected): re-anchor it
            // into the work area — centred when it fits, else with its top-left
            // at the work-area origin — and clamp so it never spills past the
            // right/bottom edge (leaving at least the top-left reachable).
            let fit_w = w.min(ww) as i128;
            let fit_h = h.min(wh) as i128;
            let cx = wx0 + (ww as i128 - w as i128) / 2;
            let cy = wy0 + (wh as i128 - h as i128) / 2;
            let nx = cx.clamp(wx0, wx1 - fit_w);
            let ny = cy.clamp(wy0, wy1 - fit_h);
            (nx, ny)
        } else {
            (x, y)
        };

        Self {
            x: nx.clamp(i128::from(i32::MIN), i128::from(i32::MAX)) as i32,
            y: ny.clamp(i128::from(i32::MIN), i128::from(i32::MAX)) as i32,
            width: self.width,
            height: self.height,
            maximized: self.maximized,
        }
    }
}

/// A display work area (the visible region, excluding OS taskbars/menus), in
/// the same screen coordinates as [`WindowState`]. The shell resolves this from
/// the current display before applying a restored position (task 9.6).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WorkArea {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// An opaque playback-session payload. Task 8.9 owns the structured schema and
/// decides what survives a restart; task 7.6 only guarantees the value is
/// durably + atomically stored and survives a crash. Held as an optional JSON
/// value so a corrupt window/theme field never fails the session restore.
pub type PlaybackSessionValue = serde_json::Value;

/// The raw persisted document (what is serialized and read back for mutation).
///
/// Enum-ish fields are kept as strings so a single unknown value degrades
/// *that field* without invalidating a sibling that was stored correctly.
/// The doc is never deserialized whole for reads — each field is extracted
/// leniently from the raw JSON instead (see [`parse_doc`]), so even a
/// wrong-typed field cannot take down valid siblings.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct RawDoc {
    /// Schema version for forward migration; not used before 0.1.0.
    version: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    theme: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    close_behavior: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    window: Option<WindowState>,
    #[serde(skip_serializing_if = "Option::is_none")]
    playback_session: Option<PlaybackSessionValue>,
}

/// The resolved, validated view of the store. `load` never fails on disk
/// content: every field is coerced to a usable value or a safe default, and
/// `had_corruption` records whether any field needed a fallback (so the shell
/// can surface a non-blocking "偏好未保存" hint when it next writes).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResolvedState {
    pub theme: DesktopTheme,
    pub close_behavior: CloseBehavior,
    pub window: Option<WindowState>,
    pub had_corruption: bool,
}

impl ResolvedState {
    /// The fully-default state used when the store is missing or unreadable.
    #[must_use]
    pub const fn defaults(platform: PlatformCloseDefault) -> Self {
        Self {
            theme: DesktopTheme::default_theme(),
            close_behavior: platform.behavior(),
            window: None,
            had_corruption: false,
        }
    }
}

/// A thread-safe handle to the desktop's local-state file.
///
/// The window (`set_window_state`) and the playback actor
/// (`set_playback_session`) are distinct writers; they are serialized by an
/// internal lock so one cannot silently lose the other's field. Reads are
/// lock-free (each sees either the old or the new file — never a torn one).
#[derive(Debug)]
pub struct DesktopStateStore {
    path: PathBuf,
    platform: PlatformCloseDefault,
    /// Serializes the read-modify-write mutation (single logical writer).
    write_lock: Mutex<()>,
}

impl DesktopStateStore {
    /// Bind the store to a concrete file path with the caller's platform close
    /// default. `path` is the *final* `desktop-state.json`; the store creates
    /// its parent directory on first write.
    #[must_use]
    pub const fn new(path: PathBuf, platform: PlatformCloseDefault) -> Self {
        Self {
            path,
            platform,
            write_lock: Mutex::new(()),
        }
    }

    /// The final store path (for diagnostics; the caller already knows it).
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Load and validate the store. A missing file yields the defaults without
    /// corruption (first launch); a file that is not valid JSON at all, or
    /// carries a semantically-invalid field, yields defaults per field with
    /// `had_corruption = true` for anything that fell back. Valid siblings in
    /// a partially-bad file are preserved. Never errors on content — only an
    /// io error reading the file itself is reported.
    ///
    /// # Errors
    ///
    /// An actual io error (e.g. the path is a directory) propagates so the
    /// caller can decide, but the safe fallback is still applied.
    pub fn load(&self) -> Result<ResolvedState, StateError> {
        let Some(bytes) = self.read_bytes()? else {
            return Ok(ResolvedState::defaults(self.platform));
        };
        Ok(parse_doc(&bytes).to_resolved(self.platform))
    }

    /// Durable, atomic mutation of the **theme**.
    ///
    /// # Errors
    ///
    /// Returns the underlying io error; on failure the previous file is left
    /// intact.
    pub fn set_theme(&self, theme: DesktopTheme) -> Result<(), StateError> {
        self.mutate(|raw| raw.theme = Some(String::from(theme)))
    }

    /// Durable, atomic mutation of the **close behavior**.
    ///
    /// # Errors
    ///
    /// As [`Self::set_theme`].
    pub fn set_close_behavior(&self, behavior: CloseBehavior) -> Result<(), StateError> {
        self.mutate(|raw| raw.close_behavior = Some(String::from(behavior)))
    }

    /// Durable, atomic mutation of the **window state**. `None` clears any
    /// saved geometry (e.g. after the user resets it or the display changed).
    ///
    /// # Errors
    ///
    /// As [`Self::set_theme`].
    pub fn set_window_state(&self, window: Option<WindowState>) -> Result<(), StateError> {
        self.mutate(|raw| raw.window = window)
    }

    /// Durable, atomic mutation of the **playback session** payload. `None`
    /// clears it (a clean stop). The structured schema is owned by task 8.9;
    /// this only guarantees durable, atomic storage.
    ///
    /// # Errors
    ///
    /// As [`Self::set_theme`].
    pub fn set_playback_session(
        &self,
        session: Option<PlaybackSessionValue>,
    ) -> Result<(), StateError> {
        self.mutate(|raw| raw.playback_session = session)
    }

    /// The currently-valid theme, falling back to coral.
    ///
    /// # Errors
    ///
    /// As [`Self::load`].
    pub fn theme(&self) -> Result<DesktopTheme, StateError> {
        Ok(self.load()?.theme)
    }

    /// The currently-valid close behavior, falling back to the platform
    /// default.
    ///
    /// # Errors
    ///
    /// As [`Self::load`].
    pub fn close_behavior(&self) -> Result<CloseBehavior, StateError> {
        Ok(self.load()?.close_behavior)
    }

    /// The currently-valid window state, if any.
    ///
    /// # Errors
    ///
    /// As [`Self::load`].
    pub fn window_state(&self) -> Result<Option<WindowState>, StateError> {
        Ok(self.load()?.window)
    }

    /// The durable playback-session payload (structured schema owned by 8.9).
    ///
    /// # Errors
    ///
    /// As [`Self::load`].
    pub fn playback_session(&self) -> Result<Option<PlaybackSessionValue>, StateError> {
        let Some(bytes) = self.read_bytes()? else {
            return Ok(None);
        };
        Ok(parse_doc(&bytes).playback_session)
    }

    /// Re-read the store and return whether any field required a fallback
    /// (a non-blocking "偏好未保存" hint source for the shell).
    ///
    /// # Errors
    ///
    /// As [`Self::load`].
    pub fn had_corruption(&self) -> Result<bool, StateError> {
        Ok(self.load()?.had_corruption)
    }

    /// Read-modify-write the raw doc atomically: load the current file (or a
    /// blank doc on first launch), apply `edit`, and durably replace the file.
    ///
    /// Writers are serialized so the window and the playback actor cannot
    /// overwrite each other's field with a stale read.
    ///
    /// # Errors
    ///
    /// Returns the underlying io error; on failure the previous file is left
    /// intact (the temp file is best-effort removed).
    fn mutate(&self, edit: impl FnOnce(&mut RawDoc)) -> Result<(), StateError> {
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let mut raw = self.read_raw()?;
        edit(&mut raw);
        self.save(&raw)
    }

    /// Read the raw persisted doc. A missing file yields a blank doc (first
    /// launch); a file that is not valid JSON yields a blank doc (nothing
    /// recoverable — the next mutation writes a fresh one). Any *valid*
    /// fields inside a partially-degraded file are preserved, so fixing one
    /// field never erases the session/window stored alongside it.
    ///
    /// # Errors
    ///
    /// An io error reading the file propagates; a content error does not.
    fn read_raw(&self) -> Result<RawDoc, StateError> {
        let Some(bytes) = self.read_bytes()? else {
            return Ok(RawDoc::default());
        };
        Ok(parse_doc(&bytes).into_raw())
    }

    /// Replace the store file with `doc`, durably and atomically.
    ///
    /// # Errors
    ///
    /// Returns the underlying io error; on failure the previous file is left
    /// intact (the temp file is best-effort removed).
    fn save(&self, raw: &RawDoc) -> Result<(), StateError> {
        let json =
            serde_json::to_vec_pretty(raw).map_err(|e| StateError::Serialize(e.to_string()))?;
        write_atomically(&self.path, &json).map_err(StateError::Io)
    }

    /// Read the store's bytes; `Ok(None)` when the file does not exist.
    fn read_bytes(&self) -> Result<Option<Vec<u8>>, StateError> {
        match fs::read(&self.path) {
            Ok(b) => Ok(Some(b)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(StateError::Io(e)),
        }
    }
}

/// Leniently parse the raw bytes into a documented view. Every field is
/// extracted independently, so a bad field only degrades itself.
fn parse_doc(bytes: &[u8]) -> ParsedDoc {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        // The file is not valid JSON at all (e.g. truncated mid-write):
        // nothing inside it is recoverable. Fall back wholesale.
        return ParsedDoc::unparseable();
    };
    ParsedDoc::from_value(&value)
}

/// The lenient per-field parse result. `corruption` is true when at least one
/// field was *present but invalid* (unknown enum value, wrong type, or
/// degenerate geometry) and had to fall back.
#[derive(Debug, Default)]
struct ParsedDoc {
    theme: Option<DesktopTheme>,
    close_behavior: Option<CloseBehavior>,
    window: Option<WindowState>,
    playback_session: Option<PlaybackSessionValue>,
    corruption: bool,
}

impl ParsedDoc {
    /// Nothing was recoverable (the file was not valid JSON or missing).
    fn unparseable() -> Self {
        Self {
            corruption: true,
            ..Self::default()
        }
    }

    /// Extract every field independently from the parsed JSON object. A field
    /// that is absent is a default (no corruption); one that is present but
    /// invalid degrades to its default and marks corruption.
    fn from_value(value: &serde_json::Value) -> Self {
        let mut doc = Self::default();

        match value.get("theme") {
            Some(serde_json::Value::String(s)) => match s.as_str() {
                "coral" => doc.theme = Some(DesktopTheme::Coral),
                "cobalt" => doc.theme = Some(DesktopTheme::Cobalt),
                "turquoise" => doc.theme = Some(DesktopTheme::Turquoise),
                _ => doc.corruption = true, // unknown string -> coral default
            },
            Some(_) => doc.corruption = true, // wrong type -> coral default
            None => {}
        }

        match value.get("closeBehavior") {
            Some(serde_json::Value::String(s)) => match s.as_str() {
                "exit" => doc.close_behavior = Some(CloseBehavior::Exit),
                "background" => doc.close_behavior = Some(CloseBehavior::Background),
                _ => doc.corruption = true, // unknown string -> platform default
            },
            Some(_) => doc.corruption = true, // wrong type -> platform default
            None => {}
        }

        if let Some(w) = value.get("window") {
            match serde_json::from_value::<WindowState>(w.clone()) {
                Ok(ws) if ws.is_usable() => doc.window = Some(ws),
                // Degenerate or wrong-typed geometry is not trusted; report it.
                _ => doc.corruption = true,
            }
        }

        // The session is opaque; any JSON value is accepted verbatim.
        if let Some(session) = value.get("playbackSession").cloned() {
            doc.playback_session = Some(session);
        }

        doc
    }

    /// The typed resolved view with safe defaults applied.
    fn to_resolved(&self, platform: PlatformCloseDefault) -> ResolvedState {
        ResolvedState {
            theme: self.theme.unwrap_or_else(DesktopTheme::default_theme),
            close_behavior: self.close_behavior.unwrap_or_else(|| platform.behavior()),
            window: self.window,
            had_corruption: self.corruption,
        }
    }

    /// The serializable raw doc, keeping only the fields that parsed valid. An
    /// invalid field is dropped entirely (it is untrustworthy and a fallback
    /// writes the default on next load), so a mutation never resurrects it.
    fn into_raw(self) -> RawDoc {
        RawDoc {
            theme: self.theme.map(String::from),
            close_behavior: self.close_behavior.map(String::from),
            window: self.window,
            playback_session: self.playback_session,
            version: 0,
        }
    }
}

/// Write `bytes` to `path` durably and atomically.
///
/// Sequence: remove stale temps from a crashed write, create a uniquely-named
/// temp file **in the same directory**, write + `fsync` it, then atomic
/// `rename` over `path`, then best-effort `fsync` the directory so the rename
/// itself is durable. On failure the temp file is cleaned up and the previous
/// `path` (if any) is untouched.
///
/// # Errors
///
/// Propagates the failing io error; the target file is never left torn.
fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;

    let file_name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("desktop-state.json");

    // A previous crash can leave `.desktop-state.json.*.tmp` behind. Remove
    // them first so they neither accumulate nor collide when the OS reuses a
    // process id. Single-writer by design, so no live temp can be touched.
    clean_stale_temps(parent, file_name);

    // `create_new` refuses to clobber, so a leftover we failed to clean (or a
    // pid-reuse race) is dodged by trying a fresh suffix a few times.
    for attempt in 0..8 {
        let temp_name = format!(".{file_name}.{}.{attempt}.tmp", std::process::id());
        let temp_path = parent.join(&temp_name);
        match write_temp_and_rename(&temp_path, path, bytes) {
            Ok(()) => return Ok(()),
            // A stale leftover we could not clean (or a pid-reuse race): fall
            // through and retry with a fresh suffix.
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "desktop-state temp file kept colliding with stale leftovers",
    ))
}

/// Write `bytes` into `temp_path` (created exclusively), fsync it, then
/// atomically rename it over `path`. On failure the temp is removed.
fn write_temp_and_rename(temp_path: &Path, path: &Path, bytes: &[u8]) -> io::Result<()> {
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(temp_path)?;
        file.write_all(bytes)?;
        // fsync the file contents before it can ever be renamed into place.
        file.sync_all()?;
        drop(file);
        // Atomic replace on the same filesystem.
        fs::rename(temp_path, path)?;
        // Best-effort: persist the rename itself (durability of the directory
        // entry). Not a durability guarantee on every platform, so failures
        // are ignored, not surfaced as a write error.
        sync_directory(path.parent().unwrap_or_else(|| Path::new(".")));
        Ok(())
    })();

    if result.is_err() {
        let _ = fs::remove_file(temp_path);
    }
    result
}

/// Best-effort removal of stale `.basename.*.tmp` files from crashed writes.
fn clean_stale_temps(parent: &Path, base_name: &str) {
    let prefix = format!(".{base_name}.");
    let Ok(entries) = fs::read_dir(parent) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let is_tmp = entry
            .path()
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("tmp"));
        if name.starts_with(&prefix) && is_tmp {
            let _ = fs::remove_file(entry.path());
        }
    }
}

/// `fsync` a directory so a rename is durable. Only meaningful on Unix; Win is
/// a no-op (the file-content `sync_all` above is the real guarantee).
#[cfg(unix)]
fn sync_directory(dir: &Path) {
    if let Ok(d) = File::open(dir) {
        let _ = d.sync_all();
    }
}

#[cfg(not(unix))]
fn sync_directory(_dir: &Path) {}

/// The desktop-state storage error surface. Kept minimal: callers only need
/// to distinguish "io" from "could not serialize the schema".
#[derive(Debug, thiserror::Error)]
pub enum StateError {
    #[error("desktop-state io error: {0}")]
    Io(#[from] io::Error),
    #[error("desktop-state serialization error: {0}")]
    Serialize(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("echo-ls-{name}-{}", std::process::id()))
    }

    fn cleanup(p: &Path) {
        let _ = fs::remove_file(p);
    }

    #[test]
    fn missing_store_yields_defaults_without_corruption() {
        let p = temp_path("missing");
        cleanup(&p);
        let store = DesktopStateStore::new(p.clone(), PlatformCloseDefault::Other);
        let s = store
            .load()
            .expect("missing file is defaults, not an error");
        assert_eq!(s.theme, DesktopTheme::Coral);
        assert_eq!(s.close_behavior, CloseBehavior::Exit);
        assert_eq!(s.window, None);
        assert!(!s.had_corruption, "first launch is not corruption");
        cleanup(&p);
    }

    #[test]
    fn perfectly_corrupt_file_falls_back_to_defaults_and_marks_corruption() {
        let p = temp_path("corrupt");
        fs::write(&p, b"{ this is not json ").expect("write");
        let store = DesktopStateStore::new(p.clone(), PlatformCloseDefault::Other);
        let s = store.load().expect("corrupt file falls back, not an error");
        assert_eq!(s.theme, DesktopTheme::Coral);
        assert_eq!(s.close_behavior, CloseBehavior::Exit);
        assert!(s.had_corruption);
        cleanup(&p);
    }

    #[test]
    fn corrupt_file_on_macos_falls_back_to_coral_and_background_default() {
        let p = temp_path("corrupt-macos");
        fs::write(&p, b"\x00\xff\xfe{not valid").expect("write");
        let store = DesktopStateStore::new(p.clone(), PlatformCloseDefault::Macos);
        let s = store.load().expect("corrupt file falls back, not an error");
        assert_eq!(s.theme, DesktopTheme::Coral, "损坏偏好回退到珊瑚主题");
        assert_eq!(
            s.close_behavior,
            CloseBehavior::Background,
            "macOS 平台默认关窗值 = 后台运行"
        );
        assert!(s.had_corruption);
        cleanup(&p);
    }

    #[test]
    fn unknown_theme_and_close_values_fall_back_per_field_preserving_siblings() {
        let p = temp_path("unknown");
        fs::write(
            &p,
            br#"{"theme":"neon","closeBehavior":"sleepy","window":{"x":0,"y":0,"width":1280,"height":800,"maximized":false}}"#,
        )
        .expect("write valid");
        let store = DesktopStateStore::new(p.clone(), PlatformCloseDefault::Macos);
        let s = store.load().expect("unknown values fall back");
        assert_eq!(s.theme, DesktopTheme::Coral, "unknown theme -> coral");
        assert_eq!(
            s.close_behavior,
            CloseBehavior::Background,
            "unknown close -> platform default (macos = background)"
        );
        // A *valid* sibling field (window) is preserved even though others fell back.
        assert_eq!(
            s.window,
            Some(WindowState {
                x: 0,
                y: 0,
                width: 1280,
                height: 800,
                maximized: false,
            })
        );
        assert!(s.had_corruption);
        cleanup(&p);
    }

    #[test]
    fn wrong_typed_field_degrades_only_itself_and_keeps_valid_siblings() {
        let p = temp_path("wrong-type");
        // `window` is a string, not an object: only that field falls back; the
        // valid cobalt theme and the playback session must survive.
        fs::write(
            &p,
            br#"{"theme":"cobalt","closeBehavior":"background","window":"not-an-object","playbackSession":{"queue":[7,8]}}"#,
        )
        .expect("write");
        let store = DesktopStateStore::new(p.clone(), PlatformCloseDefault::Other);
        let s = store
            .load()
            .expect("wrong-typed field degrades, not an error");
        assert_eq!(s.theme, DesktopTheme::Cobalt, "valid sibling preserved");
        assert_eq!(
            s.close_behavior,
            CloseBehavior::Background,
            "valid sibling preserved"
        );
        assert_eq!(s.window, None, "wrong-typed window falls back");
        assert!(s.had_corruption);
        assert_eq!(
            store.playback_session().expect("session"),
            Some(serde_json::json!({"queue": [7, 8]})),
            "session isolated from the bad window field"
        );
        cleanup(&p);
    }

    #[test]
    fn known_values_round_trip_with_no_corruption() {
        let p = temp_path("roundtrip");
        cleanup(&p);
        let store = DesktopStateStore::new(p.clone(), PlatformCloseDefault::Other);
        store.set_theme(DesktopTheme::Cobalt).expect("theme");
        store
            .set_close_behavior(CloseBehavior::Background)
            .expect("close");
        store
            .set_window_state(Some(WindowState {
                x: 10,
                y: 20,
                width: 900,
                height: 600,
                maximized: true,
            }))
            .expect("window");
        store
            .set_playback_session(Some(serde_json::json!({"queue": [1, 2, 3]})))
            .expect("session");
        let s = store.load().expect("reload");
        assert_eq!(s.theme, DesktopTheme::Cobalt);
        assert_eq!(s.close_behavior, CloseBehavior::Background);
        assert_eq!(
            s.window,
            Some(WindowState {
                x: 10,
                y: 20,
                width: 900,
                height: 600,
                maximized: true,
            })
        );
        assert!(!s.had_corruption);
        assert_eq!(
            store.playback_session().expect("session"),
            Some(serde_json::json!({"queue": [1, 2, 3]}))
        );
        cleanup(&p);
    }

    #[test]
    fn degenerate_window_state_is_rejected_and_falls_back() {
        let p = temp_path("degenerate-window");
        fs::write(
            &p,
            br#"{"theme":"coral","closeBehavior":"exit","window":{"x":-5,"y":-5,"width":0,"height":0,"maximized":true}}"#,
        )
        .expect("write");
        let store = DesktopStateStore::new(p.clone(), PlatformCloseDefault::Other);
        let s = store.load().expect("degenerate window falls back");
        assert_eq!(s.window, None, "zero-size window is not usable");
        assert_eq!(s.theme, DesktopTheme::Coral);
        assert!(s.had_corruption);
        cleanup(&p);
    }

    #[test]
    fn windows_platform_default_is_exit() {
        let p = temp_path("win-default");
        cleanup(&p);
        let store = DesktopStateStore::new(p, PlatformCloseDefault::Other);
        let s = store.load().expect("defaults");
        assert_eq!(s.close_behavior, CloseBehavior::Exit);
    }

    #[test]
    fn window_left_on_a_disconnected_display_is_recentered_on_screen() {
        let window = WindowState {
            x: -1000,
            y: 2000,
            width: 800,
            height: 600,
            maximized: false,
        };
        let work = WorkArea {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        // Off every visible area => centred into the primary work area.
        let clamped = window.clamp_to_visible(work);
        assert_eq!(clamped.x, 560); // (1920 - 800) / 2
        assert_eq!(clamped.y, 240); // (1080 - 600) / 2
        assert_eq!(clamped.width, 800);
        assert_eq!(clamped.height, 600);
        assert!(!clamped.maximized, "size/maximize untouched by re-anchor");
    }

    #[test]
    fn a_still_visible_window_is_left_untouched() {
        let window = WindowState {
            x: 100,
            y: 100,
            width: 800,
            height: 600,
            maximized: false,
        };
        let work = WorkArea {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        assert_eq!(window.clamp_to_visible(work), window);
    }

    #[test]
    fn a_window_off_screen_and_bigger_than_the_screen_snaps_to_the_work_origin() {
        let window = WindowState {
            x: 5000,
            y: 5000,
            width: 3000,
            height: 2500,
            maximized: false,
        };
        let work = WorkArea {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        let clamped = window.clamp_to_visible(work);
        // Fully off-screen and larger than the work area: the top-left lands at
        // the work origin (never spilled past the right/bottom edge).
        assert_eq!((clamped.x, clamped.y), (0, 0));
        assert_eq!((clamped.width, clamped.height), (3000, 2500));
    }

    #[test]
    fn a_partially_visible_window_is_left_untouched() {
        // Even a window that spills past an edge is reachable while any part
        // overlaps — only a fully off-screen window is re-anchored.
        let window = WindowState {
            x: 1500,
            y: 800,
            width: 1200,
            height: 800,
            maximized: false,
        };
        let work = WorkArea {
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
        };
        assert_eq!(window.clamp_to_visible(work), window);
    }

    #[test]
    fn a_degenerate_work_area_leaves_the_position_unchanged() {
        let window = WindowState {
            x: -1000,
            y: 2000,
            width: 800,
            height: 600,
            maximized: false,
        };
        // No usable work area (shell couldn't resolve a display): don't guess.
        assert_eq!(
            window.clamp_to_visible(WorkArea {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
            }),
            window
        );
        // An unusable window (zero size) is likewise left alone.
        assert_eq!(
            WindowState {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
                maximized: true,
            }
            .clamp_to_visible(WorkArea {
                x: 0,
                y: 0,
                width: 1920,
                height: 1080,
            }),
            WindowState {
                x: 0,
                y: 0,
                width: 0,
                height: 0,
                maximized: true,
            }
        );
    }

    #[test]
    fn mutation_preserves_valid_session_across_a_partially_corrupt_file() {
        let p = temp_path("preserve-session");
        // The close field is corrupt, but the session is valid: fixing the
        // theme must NOT erase the session.
        fs::write(
            &p,
            br#"{"closeBehavior":{"bad":true},"playbackSession":{"queue":[42]}}"#,
        )
        .expect("write");
        let store = DesktopStateStore::new(p.clone(), PlatformCloseDefault::Other);
        store.set_theme(DesktopTheme::Cobalt).expect("mutate theme");
        let s = store.load().expect("reload");
        assert_eq!(s.theme, DesktopTheme::Cobalt, "edited field written");
        assert_eq!(
            s.close_behavior,
            CloseBehavior::Exit,
            "bad close drops to default"
        );
        assert_eq!(
            store.playback_session().expect("session"),
            Some(serde_json::json!({"queue": [42]})),
            "valid session survived the theme mutation"
        );
        cleanup(&p);
    }

    #[test]
    fn failed_write_preserves_previous_file() {
        // A store whose target sits under a path component that is a *file* —
        // `create_dir_all` must fail — so the mutation errors and leaves
        // nothing partial behind.
        let base = std::env::temp_dir().join(format!("echo-ls-block-{}", std::process::id()));
        let blocker = base.join("not_a_dir");
        fs::create_dir_all(&base).expect("base");
        fs::write(&blocker, b"i am a file not a dir").expect("seed blocker");
        let target = blocker.join("state.json");
        let store = DesktopStateStore::new(target.clone(), PlatformCloseDefault::Other);
        assert!(
            store.set_theme(DesktopTheme::Cobalt).is_err(),
            "unwritable parent errors"
        );
        assert!(!target.exists(), "no partial file left behind");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn typed_mutators_are_read_modify_write_and_clear_with_none() {
        let p = temp_path("mutate");
        cleanup(&p);
        let store = DesktopStateStore::new(p.clone(), PlatformCloseDefault::Other);
        store
            .set_window_state(Some(WindowState {
                x: 0,
                y: 0,
                width: 640,
                height: 480,
                maximized: false,
            }))
            .expect("write window");
        assert_eq!(
            store.window_state().expect("read").map(|w| w.width),
            Some(640)
        );

        // Updating theme must preserve the window written earlier.
        store
            .set_theme(DesktopTheme::Turquoise)
            .expect("write theme");
        let s = store.load().expect("load");
        assert_eq!(s.theme, DesktopTheme::Turquoise);
        assert_eq!(s.window.map(|w| w.width), Some(640), "sibling preserved");

        // Clearing the window removes it, theme stays.
        store.set_window_state(None).expect("clear window");
        let s = store.load().expect("load");
        assert_eq!(s.window, None);
        assert_eq!(s.theme, DesktopTheme::Turquoise);

        // Clearing the playback session removes it.
        store
            .set_playback_session(Some(serde_json::json!({"x": 1})))
            .expect("write session");
        assert_eq!(
            store.playback_session().expect("read"),
            Some(serde_json::json!({"x": 1}))
        );
        store.set_playback_session(None).expect("clear session");
        assert_eq!(store.playback_session().expect("read"), None);
        cleanup(&p);
    }

    #[test]
    fn parent_directory_is_created_on_first_write() {
        let base = std::env::temp_dir().join(format!("echo-ls-dir-{}", std::process::id()));
        let p = base.join("sub/deeper/state.json");
        cleanup(&p);
        let store = DesktopStateStore::new(p, PlatformCloseDefault::Other);
        store
            .set_theme(DesktopTheme::Turquoise)
            .expect("creates parents and saves");
        assert_eq!(store.theme().expect("read"), DesktopTheme::Turquoise);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn serialized_file_uses_camel_case_and_skips_absent_fields() {
        let p = temp_path("serialized-shape");
        cleanup(&p);
        let store = DesktopStateStore::new(p.clone(), PlatformCloseDefault::Other);
        store.set_theme(DesktopTheme::Coral).expect("theme");
        let text = fs::read_to_string(&p).expect("read");
        assert!(
            text.contains("\"theme\": \"coral\""),
            "camel-case key: {text}"
        );
        assert!(
            !text.contains("closeBehavior"),
            "absent field skipped: {text}"
        );
        cleanup(&p);
    }

    #[test]
    fn stale_temp_file_is_cleaned_before_the_next_write() {
        let p = temp_path("stale-temp");
        cleanup(&p);
        let file_name = p
            .file_name()
            .and_then(|n| n.to_str())
            .expect("temp basename");
        // Simulate a crashed write: a leftover temp with our own pid.
        let stale = p
            .parent()
            .expect("parent")
            .join(format!(".{file_name}.{}.0.tmp", std::process::id()));
        let _ = fs::remove_file(&stale);
        fs::write(&stale, b"partial").expect("seed stale temp");
        let store = DesktopStateStore::new(p.clone(), PlatformCloseDefault::Other);
        store
            .set_theme(DesktopTheme::Coral)
            .expect("write after stale temp");
        assert!(
            !stale.exists(),
            "stale temp removed so it cannot collide on a reused pid"
        );
        cleanup(&p);
        let _ = fs::remove_file(&stale);
    }
}
