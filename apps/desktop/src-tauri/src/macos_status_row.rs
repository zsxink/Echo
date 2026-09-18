//! Native macOS menu-bar transport controls.
//!
//! One compact `NSStatusItem` owns the complete four-icon row. Its root view
//! handles pointer hit testing and maps the local x-coordinate to exactly one
//! action; the child buttons are decorative/accessibility elements rather than
//! competing pointer targets. Command meaning and playback authority remain in
//! `echo-desktop`.

// This is the one audited AppKit FFI island in the Tauri shell. The raw
// pointer below only crosses Tauri's main-thread scheduler and points to a
// deliberately leaked app-lifetime Objective-C view.
#![allow(unsafe_code)]

use std::{
    cell::RefCell,
    sync::{Arc, OnceLock},
};

use echo_core::domain::state::PlaybackState;
use echo_desktop::platform::status_menu::StatusMenuSink;
use echo_desktop::player::port::PlayerCommand;
use objc2::{
    define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, NSObjectProtocol},
    sel, AnyThread, DefinedClass, MainThreadOnly,
};
use objc2_app_kit::{
    NSAccessibility, NSButton, NSEvent, NSImage, NSStatusBar, NSStatusItem, NSView,
};
use objc2_foundation::{ns_string, MainThreadMarker, NSData, NSPoint, NSRect, NSSize};

const CONTROL_WIDTH: f64 = 26.0;
const CONTROL_COUNT: usize = 4;
const STATUS_ROW_WIDTH: f64 = 104.0;
const STATUS_IMAGE_SIZE: f64 = 16.0;
#[cfg(test)]
const VISUAL_ORDER: [StatusAction; CONTROL_COUNT] = [
    StatusAction::Previous,
    StatusAction::PlayPause,
    StatusAction::Next,
    StatusAction::ShowWindow,
];
/// `AppKit` decodes the existing bundle ICNS reliably. Template rendering makes
/// its Echo mark black on a light menu bar and preserves contrast in dark mode.
const BRAND_ICON: &[u8] = include_bytes!("../icons/icon.icns");

/// Pointer to the retained root view. It is initialized once during app setup
/// and dereferenced only by [`refresh_play_pause`] on `AppKit`'s main thread.
static STATUS_VIEW: OnceLock<usize> = OnceLock::new();

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum StatusAction {
    Previous,
    PlayPause,
    Next,
    ShowWindow,
}

impl StatusAction {
    const fn command(self) -> Option<PlayerCommand> {
        match self {
            Self::Previous => Some(PlayerCommand::Previous),
            Self::PlayPause => Some(PlayerCommand::TogglePlayPause),
            Self::Next => Some(PlayerCommand::Next),
            Self::ShowWindow => None,
        }
    }
}

/// Returns exactly one action for a point inside the compact row.
fn action_at_x(x: f64) -> Option<StatusAction> {
    if !(0.0..STATUS_ROW_WIDTH).contains(&x) {
        return None;
    }
    match x {
        x if x < CONTROL_WIDTH => Some(StatusAction::Previous),
        x if x < CONTROL_WIDTH * 2.0 => Some(StatusAction::PlayPause),
        x if x < CONTROL_WIDTH * 3.0 => Some(StatusAction::Next),
        _ => Some(StatusAction::ShowWindow),
    }
}

struct StatusRowViewIvars {
    sink: Arc<dyn StatusMenuSink>,
    show_main_window: Arc<dyn Fn() + Send + Sync>,
    play_pause: RefCell<Option<Retained<NSButton>>>,
}

define_class!(
    // SAFETY: NSView supports subclassing. All AppKit access is confined to the
    // main thread by `MainThreadOnly`; ivar callbacks are Send + Sync because
    // the desktop runtime owns their cross-thread state.
    #[unsafe(super = NSView)]
    #[thread_kind = MainThreadOnly]
    #[ivars = StatusRowViewIvars]
    struct StatusRowView;

    // SAFETY: NSObjectProtocol has no additional safety requirements.
    unsafe impl NSObjectProtocol for StatusRowView {}

    impl StatusRowView {
        // SAFETY: AppKit supplies a live event while the app-lifetime view is on
        // the main thread. The coordinate conversion produces this view's local
        // x-coordinate, which maps to one half-open control region.
        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let local = self.convertPoint_fromView(event.locationInWindow(), None);
            if let Some(action) = action_at_x(local.x) {
                self.dispatch(action);
            }
        }

        // SAFETY: Returning this live app-lifetime view is valid. Owning pointer
        // hit testing at the root prevents decorative child buttons from
        // overlapping or rerouting clicks.
        #[unsafe(method(hitTest:))]
        fn hit_test(&self, _point: NSPoint) -> Option<&NSView> {
            Some(&**self)
        }

        // The four selectors remain on the child buttons for accessibility
        // activation. Pointer activation always enters through `mouseDown:`.
        #[unsafe(method(previous:))]
        fn previous(&self, _sender: &AnyObject) {
            self.dispatch(StatusAction::Previous);
        }

        #[unsafe(method(togglePlayPause:))]
        fn toggle_play_pause(&self, _sender: &AnyObject) {
            self.dispatch(StatusAction::PlayPause);
        }

        #[unsafe(method(next:))]
        fn next(&self, _sender: &AnyObject) {
            self.dispatch(StatusAction::Next);
        }

        #[unsafe(method(showMainWindow:))]
        fn show_main_window(&self, _sender: &AnyObject) {
            self.dispatch(StatusAction::ShowWindow);
        }
    }
);

