//! Pure geometry for the notch player window.
//!
//! Everything here is expressed in **`AppKit` points** with the origin at the
//! screen's bottom-left corner (the coordinate space `NSScreen` reports). The
//! caller multiplies by the screen's `backingScaleFactor` when it hands a rect
//! to Tauri, which expects physical pixels — keeping the conversion at the FFI
//! boundary means the arithmetic below stays testable without a display.
//!
//! Two invariants drive every function here:
//!   1. the window's **top edge is flush with the screen's top edge**, so the
//!      collapsed bar reads as an extension of the hardware notch;
//!   2. the window is **horizontally centred on the notch**, not on the screen,
//!      because the notch is what the collapsed bar is aligned to.

/// One ear beside the housing: exactly wide enough for the 16 px cover plus its
/// 5 px padding. The collapsed width is *derived* from the housing width rather
/// than hardcoded, because the housing differs between laptop sizes — a fixed
/// width would push the cover under the bezel on a wider notch.
pub const EAR_WIDTH: f64 = 26.0;
/// Collapsed height matches the menu-bar band the housing sits in (measured at
/// 32 pt on a 14"/16" `MacBook Pro`).
pub const COLLAPSED_HEIGHT: f64 = 32.0;
/// Expanded: the 360 px panel body plus the two 26 px outward fillets.
pub const EXPANDED_WIDTH: f64 = 412.0;
pub const EXPANDED_HEIGHT: f64 = 196.0;

/// The collapsed bar's width for a given housing: housing + one ear per side.
#[must_use]
pub fn collapsed_width(notch: NotchRect) -> f64 {
    2.0f64.mul_add(EAR_WIDTH, notch.width)
}

/// A screen's frame in `AppKit` points (origin bottom-left).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenFrame {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl ScreenFrame {
    /// The screen's top edge in the `AppKit` y axis (larger y is higher).
    #[must_use]
    pub fn top(&self) -> f64 {
        self.y + self.height
    }
}

/// The camera housing's horizontal extent in the same coordinate space.
///
/// Only `x` and `width` matter: the notch always starts at the screen's top
/// edge, and the window's top edge is pinned there independently.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NotchRect {
    pub x: f64,
    pub width: f64,
}

impl NotchRect {
    #[must_use]
    pub fn center_x(&self) -> f64 {
        self.x + self.width / 2.0
    }
}

/// The window rect to apply, in `AppKit` points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowGeometry {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Compute the window rect for a state.
///
/// The result is clamped horizontally so a notch close to a screen edge can
/// never push the window off that screen — the notch is centred on real
/// hardware, but external displays and future form factors are not guaranteed
/// to be, and a half-offscreen bar would be unreachable.
#[must_use]
pub fn window_geometry(screen: ScreenFrame, notch: NotchRect, expanded: bool) -> WindowGeometry {
    let (width, height) = if expanded {
        (EXPANDED_WIDTH, EXPANDED_HEIGHT)
    } else {
        (collapsed_width(notch), COLLAPSED_HEIGHT)
    };

    let desired_x = notch.center_x() - width / 2.0;
    let max_x = screen.x + (screen.width - width).max(0.0);
    let x = desired_x.clamp(screen.x, max_x);

    WindowGeometry {
        x,
        y: screen.top() - height,
        width,
        height,
    }
}

#[cfg(test)]
// Every value here is an integer or an exact binary fraction (points and
// 26 pt ears), so exact comparison is the intended assertion, not a tolerance.
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    fn screen() -> ScreenFrame {
        // A 14" `MacBook Pro`: 1512 × 982 points.
        ScreenFrame {
            x: 0.0,
            y: 0.0,
            width: 1512.0,
            height: 982.0,
        }
    }

    fn centered_notch(width: f64) -> NotchRect {
        NotchRect {
            x: (1512.0 - width) / 2.0,
            width,
        }
    }

    #[test]
    fn collapsed_window_is_centred_on_the_notch_and_flush_with_the_top() {
        let notch = centered_notch(185.0);
        let g = window_geometry(screen(), notch, false);
        assert_eq!(
            g.width,
            collapsed_width(notch),
            "housing plus one ear per side"
        );
        assert_eq!(g.height, COLLAPSED_HEIGHT);
        assert_eq!(g.y + g.height, 982.0, "top edge is flush with the screen");
        assert_eq!(
            g.x + g.width / 2.0,
            756.0,
            "centred on the notch centre, not the screen centre by accident"
        );
    }

    #[test]
    fn a_wider_housing_widens_the_collapsed_bar_so_the_cover_stays_clear() {
        // A 14" housing is ~185 pt; a wider one must not push the ear content
        // under the bezel, so the bar grows with the housing instead of staying
        // at a fixed width.
        let narrow = window_geometry(screen(), centered_notch(185.0), false);
        let wide = window_geometry(screen(), centered_notch(205.0), false);
        assert_eq!(wide.width - narrow.width, 20.0);
        // The ear content (5 px padding + 16 px cover) never reaches the bezel.
        let bezel_edge = (wide.width - 205.0) / 2.0;
        assert!(
            5.0 + 16.0 <= bezel_edge,
            "cover must sit outside the housing"
        );
    }

    #[test]
    fn expanded_window_keeps_the_same_centre() {
        let notch = centered_notch(185.0);
        let collapsed = window_geometry(screen(), notch, false);
        let expanded = window_geometry(screen(), notch, true);
        assert_eq!(expanded.width, EXPANDED_WIDTH);
        assert_eq!(expanded.height, EXPANDED_HEIGHT);
        assert_eq!(expanded.y + expanded.height, 982.0);
        assert_eq!(
            expanded.x + expanded.width / 2.0,
            collapsed.x + collapsed.width / 2.0,
            "expanding must not shift the bar sideways"
        );
    }

    #[test]
    fn off_centre_notch_is_followed_not_the_screen_centre() {
        // A notch that is not in the middle (hypothetical form factor): the
        // window follows the hardware, not the screen.
        let notch = NotchRect {
            x: 300.0,
            width: 180.0,
        };
        let g = window_geometry(screen(), notch, false);
        assert_eq!(g.x + g.width / 2.0, 390.0);
    }

    #[test]
    fn a_notch_near_the_edge_never_pushes_the_window_offscreen() {
        let notch = NotchRect {
            x: 0.0,
            width: 100.0,
        };
        let g = window_geometry(screen(), notch, true);
        assert_eq!(g.x, 0.0, "clamped to the screen's left edge");
        assert!(g.x + g.width <= screen().width);
    }

    #[test]
    fn screen_origin_is_honoured_for_secondary_displays() {
        // A second display placed above/left of the main one has a non-zero
        // origin; the top edge must follow that display's frame.
        let screen = ScreenFrame {
            x: -1920.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        };
        let notch = NotchRect {
            x: -960.0 - 93.0,
            width: 186.0,
        };
        let g = window_geometry(screen, notch, false);
        assert_eq!(g.y + g.height, 1080.0);
        assert_eq!(g.x + g.width / 2.0, -960.0);
    }

    #[test]
    fn a_screen_narrower_than_the_window_still_yields_a_usable_rect() {
        let screen = ScreenFrame {
            x: 0.0,
            y: 0.0,
            width: 200.0,
            height: 600.0,
        };
        let notch = NotchRect { x: 0.0, width: 0.0 };
        let g = window_geometry(screen, notch, true);
        assert_eq!(
            g.x, 0.0,
            "clamp collapses to the screen origin, not a negative x"
        );
        assert_eq!(g.width, EXPANDED_WIDTH);
    }
}
