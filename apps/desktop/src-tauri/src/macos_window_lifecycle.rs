//! macOS main-window lifecycle helpers for native fullscreen close behavior.
#![allow(unsafe_code)]

use std::sync::atomic::{AtomicBool, Ordering};

/// A close request that must wait for `AppKit`'s fullscreen exit to finish.
#[derive(Default)]
pub struct PendingFullscreenHide {
    pending: AtomicBool,
    exiting: AtomicBool,
}

impl PendingFullscreenHide {
    pub(super) fn begin(&self) {
        self.pending.store(true, Ordering::Release);
        self.exiting.store(true, Ordering::Release);
    }

    pub(super) fn cancel(&self) {
        self.pending.store(false, Ordering::Release);
    }

    #[cfg(target_os = "macos")]
    pub(super) fn abort_exit(&self) {
        self.cancel();
        self.exiting.store(false, Ordering::Release);
    }

    pub(super) fn is_exiting(&self) -> bool {
        self.exiting.load(Ordering::Acquire)
    }

    pub(super) fn is_pending(&self) -> bool {
        self.pending.load(Ordering::Acquire)
    }

    pub(super) fn will_exit(&self) {
        self.exiting.store(true, Ordering::Release);
    }

    pub(super) fn did_exit(&self) -> bool {
        self.exiting.store(false, Ordering::Release);
        self.is_pending()
    }
}

#[cfg(target_os = "macos")]
mod native {
    use std::{cell::RefCell, sync::Arc};

    use objc2::{
        define_class, msg_send, rc::Retained, runtime::NSObjectProtocol, sel, DefinedClass,
        MainThreadOnly,
    };
    use objc2_app_kit::{
        NSWindow, NSWindowDidExitFullScreenNotification, NSWindowWillExitFullScreenNotification,
    };
    use objc2_foundation::{MainThreadMarker, NSNotification, NSNotificationCenter, NSObject};
    use tauri::{AppHandle, Manager};

    use super::PendingFullscreenHide;

    // The notification center does not retain selector observers. Keep ours on
    // the AppKit thread for the lifetime of the application.
    thread_local! {
        static OBSERVER: RefCell<Option<Retained<FullscreenExitObserver>>> = const { RefCell::new(None) };
    }

    struct ObserverIvars {
        app: AppHandle,
        pending: Arc<PendingFullscreenHide>,
        window_label: &'static str,
    }

    define_class!(
        #[unsafe(super = NSObject)]
        #[thread_kind = MainThreadOnly]
        #[ivars = ObserverIvars]
        struct FullscreenExitObserver;

        unsafe impl NSObjectProtocol for FullscreenExitObserver {}

        impl FullscreenExitObserver {
            #[unsafe(method(windowWillExitFullScreen:))]
            fn will_exit(&self, _notification: &NSNotification) {
                tracing::debug!("main window will exit native fullscreen");
                self.ivars().pending.will_exit();
            }

            #[unsafe(method(windowDidExitFullScreen:))]
            fn did_exit(&self, _notification: &NSNotification) {
                let ivars = self.ivars();
                tracing::debug!(pending = ivars.pending.is_pending(), "main window did exit native fullscreen");
                if !ivars.pending.did_exit() {
                    return;
                }

                // Tauri executes run_on_main_thread immediately when called on
                // the main thread. Dispatch from a worker so the task cannot
                // run until AppKit and Tao return to the event loop.
                let app = ivars.app.clone();
                let pending = Arc::clone(&ivars.pending);
                let label = ivars.window_label;
                std::thread::spawn(move || {
                    let callback_app = app.clone();
                    if let Err(error) = app.run_on_main_thread(move || {
                        if !pending.is_pending() {
                            return;
                        }
                        if let Some(window) = callback_app.get_webview_window(label) {
                            match window.hide() {
                                Ok(()) => {
                                    tracing::debug!("hid main window after native fullscreen exit");
                                    pending.cancel();
                                }
                                Err(error) => tracing::warn!(%error, "failed to hide the main window after native fullscreen exit"),
                            }
                        }
                    }) {
                        tracing::warn!(%error, "failed to schedule main-window hide after native fullscreen exit");
                    }
                });
            }
        }
    );

    /// Observe only the main native window; other windows must not consume its
    /// pending close. Called on `RunEvent::Ready`, after Tauri creates the
    /// configured window during setup.
    pub fn install(
        app: &AppHandle,
        pending: Arc<PendingFullscreenHide>,
        window_label: &'static str,
    ) -> Result<(), String> {
        let mtm = MainThreadMarker::new()
            .ok_or_else(|| "fullscreen observer must be installed on the main thread".to_owned())?;
        let window = app
            .get_webview_window(window_label)
            .ok_or_else(|| "main window is unavailable for fullscreen observer".to_owned())?;
        let ns_window = window.ns_window().map_err(|error| error.to_string())?;
        // SAFETY: Tauri returns the live NSWindow for this WebviewWindow. The
        // observer is removed before the application releases its windows.
        let ns_window = unsafe { &*ns_window.cast::<NSWindow>() };
        let this = FullscreenExitObserver::alloc(mtm).set_ivars(ObserverIvars {
            app: app.clone(),
            pending,
            window_label,
        });
        // SAFETY: NSObject's designated initializer is valid for this subclass.
        let observer: Retained<FullscreenExitObserver> = unsafe { msg_send![super(this), init] };
        let center = NSNotificationCenter::defaultCenter();
        // SAFETY: Both selectors are implemented by this observer. The object
        // filter is a live NSWindow and unregister() removes the observer.
        unsafe {
            center.addObserver_selector_name_object(
                &observer,
                sel!(windowWillExitFullScreen:),
                Some(NSWindowWillExitFullScreenNotification),
                Some(ns_window),
            );
            center.addObserver_selector_name_object(
                &observer,
                sel!(windowDidExitFullScreen:),
                Some(NSWindowDidExitFullScreenNotification),
                Some(ns_window),
            );
        }
        OBSERVER.with(|slot| *slot.borrow_mut() = Some(observer));
        tracing::debug!("installed main-window fullscreen observer");
        Ok(())
    }

    pub fn unregister() {
        OBSERVER.with(|slot| {
            if let Some(observer) = slot.borrow_mut().take() {
                // SAFETY: This is the same live selector observer registered in
                // install(), and AppKit no longer calls it after removal.
                unsafe { NSNotificationCenter::defaultCenter().removeObserver(&observer) };
            }
        });
    }
}

#[cfg(target_os = "macos")]
pub use native::{install, unregister};