impl StatusRowView {
    fn new(
        mtm: MainThreadMarker,
        height: f64,
        sink: Arc<dyn StatusMenuSink>,
        show_main_window: Arc<dyn Fn() + Send + Sync>,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(StatusRowViewIvars {
            sink,
            show_main_window,
            play_pause: RefCell::new(None),
        });
        let frame = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(STATUS_ROW_WIDTH, height),
        );
        // SAFETY: `initWithFrame:` is NSView's designated initializer for this
        // freshly allocated subclass whose Rust ivars were initialized above.
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    fn set_play_pause_button(&self, button: Retained<NSButton>) {
        self.ivars().play_pause.replace(Some(button));
    }

    fn refresh_play_pause(&self, state: PlaybackState) {
        let Some(button) = self.ivars().play_pause.borrow().as_ref().cloned() else {
            return;
        };
        let (symbol, label) = if state == PlaybackState::Playing {
            ("pause.fill", "暂停")
        } else {
            ("play.fill", "播放")
        };
        set_symbol_button(&button, symbol, label);
    }

    fn dispatch(&self, action: StatusAction) {
        dispatch_action(&*self.ivars().sink, &*self.ivars().show_main_window, action);
    }
}

/// `AppKit` objects remain main-thread confined and live for the application.
/// One status item avoids the system spacing added between separate items.
struct StatusRowRoot {
    _item: Retained<NSStatusItem>,
    _view: Retained<StatusRowView>,
    _buttons: [Retained<NSButton>; CONTROL_COUNT],
}

/// Install Echo's compact, always-visible menu-bar control row.
///
/// # Errors
///
/// Returns an error when setup is not running on macOS' main thread.
pub fn install(
    sink: Arc<dyn StatusMenuSink>,
    show_main_window: Arc<dyn Fn() + Send + Sync>,
) -> Result<(), String> {
    let mtm = MainThreadMarker::new()
        .ok_or_else(|| "macOS status controls must be installed on the main thread".to_owned())?;
    let bar = NSStatusBar::systemStatusBar();
    let item = bar.statusItemWithLength(STATUS_ROW_WIDTH);
    let view = StatusRowView::new(mtm, bar.thickness(), sink, show_main_window);

    let previous = make_button(&view, mtm, 0.0, "上一首", sel!(previous:));
    set_symbol_button(&previous, "backward.end.fill", "上一首");

    let play_pause = make_button(&view, mtm, CONTROL_WIDTH, "播放", sel!(togglePlayPause:));
    set_symbol_button(&play_pause, "play.fill", "播放");

    let next = make_button(&view, mtm, CONTROL_WIDTH * 2.0, "下一首", sel!(next:));
    set_symbol_button(&next, "forward.end.fill", "下一首");

    let brand = make_button(
        &view,
        mtm,
        CONTROL_WIDTH * 3.0,
        "打开 Echo",
        sel!(showMainWindow:),
    );
    set_brand_button(&brand);

    view.set_play_pause_button(play_pause.clone());
    // The children remain separate accessible buttons, while the root owns all
    // pointer hit testing and routes from the event x-coordinate.
    view.setAccessibilityElement(false);
    #[allow(deprecated)]
    item.setView(Some(&view));

    STATUS_VIEW
        .set(std::ptr::from_ref::<StatusRowView>(view.as_ref()) as usize)
        .map_err(|_| "macOS status controls are already installed".to_owned())?;
    let _root: &'static mut StatusRowRoot = Box::leak(Box::new(StatusRowRoot {
        _item: item,
        _view: view,
        _buttons: [previous, play_pause, next, brand],
    }));
    Ok(())
}

/// Apply the authoritative player snapshot to the compact toggle region.
///
/// Callers schedule this onto Tauri/AppKit's main thread. The raw view address
/// is valid because `install` leaks the root for the app lifetime.
pub fn refresh_play_pause(state: PlaybackState) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let Some(address) = STATUS_VIEW.get() else {
        return;
    };
    // SAFETY: `mtm` proves this is AppKit's main thread; the root is app-lived.
    let view = unsafe { &*(*address as *const StatusRowView) };
    let _ = mtm;
    view.refresh_play_pause(state);
}

fn make_button(
    view: &StatusRowView,
    mtm: MainThreadMarker,
    x: f64,
    accessibility_label: &str,
    action: objc2::runtime::Sel,
) -> Retained<NSButton> {
    let frame = NSRect::new(
        NSPoint::new(x, 0.0),
        NSSize::new(CONTROL_WIDTH, view.frame().size.height),
    );
    let button = NSButton::initWithFrame(NSButton::alloc(mtm), frame);
    button.setBordered(false);
    button.setTitle(ns_string!(""));
    button.setAccessibilityLabel(Some(&objc2_foundation::NSString::from_str(
        accessibility_label,
    )));
    // SAFETY: Every selector is implemented by `StatusRowView`. These targets
    // serve accessibility activation; root hit testing handles pointer input.
    unsafe {
        button.setTarget(Some(view));
        button.setAction(Some(action));
    }
    view.addSubview(&button);
    button
}

