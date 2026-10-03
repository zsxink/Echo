//! One tracked debounce thread per service, with a joined shutdown.
use super::{persist, PreferencesWrite, Shared};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::Duration;

pub(super) struct PreferenceWorker {
    sender: mpsc::Sender<bool>,
    handle: Option<JoinHandle<()>>,
}
impl PreferenceWorker {
    pub(super) fn start(shared: Arc<Shared>) -> Self {
        let (sender, receiver) = mpsc::channel();
        let handle = thread::spawn(move || {
            loop {
                match receiver.recv_timeout(Duration::from_millis(25)) {
                    Ok(false) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                    Ok(true) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                }
                let mut inner = shared
                    .inner
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if inner.state.runtime.persistence_status
                    != crate::effects::PersistenceStatus::Failed
                    && inner
                        .pending_since
                        .is_some_and(|since| since.elapsed() >= Duration::from_millis(250))
                {
                    if let Err(error) = persist(&shared, &mut inner, PreferencesWrite::Normal) {
                        // Retain the tail for explicit retry, without a write storm.
                        tracing::warn!(error = %error, "effects preference debounce failed");
                    }
                }
            }
        });
        Self {
            sender,
            handle: Some(handle),
        }
    }
    pub(super) fn wake(&self) {
        let _ = self.sender.send(true);
    }
}
impl Drop for PreferenceWorker {
    fn drop(&mut self) {
        let _ = self.sender.send(false);
        if let Some(handle) = self.handle.take() {
            if handle.join().is_err() {
                tracing::error!("effects preference worker panicked");
            }
        }
    }
}
