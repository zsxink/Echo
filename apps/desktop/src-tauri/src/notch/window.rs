//! The notch player's companion window: creation, `AppKit` post-processing and
//! resizing between the collapsed and expanded rects.
//!
//! The window is deliberately *not* declared in `tauri.conf.json`: it must only
//! exist on Macs with a camera housing, and only while the user's preference is
//! on, so it is created programmatically and destroyed when either condition
//! stops holding.
//!
//! Two `AppKit` details carry the "liquid glass" and the "does not steal focus"
//! requirements:
//!   · a `NSVisualEffectView` sits *below* the web view, so the expanded panel
//!     blurs the real desktop behind the window (CSS `backdrop-filter` cannot:
//!     a transparent window has no desktop in its own compositing tree);
//!   · the window is borderless at `NSStatusWindowLevel`, which makes
//!     `canBecomeKeyWindow` false and keeps it out of the `Dock`, `Mission Control`
//!     and the app switcher.

// Audited `AppKit` FFI island: only window chrome is configured here.
#![allow(unsafe_code)]

use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use super::detect;
use super::geometry::{self, WindowGeometry};

/// The companion window's label. Also the capability scope in
/// `capabilities/notch.json`.
pub const LABEL: &str = "notch";

/// Create the window if it does not exist, then position it for `expanded`.
///
/// Returns an error when the display no longer reports a housing — the caller
/// treats that as "not available" and removes the window instead of leaving it
/// at a stale position.
pub fn ensure(app: &AppHandle, expanded: bool) -> Result<(), String> {
    let detected =
        detect::detect().ok_or_else(|| "no display reports a camera housing".to_owned())?;
    let target = geometry::window_geometry(detected.screen, detected.notch, expanded);

    if let Some(window) = app.get_webview_window(LABEL) {
        return apply_frame(app, &window, target, expanded);
    }

    let window = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("notch.html".into()))
        .title("Echo")
        .inner_size(target.width, target.height)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .closable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .focused(false)
        .visible(false)
        .build()
        .map_err(|error| error.to_string())?;

    let app_for_main = app.clone();
    app.run_on_main_thread(move || {
        if let Err(error) = configure_native(&window, &target, expanded) {
            tracing::warn!(%error, "notch window native configuration failed");
        }
        // `show` after the frame is set: the window must never flash at the
        // default position before it is aligned to the housing.
        if let Err(error) = window.show() {
            tracing::warn!(%error, "notch window could not be shown");
        }
        let _ = app_for_main;
    })
    .map_err(|error| error.to_string())
}

/// Destroy the window if it exists. Idempotent.
pub fn remove(app: &AppHandle) {
    if let Some(window) = app.get_webview_window(LABEL) {
        if let Err(error) = window.destroy() {
            tracing::warn!(%error, "notch window could not be destroyed");
        }
    }
}

/// Resize/position an existing window for `expanded`, keeping the top edge and
/// horizontal centre fixed (both are recomputed from the screen, not derived
/// from the current frame, so repeated calls cannot drift).
pub fn set_expanded(app: &AppHandle, expanded: bool) -> Result<(), String> {
    let detected =
        detect::detect().ok_or_else(|| "no display reports a camera housing".to_owned())?;
    let target = geometry::window_geometry(detected.screen, detected.notch, expanded);
    let window = app
        .get_webview_window(LABEL)
        .ok_or_else(|| "notch window is not open".to_owned())?;
    apply_frame(app, &window, target, expanded)
}

fn apply_frame(
    app: &AppHandle,
    window: &WebviewWindow,
    target: WindowGeometry,
    expanded: bool,
) -> Result<(), String> {
    let window = window.clone();
    app.run_on_main_thread(move || {
        let Ok(ns_window) = window.ns_window() else {
            return;
        };
        // SAFETY: Tauri returns the live `NSWindow` backing this WebviewWindow.
        let ns_window = unsafe { &*ns_window.cast::<objc2_app_kit::NSWindow>() };
        let rect = objc2_foundation::NSRect::new(
            objc2_foundation::NSPoint::new(target.x, target.y),
            objc2_foundation::NSSize::new(target.width, target.height),
        );
        ns_window.setFrame_display(rect, true);
        // The window is wider than the panel (the outward fillets need the
        // extra room), so the material has to follow the panel — otherwise the
        // blurred band beside it reads as a glass frame around the panel.
        sync_vibrancy(ns_window, expanded);
    })
    .map_err(|error| error.to_string())
}