fn dispatch_action(sink: &dyn StatusMenuSink, show_main_window: &dyn Fn(), action: StatusAction) {
    if let Some(command) = action.command() {
        sink.on_command(command);
    } else {
        show_main_window();
    }
}

fn set_symbol_button(button: &NSButton, symbol: &str, accessibility_label: &str) {
    let symbol_name = objc2_foundation::NSString::from_str(symbol);
    let description = objc2_foundation::NSString::from_str(accessibility_label);
    if let Some(image) = NSImage::imageWithSystemSymbolName_accessibilityDescription(
        &symbol_name,
        Some(&description),
    ) {
        image.setSize(NSSize::new(STATUS_IMAGE_SIZE, STATUS_IMAGE_SIZE));
        button.setTitle(ns_string!(""));
        button.setImage(Some(&image));
    } else {
        button.setImage(None);
        button.setTitle(&description);
    }
    button.setAccessibilityLabel(Some(&description));
}

fn set_brand_button(button: &NSButton) {
    // NSData copies the bundled ICNS before NSImage decodes it.
    let data =
        unsafe { NSData::dataWithBytes_length(BRAND_ICON.as_ptr().cast(), BRAND_ICON.len()) };
    if let Some(image) = NSImage::initWithData(NSImage::alloc(), &data) {
        image.setTemplate(true);
        image.setSize(NSSize::new(STATUS_IMAGE_SIZE, STATUS_IMAGE_SIZE));
        button.setTitle(ns_string!(""));
        button.setImage(Some(&image));
    } else {
        button.setImage(None);
        button.setTitle(ns_string!("Echo"));
    }
    button.setAccessibilityLabel(Some(ns_string!("打开 Echo")));
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Mutex,
    };

    #[test]
    fn visual_order_is_compact_and_fixed() {
        assert_eq!(
            VISUAL_ORDER,
            [
                StatusAction::Previous,
                StatusAction::PlayPause,
                StatusAction::Next,
                StatusAction::ShowWindow,
            ]
        );
        assert_eq!(STATUS_ROW_WIDTH, 104.0);
    }

    #[test]
    fn exact_boundaries_route_to_one_non_overlapping_action() {
        assert_eq!(action_at_x(-0.001), None);
        assert_eq!(action_at_x(0.0), Some(StatusAction::Previous));
        assert_eq!(action_at_x(25.999), Some(StatusAction::Previous));
        assert_eq!(action_at_x(26.0), Some(StatusAction::PlayPause));
        assert_eq!(action_at_x(51.999), Some(StatusAction::PlayPause));
        assert_eq!(action_at_x(52.0), Some(StatusAction::Next));
        assert_eq!(action_at_x(77.999), Some(StatusAction::Next));
        assert_eq!(action_at_x(78.0), Some(StatusAction::ShowWindow));
        assert_eq!(action_at_x(103.999), Some(StatusAction::ShowWindow));
        assert_eq!(action_at_x(104.0), None);
    }

    #[test]
    fn playing_state_uses_pause_and_every_other_state_uses_play() {
        let symbol_for = |state| {
            if state == PlaybackState::Playing {
                "pause.fill"
            } else {
                "play.fill"
            }
        };
        assert_eq!(symbol_for(PlaybackState::Playing), "pause.fill");
        assert_eq!(symbol_for(PlaybackState::Paused), "play.fill");
        assert_eq!(symbol_for(PlaybackState::Stopped), "play.fill");
    }

    #[test]
    fn bundled_brand_resource_is_a_decodable_native_icon() {
        assert!(BRAND_ICON.starts_with(b"icns"));
        assert!(BRAND_ICON.len() > 1_000);
    }

    #[test]
    fn each_region_dispatches_only_its_own_action_once() {
        #[derive(Default)]
        struct RecordingSink(Mutex<Vec<PlayerCommand>>);
        impl StatusMenuSink for RecordingSink {
            fn on_command(&self, command: PlayerCommand) {
                self.0.lock().expect("recording sink lock").push(command);
            }
        }

        let sink = RecordingSink::default();
        let shown = AtomicUsize::new(0);
        let show = || {
            shown.fetch_add(1, Ordering::SeqCst);
        };
        for x in [13.0, 39.0, 65.0, 91.0] {
            dispatch_action(&sink, &show, action_at_x(x).expect("center is in a region"));
        }
        assert_eq!(
            *sink.0.lock().expect("recording sink lock"),
            vec![
                PlayerCommand::Previous,
                PlayerCommand::TogglePlayPause,
                PlayerCommand::Next,
            ]
        );
        assert_eq!(shown.load(Ordering::SeqCst), 1);
    }
}
