//! The `NSScreen` FFI island for notch detection.
//!
//! `auxiliaryTopLeftArea` / `auxiliaryTopRightArea` (macOS 12+, the project's
//! minimum) report the *usable* menu-bar segments on either side of the camera
//! housing. Their gap is the housing itself: that is the only supported way to
//! get its width, which is what the collapsed bar is aligned to.
//!
//! A display without a housing returns empty rects for both, so "notch present"
//! is simply "both sides are non-degenerate".

// This module is the second audited `AppKit` FFI island (the first is
// `macos_status_row.rs`). It only reads screen geometry.
#![allow(unsafe_code)]

use objc2_app_kit::NSScreen;
use objc2_foundation::MainThreadMarker;

use super::geometry::{NotchRect, ScreenFrame};

/// A detected camera housing plus the display it belongs to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DetectedNotch {
    pub screen: ScreenFrame,
    pub notch: NotchRect,
    /// The display's `backingScaleFactor`; reported so callers can log physical
    /// sizes without re-querying `AppKit`.
    pub scale: f64,
}

/// Find the first display with a camera housing.
///
/// Returns `None` when no connected display has one — an external-only setup, a
/// closed lid, or a Mac without a notch. Must run on the main thread: `NSScreen`
/// is not thread safe and `MainThreadMarker::new()` enforces it.
#[must_use]
pub fn detect() -> Option<DetectedNotch> {
    let mtm = MainThreadMarker::new()?;
    let screens = NSScreen::screens(mtm);
    for index in 0..screens.count() {
        let screen = screens.objectAtIndex(index);
        // Both selectors are documented on `NSScreen` since macOS 12 (the
        // project's minimum) and need no unsafe.
        let (left, right) = (
            screen.auxiliaryTopLeftArea(),
            screen.auxiliaryTopRightArea(),
        );
        if left.size.width <= 0.0 || right.size.width <= 0.0 {
            continue;
        }
        // The housing spans the gap between the two usable segments.
        let notch_x = left.origin.x + left.size.width;
        let notch_width = right.origin.x - notch_x;
        if notch_width <= 0.0 {
            continue;
        }
        let frame = screen.frame();
        return Some(DetectedNotch {
            screen: ScreenFrame {
                x: frame.origin.x,
                y: frame.origin.y,
                width: frame.size.width,
                height: frame.size.height,
            },
            notch: NotchRect {
                x: notch_x,
                width: notch_width,
            },
            scale: screen.backingScaleFactor(),
        });
    }
    None
}
