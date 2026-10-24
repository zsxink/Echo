//! Keep the native event loop alive while admitted local writes drain.
use echo_core::error::Error;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use tauri::Manager;

#[derive(Default)]
pub struct ExitCoordinator(AtomicU8);

impl ExitCoordinator {
    fn ready(&self) -> bool {
        self.0.load(Ordering::Acquire) == 2
    }
    fn begin(&self) -> bool {
        self.0
            .compare_exchange(0, 1, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }
    fn complete(&self) {
        self.0.store(2, Ordering::Release);
    }
    fn retry(&self) {
        self.0.store(0, Ordering::Release);
    }

    // This is the production dispatcher, including failure recovery. Keeping
    // the work injectable tests the real background path without a UI loop.
    fn dispatch(
        self: &Arc<Self>,
        work: impl FnOnce() -> Result<(), Error> + Send + 'static,
        ready: impl FnOnce() + Send + 'static,
        failed: impl FnOnce(Error) + Send + 'static,
    ) -> std::io::Result<bool> {
        if !self.begin() {
            return Ok(false);
        }
        let coordinator = Arc::clone(self);
        let spawned = std::thread::Builder::new()
            .name("echo-graceful-exit".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(work))
                    .unwrap_or_else(|_| {
                        Err(Error::unavailable("shutdown", "shutdown worker panicked"))
                    });
                match result {
                    Ok(()) => {
                        coordinator.complete();
                        ready();
                    }
                    Err(error) => {
                        coordinator.retry();
                        failed(error);
                    }
                }
            });
        if let Err(error) = spawned {
            self.retry();
            return Err(error);
        }
        Ok(true)
    }
}

fn stop_producers(app: &tauri::AppHandle) -> Result<(), Error> {
    // Attempt both stops even if one fails. Sync's exit fence remains set so
    // a failed cleanup cannot let an in-flight root handoff restart producers.
    let sync = app
        .try_state::<Arc<super::sync_runtime::SyncRuntime>>()
        .map_or(Ok(()), |sync| {
            sync.shutdown()
                .map(|_| ())
                .map_err(|error| Error::unavailable("sync shutdown", format!("{error:?}")))
        });
    if let Some(trash) = app.try_state::<super::TrashFinalizationWorker>() {
        trash.stop();
    }
    sync
}

pub fn request(
    app: &tauri::AppHandle,
    api: &tauri::ExitRequestApi,
    code: Option<i32>,
    coordinator: &Arc<ExitCoordinator>,
) {
    if coordinator.ready() {
        super::commands_audio_effects::flush(app);
        #[cfg(target_os = "macos")]
        super::macos_now_playing::clear();
        #[cfg(target_os = "macos")]
        super::macos_window_lifecycle::unregister();
        return;
    }
    api.prevent_exit();
    let work_app = app.clone();
    let ready_app = app.clone();
    if let Err(error) = coordinator.dispatch(
        move || {
            work_app.try_state::<super::AppServices>().map_or_else(
                || stop_producers(&work_app),
                |services| services.shutdown(|| stop_producers(&work_app)),
            )
        },
        move || ready_app.exit(code.unwrap_or(0)),
        |error| tracing::error!(%error, "graceful shutdown failed"),
    ) {
        tracing::error!(%error, "failed to spawn graceful shutdown worker");
    }
}

#[cfg(test)]
mod tests;
