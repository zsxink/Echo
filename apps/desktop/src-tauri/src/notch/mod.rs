//! macOS notch player window: availability detection, geometry and lifecycle.
//!
//! The window is a *companion* of the main window in the same process: it never
//! owns a player, queue or session of its own, and it exists only on Macs whose
//! connected display has a camera housing (see
//! `openspec/specs/macos-notch-player`).
//!
//! Split by responsibility:
//!   · `geometry` — pure arithmetic in `AppKit` points, unit-tested everywhere.
//!   · `detect` — the `NSScreen` FFI island (macOS only).
//!   · `window` — Tauri window creation, `AppKit` post-processing and resizing.
//!
//! Only the geometry is compiled on non-macOS targets, so the platform gate is
//! structural: there is no code path that can create this window elsewhere.
//! [`NotchPlayer`] is the runtime state the shell and the IPC commands share.

pub mod geometry;

#[cfg(target_os = "macos")]
pub mod detect;
#[cfg(target_os = "macos")]
pub mod window;

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::AppHandle;

/// Shared, lock-free state for the companion window.
///
/// Three independent facts, deliberately not collapsed into one enum: the
/// hardware can disappear while the preference stays on (closed lid), and the
/// preference can be switched off while the hardware is still there.
#[derive(Debug, Default)]
pub struct NotchPlayer {
    available: AtomicBool,
    enabled: AtomicBool,
    expanded: AtomicBool,
}

impl NotchPlayer {
    /// `enabled` starts from the persisted preference; `available` is filled in
    /// by [`Self::sync`] once the shell is running.
    #[must_use]
    pub const fn new(enabled: bool) -> Self {
        Self {
            available: AtomicBool::new(false),
            enabled: AtomicBool::new(enabled),
            expanded: AtomicBool::new(false),
        }
    }

    #[must_use]
    pub fn available(&self) -> bool {
        self.available.load(Ordering::Relaxed)
    }

    #[must_use]
    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    pub fn set_enabled(&self, enabled: bool) {
        self.enabled.store(enabled, Ordering::Relaxed);
    }

    #[must_use]
    pub fn expanded(&self) -> bool {
        self.expanded.load(Ordering::Relaxed)
    }

    pub fn set_expanded(&self, expanded: bool) {
        self.expanded.store(expanded, Ordering::Relaxed);
    }

    /// Record the current hardware verdict and reconcile the window with it.
    ///
    /// Returns the (possibly updated) availability so callers can answer an IPC
    /// request with one source of truth.
    pub fn sync(&self, app: &AppHandle) -> bool {
        let available = Self::detect_available();
        self.available.store(available, Ordering::Relaxed);

        if !available || !self.enabled() {
            self.set_expanded(false);
            remove_window(app);
            return available;
        }

        if let Err(error) = ensure_window(app, self.expanded()) {
            // A display that stopped reporting a housing between detection and
            // creation is not an error worth surfacing: drop availability and
            // let the settings row explain the disabled state.
            tracing::warn!(%error, "notch player window is unavailable");
            self.available.store(false, Ordering::Relaxed);
        }
        // Report the final verdict, not the pre-failure one: a caller that
        // answers an IPC request from this value must not claim the window is
        // available when creating it just failed.
        self.available()
    }

    /// Re-evaluate the display after a screen-parameter change.
    pub fn refresh_screen(&self, app: &AppHandle) -> bool {
        self.sync(app)
    }

    #[cfg(target_os = "macos")]
    fn detect_available() -> bool {
        detect::detect().is_some()
    }

    #[cfg(not(target_os = "macos"))]
    fn detect_available() -> bool {
        false
    }
}

/// The housing width in logical points, when a display reports one. The
/// renderer draws the bezel at this width, so it must be the measured value and
/// not a stylesheet constant.
#[must_use]
pub fn notch_width() -> Option<f64> {
    #[cfg(target_os = "macos")]
    {
        detect::detect().map(|detected| detected.notch.width)
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

#[cfg(target_os = "macos")]
fn ensure_window(app: &AppHandle, expanded: bool) -> Result<(), String> {
    window::ensure(app, expanded)
}

#[cfg(not(target_os = "macos"))]
fn ensure_window(_app: &AppHandle, _expanded: bool) -> Result<(), String> {
    Ok(())
}

#[cfg(target_os = "macos")]
fn remove_window(app: &AppHandle) {
    window::remove(app);
}

#[cfg(not(target_os = "macos"))]
fn remove_window(_app: &AppHandle) {}

/// Resize an existing window. Returns an error when the window is not open, so
/// a stale IPC call cannot silently succeed.
pub fn set_expanded(app: &AppHandle, expanded: bool) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        window::set_expanded(app, expanded)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, expanded);
        Err("the notch player is macOS only".to_owned())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_follow_the_persisted_preference() {
        let on = NotchPlayer::new(true);
        assert!(on.enabled());
        assert!(
            !on.available(),
            "availability is a runtime fact, not a default"
        );
        assert!(!on.expanded());

        let off = NotchPlayer::new(false);
        assert!(!off.enabled());
    }

    #[test]
    fn state_fields_are_independent() {
        let player = NotchPlayer::new(true);
        player.set_enabled(false);
        player.set_expanded(true);
        assert!(!player.enabled(), "preference is independent of expansion");
        assert!(player.expanded(), "expansion survives a preference toggle");
        assert!(!player.available());
    }
}