fn configure_native(
    window: &WebviewWindow,
    target: &WindowGeometry,
    expanded: bool,
) -> Result<(), String> {
    use objc2_app_kit::{
        NSColor, NSStatusWindowLevel, NSWindow, NSWindowCollectionBehavior, NSWindowStyleMask,
    };
    use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize};

    let mtm = MainThreadMarker::new()
        .ok_or_else(|| "notch window must be configured on the main thread".to_owned())?;
    let ns_window = window.ns_window().map_err(|error| error.to_string())?;
    // SAFETY: Tauri returns the live `NSWindow` backing this WebviewWindow, and
    // this runs on `AppKit`'s main thread.
    let ns_window = unsafe { &*ns_window.cast::<NSWindow>() };

    // Borderless also makes `canBecomeKeyWindow` false, so clicking the panel
    // never activates Echo or moves focus out of the user's current app.
    ns_window.setStyleMask(NSWindowStyleMask::Borderless);
    ns_window.setLevel(NSStatusWindowLevel);
    ns_window.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::IgnoresCycle
            | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );
    ns_window.setHasShadow(false);
    ns_window.setOpaque(false);
    ns_window.setBackgroundColor(Some(&NSColor::clearColor()));
    ns_window.setFrame_display(
        NSRect::new(
            NSPoint::new(target.x, target.y),
            NSSize::new(target.width, target.height),
        ),
        true,
    );

    install_vibrancy(mtm, ns_window);
    sync_vibrancy(ns_window, expanded);
    Ok(())
}

/// The panel's width when expanded: the 360 pt body inside the 412 pt window.
const EXPANDED_PANEL_WIDTH: f64 = 360.0;
/// Bottom corner radius of the collapsed bar / expanded panel (`notch.css`).
const COLLAPSED_RADIUS: f64 = 9.0;
const EXPANDED_RADIUS: f64 = 24.0;

/// Shape the material to the **panel**, not to the window.
///
/// The window is deliberately larger than the panel: the expanded state needs
/// 26 pt of room on each side for the outward fillets. A material that filled
/// the window would therefore paint a blurred band beside the panel — read as a
/// glass frame around it. Setting the view's frame to the panel rect and
/// masking it to the same rounded rect keeps the glass exactly where the body
/// is; the top corners are square in CSS, so the mask's rounded top corners only
/// affect an area that is already painted by the panel's own background.
fn sync_vibrancy(ns_window: &objc2_app_kit::NSWindow, expanded: bool) {
    use objc2_foundation::{NSRect, NSSize};

    let Some(effect) = vibrancy_view(ns_window) else {
        return;
    };
    let bounds = ns_window
        .contentView()
        .map_or_else(|| NSSize::new(0.0, 0.0), |content| content.bounds().size);

    let (width, radius) = if expanded {
        (EXPANDED_PANEL_WIDTH, EXPANDED_RADIUS)
    } else {
        (bounds.width, COLLAPSED_RADIUS)
    };
    let x = (bounds.width - width) / 2.0;
    effect.setFrame(NSRect::new(
        objc2_foundation::NSPoint::new(x, 0.0),
        NSSize::new(width, bounds.height),
    ));
    effect.setMaskImage(Some(&rounded_mask(width, bounds.height, radius)));
}

/// Find the material view installed by [`install_vibrancy`].
fn vibrancy_view(
    ns_window: &objc2_app_kit::NSWindow,
) -> Option<objc2::rc::Retained<objc2_app_kit::NSVisualEffectView>> {
    let content = ns_window.contentView()?;
    let subviews = content.subviews();
    for index in 0..subviews.count() {
        if let Ok(effect) = subviews
            .objectAtIndex(index)
            .downcast::<objc2_app_kit::NSVisualEffectView>()
        {
            return Some(effect);
        }
    }
    None
}

/// An alpha mask with a rounded rect: opaque inside, transparent outside.
///
/// Uses `+imageWithSize:flipped:drawingHandler:` rather than the deprecated
/// `lockFocus` pair: the drawing handler is the supported synchronous way to
/// describe an image without a resolution-dependent focus stack.
fn rounded_mask(
    width: f64,
    height: f64,
    radius: f64,
) -> objc2::rc::Retained<objc2_app_kit::NSImage> {
    use block2::RcBlock;
    use objc2_app_kit::{NSBezierPath, NSColor, NSImage};
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    let size = NSSize::new(width, height);
    let handler = RcBlock::new(move |_dst: NSRect| -> objc2::runtime::Bool {
        NSColor::blackColor().set();
        let rect = NSRect::new(NSPoint::new(0.0, 0.0), size);
        NSBezierPath::bezierPathWithRoundedRect_xRadius_yRadius(rect, radius, radius).fill();
        objc2::runtime::Bool::YES
    });
    NSImage::imageWithSize_flipped_drawingHandler(size, false, &handler)
}

