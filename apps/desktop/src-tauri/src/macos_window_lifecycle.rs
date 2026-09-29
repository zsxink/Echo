//! macOS main-window lifecycle helpers for native fullscreen close behavior.

use std::sync::atomic::{AtomicBool, Ordering};

/// Defers hiding the main window until macOS has completed fullscreen exit.
#[derive(Default)]
pub struct PendingFullscreenHide {
    pending: AtomicBool,
}

impl PendingFullscreenHide {
    pub(super) fn begin(&self) {
        self.pending.store(true, Ordering::Release);
    }

    pub(super) fn cancel(&self) {
        self.pending.store(false, Ordering::Release);
    }

    pub(super) fn on_resize(&self, is_fullscreen: bool) -> bool {
        !is_fullscreen && self.pending.swap(false, Ordering::AcqRel)
    }

    #[cfg(target_os = "macos")]
    fn is_pending(&self) -> bool {
        self.pending.load(Ordering::Acquire)
    }
}

/// Retries the fullscreen close after `AppKit` has had time to finish its native
/// transition. Tao normally emits a resize from `windowDidExitFullScreen`, but
/// the installed app has shown that relying on that event alone can leave the
/// window visible. The first retry is deliberately delayed so Tao's early
/// fullscreen state update cannot hide the window during the animation.
#[cfg(target_os = "macos")]
pub fn schedule_fullscreen_hide_retries(
    app: tauri::AppHandle,
    pending: std::sync::Arc<PendingFullscreenHide>,
    window_label: &'static str,
) {
    use std::{thread, time::Duration};
    use tauri::Manager;

    thread::spawn(move || {
        for delay in [750, 500, 500, 1_000, 1_500] {
            thread::sleep(Duration::from_millis(delay));
            if !pending.is_pending() {
                return;
            }

            let app = app.clone();
            let callback_app = app.clone();
            let pending = std::sync::Arc::clone(&pending);
            if let Err(error) = app.run_on_main_thread(move || {
                if !pending.is_pending() {
                    return;
                }
                let Some(window) = callback_app.get_webview_window(window_label) else {
                    pending.cancel();
                    return;
                };
                match window.is_fullscreen() {
                    Ok(false) => match window.hide() {
                        Ok(()) => pending.cancel(),
                        Err(error) => {
                            tracing::warn!(%error, "failed to hide the main window after fullscreen close");
                        }
                    },
                    Ok(true) => tracing::debug!("waiting for fullscreen exit before hiding the main window"),
                    Err(error) => tracing::warn!(%error, "failed to read fullscreen state while retrying window hide"),
                }
            }) {
                tracing::warn!(%error, "failed to schedule fullscreen close retry");
            }
        }

        if pending.is_pending() {
            tracing::warn!("main window remains visible after fullscreen close retries");
        }
    });
}
