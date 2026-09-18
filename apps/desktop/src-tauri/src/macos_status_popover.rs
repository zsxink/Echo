//! Native macOS status-item popover.
//!
//! This is deliberately an app-shell-only `AppKit` boundary. The portable
//! `echo-desktop::platform::status_menu` module owns command meaning; this file
//! only creates macOS controls and forwards their one action each.

// This is the one audited AppKit FFI island in the Tauri shell. Every unsafe
// operation below is paired with its selector/lifetime justification.
#![allow(unsafe_code)]

use std::sync::Arc;

use echo_desktop::platform::status_menu::StatusMenuSink;
use echo_desktop::player::port::PlayerCommand;
use objc2::{
    define_class, msg_send,
    rc::Retained,
    runtime::{AnyObject, NSObjectProtocol},
    sel, DefinedClass, MainThreadOnly,
};
use objc2_app_kit::{
    NSAccessibility, NSButton, NSPopover, NSStatusBar, NSStatusBarButton, NSStatusItem, NSView,
    NSViewController,
};
use objc2_foundation::{
    ns_string, MainThreadMarker, NSObject, NSPoint, NSRect, NSRectEdge, NSSize,
};

/// Objective-C target retained by the app-lifetime status-item owner.
///
/// Keeping all actions here makes each native control dispatch exactly one
/// authoritative [`PlayerCommand`] through the existing status-menu sink.
struct PanelTargetIvars {
    sink: Arc<dyn StatusMenuSink>,
    popover: Retained<NSPopover>,
    anchor: Retained<NSStatusBarButton>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and this target owns
    // only Rust values whose access remains on AppKit's main thread.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = PanelTargetIvars]
    struct PanelTarget;

    // SAFETY: NSObjectProtocol has no additional safety requirements.
    unsafe impl NSObjectProtocol for PanelTarget {}

    impl PanelTarget {
        // SAFETY: AppKit invokes this selector with the status-item button as
        // its sender; the sender is intentionally not otherwise inspected.
        #[unsafe(method(togglePopover:))]
        fn toggle_popover(&self, _sender: &AnyObject) {
            let popover = &self.ivars().popover;
            if popover.isShown() {
                popover.close();
            } else {
                let anchor = &self.ivars().anchor;
                popover.showRelativeToRect_ofView_preferredEdge(
                    anchor.bounds(),
                    anchor,
                    NSRectEdge::MinY,
                );
            }
        }

        // SAFETY: The button target/action pair is created below with this
        // exact selector and the sender is not dereferenced.
        #[unsafe(method(previous:))]
        fn previous(&self, _sender: &AnyObject) {
            self.ivars().sink.on_command(PlayerCommand::Previous);
        }

        // SAFETY: The button target/action pair is created below with this
        // exact selector and the sender is not dereferenced.
        #[unsafe(method(togglePlayPause:))]
        fn toggle_play_pause(&self, _sender: &AnyObject) {
            self.ivars()
                .sink
                .on_command(PlayerCommand::TogglePlayPause);
        }

        // SAFETY: The button target/action pair is created below with this
        // exact selector and the sender is not dereferenced.
        #[unsafe(method(next:))]
        fn next(&self, _sender: &AnyObject) {
            self.ivars().sink.on_command(PlayerCommand::Next);
        }
    }
);

impl PanelTarget {
    fn new(
        mtm: MainThreadMarker,
        sink: Arc<dyn StatusMenuSink>,
        popover: Retained<NSPopover>,
        anchor: Retained<NSStatusBarButton>,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(PanelTargetIvars {
            sink,
            popover,
            anchor,
        });
        // SAFETY: This is NSObject's designated initializer for the freshly
        // allocated instance whose ivars were initialized above.
        unsafe { msg_send![super(this), init] }
    }
}

/// `AppKit` objects must stay main-thread confined, so they cannot be placed in
/// Tauri's `Send + Sync` managed state. This root owns them for the process
/// lifetime; macOS tears the status item down with the application.
struct StatusPopoverRoot {
    _item: Retained<NSStatusItem>,
    _target: Retained<PanelTarget>,
}

/// Install Echo's native, status-item-anchored transport popover.
///
/// # Errors
///
/// Returns an error when setup is not running on macOS' main thread.
pub fn install(sink: Arc<dyn StatusMenuSink>) -> Result<(), String> {
    let mtm = MainThreadMarker::new()
        .ok_or_else(|| "macOS status panel must be installed on the main thread".to_owned())?;
    let item = NSStatusBar::systemStatusBar().statusItemWithLength(-1.0);
    let anchor = item
        .button(mtm)
        .ok_or_else(|| "macOS status item has no anchor button".to_owned())?;
    anchor.setTitle(ns_string!("Echo"));
    anchor.setAccessibilityLabel(Some(ns_string!("Echo 播放控制")));

    let popover = NSPopover::new(mtm);
    popover.setContentSize(NSSize::new(168.0, 44.0));
    let target = PanelTarget::new(mtm, sink, popover, anchor.clone());

    // SAFETY: `target` implements every selector supplied here, and AppKit
    // invokes them only while the retained root below is alive.
    unsafe {
        anchor.setTarget(Some(target.as_ref()));
        anchor.setAction(Some(sel!(togglePopover:)));
    }

    let content = NSView::initWithFrame(
        NSView::alloc(mtm),
        NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(168.0, 44.0)),
    );
    let previous = transport_button(mtm, "◀︎", "上一首", &target, sel!(previous:), 8.0);
    let play_pause = transport_button(
        mtm,
        "⏯",
        "播放或暂停",
        &target,
        sel!(togglePlayPause:),
        60.0,
    );
    let next = transport_button(mtm, "▶︎", "下一首", &target, sel!(next:), 112.0);
    // Each retained button is a valid NSView and is retained by its content
    // view for at least as long as the popover is displayed.
    content.addSubview(&previous);
    content.addSubview(&play_pause);
    content.addSubview(&next);
    let controller = NSViewController::new(mtm);
    controller.setView(&content);
    target
        .ivars()
        .popover
        .setContentViewController(Some(&controller));

    // The OS status bar does not promise to retain our Rust target. Keep the
    // root alive for the process lifetime; this is an app-lifetime native UI
    // resource, not a per-interaction allocation.
    let _root: &'static mut StatusPopoverRoot = Box::leak(Box::new(StatusPopoverRoot {
        _item: item,
        _target: target,
    }));
    Ok(())
}

fn transport_button(
    mtm: MainThreadMarker,
    glyph: &str,
    accessibility_label: &str,
    target: &PanelTarget,
    action: objc2::runtime::Sel,
    origin_x: f64,
) -> Retained<NSButton> {
    // SAFETY: `target` implements each selector passed by `install`, and all
    // action methods accept the NSButton sender sent by AppKit.
    let button = unsafe {
        NSButton::buttonWithTitle_target_action(
            &objc2_foundation::NSString::from_str(glyph),
            Some(target.as_ref()),
            Some(action),
            mtm,
        )
    };
    button.setFrame(NSRect::new(
        NSPoint::new(origin_x, 6.0),
        NSSize::new(48.0, 32.0),
    ));
    button.setAccessibilityLabel(Some(&objc2_foundation::NSString::from_str(
        accessibility_label,
    )));
    button
}