/// Put a `.behindWindow` vibrancy view under the web view so the expanded
/// panel's translucent background blurs the actual desktop.
fn install_vibrancy(mtm: objc2_foundation::MainThreadMarker, ns_window: &objc2_app_kit::NSWindow) {
    use objc2_app_kit::{
        NSVisualEffectBlendingMode, NSVisualEffectMaterial, NSVisualEffectState, NSVisualEffectView,
    };
    use objc2_foundation::NSInteger;

    let Some(content) = ns_window.contentView() else {
        return;
    };
    let effect = NSVisualEffectView::new(mtm);
    effect.setMaterial(NSVisualEffectMaterial::HUDWindow);
    effect.setBlendingMode(NSVisualEffectBlendingMode::BehindWindow);
    // `Active` keeps the blur steady even when Echo is not the front app, which
    // is the normal case for this window.
    effect.setState(NSVisualEffectState::Active);
    // The frame is driven by [`sync_vibrancy`] (panel-shaped, not window-shaped),
    // so it must not also be autoresized by AppKit's layout pass.
    effect.setAutoresizingMask(objc2_app_kit::NSAutoresizingMaskOptions::empty());
    let _: NSInteger = 0;
    content.addSubview_positioned_relativeTo(
        &effect,
        objc2_app_kit::NSWindowOrderingMode::Below,
        None,
    );
}

// --- screen parameter observer ----------------------------------------------
//
// Plugging/unplugging a display, changing arrangement or resolution, or
// closing/opening the lid all republish the screen list. The notch can appear
// or disappear in that moment, so the window is re-evaluated rather than left
// at a stale frame.

use std::cell::RefCell;

use objc2::{
    define_class, msg_send, rc::Retained, runtime::NSObjectProtocol, DefinedClass, MainThreadOnly,
};
use objc2_app_kit::NSApplicationDidChangeScreenParametersNotification;
use objc2_foundation::{NSNotification, NSNotificationCenter, NSObject};

thread_local! {
    static OBSERVER: RefCell<Option<Retained<ScreenObserver>>> = const { RefCell::new(None) };
}

struct ObserverIvars {
    app: AppHandle,
}

define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = ObserverIvars]
    struct ScreenObserver;

    unsafe impl NSObjectProtocol for ScreenObserver {}

    impl ScreenObserver {
        #[unsafe(method(screenParametersChanged:))]
        fn screen_parameters_changed(&self, _notification: &NSNotification) {
            let app = self.ivars().app.clone();
            // The notification can arrive while `AppKit` is mid-update; defer the
            // work so the screen list is settled and the window frame is applied
            // outside the notification delivery.
            std::thread::spawn(move || {
                let callback_app = app.clone();
                let _ = app.run_on_main_thread(move || {
                    if let Some(player) = callback_app.try_state::<super::NotchPlayer>() {
                        player.refresh_screen(&callback_app);
                    }
                });
            });
        }
    }
);

/// Observe screen-parameter changes for the lifetime of the app.
///
/// Idempotent: a second install replaces the stored observer instead of
/// stacking a second notification registration.
pub fn install_screen_observer(app: &AppHandle) -> Result<(), String> {
    let mtm = objc2_foundation::MainThreadMarker::new()
        .ok_or_else(|| "screen observer must be installed on the main thread".to_owned())?;
    let this = ScreenObserver::alloc(mtm).set_ivars(ObserverIvars { app: app.clone() });
    // SAFETY: NSObject's designated initializer is valid for this subclass.
    let observer: Retained<ScreenObserver> = unsafe { msg_send![super(this), init] };
    let center = NSNotificationCenter::defaultCenter();
    // SAFETY: the selector is implemented above and the observer outlives the
    // registration (kept in the thread-local slot).
    unsafe {
        center.addObserver_selector_name_object(
            &observer,
            objc2::sel!(screenParametersChanged:),
            Some(NSApplicationDidChangeScreenParametersNotification),
            None,
        );
    }
    OBSERVER.with(|slot| *slot.borrow_mut() = Some(observer));
    Ok(())
}

/// Remove the observer (called on explicit shutdown in tests and on exit).
pub fn unregister_screen_observer() {
    OBSERVER.with(|slot| {
        if let Some(observer) = slot.borrow_mut().take() {
            // SAFETY: the observer is still alive here; removing it before drop
            // keeps the notification center from messaging a freed object.
            unsafe {
                NSNotificationCenter::defaultCenter().removeObserver(&observer);
            }
        }
    });
}
