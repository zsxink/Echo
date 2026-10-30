//! Single-owner SQLite writer actor.
//!
//! Every mutation is serialized through one dedicated OS thread, so no
//! transaction survives an `.await` or contends with a second writer.

#![allow(
    clippy::doc_markdown,
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::needless_pass_by_value,
    clippy::uninlined_format_args
)]

use std::any::Any;
use std::sync::{mpsc, Arc, Mutex};
use std::thread;

use rusqlite::Connection;

use crate::error::Error;

type BoxedWrite =
    Box<dyn FnOnce(&mut Connection) -> Result<(Box<dyn Any + Send>, bool), Error> + Send>;

struct WriteRequest {
    action: BoxedWrite,
    reply: mpsc::Sender<Result<Box<dyn Any + Send>, Error>>,
}

/// Single-owner SQLite writer. All writes are performed on its dedicated OS
/// thread, so no transaction can survive an `.await` or contend with another
/// writer connection.
#[derive(Clone)]
pub(super) struct SqliteWriteActor {
    sender: mpsc::Sender<WriteRequest>,
    listeners: Arc<Mutex<Vec<mpsc::Sender<()>>>>,
    sync_listeners: Arc<Mutex<Vec<mpsc::Sender<()>>>>,
}

impl SqliteWriteActor {
    pub(super) fn spawn(connection: Connection) -> Self {
        let (sender, receiver) = mpsc::channel::<WriteRequest>();
        let listeners = Arc::new(Mutex::new(Vec::<mpsc::Sender<()>>::new()));
        let worker_listeners = Arc::clone(&listeners);
        let sync_listeners = Arc::new(Mutex::new(Vec::<mpsc::Sender<()>>::new()));
        let worker_sync_listeners = Arc::clone(&sync_listeners);
        thread::Builder::new()
            .name("echo-sqlite-writer".to_owned())
            .spawn(move || {
                let mut connection = connection;
                while let Ok(request) = receiver.recv() {
                    let watch_sync = !worker_sync_listeners
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .is_empty();
                    // Outbox ids are an AUTOINCREMENT INTEGER PRIMARY KEY, so
                    // this reads the rightmost B-tree entry rather than scanning
                    // the library. Read committed state around the whole action:
                    // a rolled-back insert must never wake synchronization.
                    let before = if watch_sync {
                        outbox_head(&connection)
                    } else {
                        Ok(None)
                    };
                    let result = before.and_then(|before| {
                        let (value, explicit_wake) = (request.action)(&mut connection)?;
                        let outbox_added =
                            watch_sync && !explicit_wake && outbox_head(&connection)? > before;
                        Ok((value, explicit_wake || outbox_added))
                    });
                    if let Ok((_, sync_work)) = &result {
                        worker_listeners
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .retain(|listener| listener.send(()).is_ok());
                        if *sync_work {
                            worker_sync_listeners
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .retain(|listener| listener.send(()).is_ok());
                        }
                    }
                    let _ = request.reply.send(result.map(|(value, _)| value));
                }
            })
            .expect("creating the SQLite writer thread must succeed");
        Self {
            sender,
            listeners,
            sync_listeners,
        }
    }

    /// Subscribe to successful writer transactions. Subscribers use this only
    /// as a coalescible wake-up hint and must re-read durable state after wake.
    pub(super) fn subscribe(&self) -> mpsc::Receiver<()> {
        let (sender, receiver) = mpsc::channel();
        self.register_listener(sender);
        receiver
    }

    pub(super) fn register_listener(&self, sender: mpsc::Sender<()>) {
        self.listeners
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(sender);
    }

    /// Wake sync only for newly queued local mutations or explicitly declared
    /// background work. Ordinary writer subscribers retain their broader signal.
    pub(super) fn register_sync_listener(&self, sender: mpsc::Sender<()>) {
        self.sync_listeners
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .push(sender);
    }

    pub(super) fn run<T: Send + 'static>(
        &self,
        action: impl FnOnce(&mut Connection) -> Result<T, Error> + Send + 'static,
    ) -> Result<T, Error> {
        self.run_with_sync_wake(move |connection| action(connection).map(|value| (value, false)))
    }

    /// The action declares sync work only after committing a durable transition.
    /// New outbox rows are detected automatically regardless of this flag.
    pub(super) fn run_with_sync_wake<T: Send + 'static>(
        &self,
        action: impl FnOnce(&mut Connection) -> Result<(T, bool), Error> + Send + 'static,
    ) -> Result<T, Error> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(WriteRequest {
                action: Box::new(move |connection| {
                    action(connection).map(|(value, wake)| (Box::new(value) as _, wake))
                }),
                reply,
            })
            .map_err(|_| Error::unavailable("database writer", "writer actor stopped"))?;
        let result = receiver
            .recv()
            .map_err(|_| Error::unavailable("database writer", "writer actor stopped"))??;
        result
            .downcast::<T>()
            .map(|boxed| *boxed)
            .map_err(|_| Error::InvariantViolation {
                why: "SQLite writer returned a result of an unexpected type".to_owned(),
            })
    }
}

/// The writer is opened only after all migrations have installed sync_outbox.
fn outbox_head(connection: &Connection) -> Result<Option<i64>, Error> {
    connection
        .prepare_cached("SELECT MAX(outbox_id) FROM sync_outbox")
        .map_err(super::support::storage)?
        .query_row([], |row| row.get(0))
        .map_err(super::support::storage)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writer_without_sync_subscribers_does_not_query_the_outbox() {
        // There is deliberately no schema here. Normal writer actions must not
        // pay for sync queries when synchronization has no subscribers.
        let writer = SqliteWriteActor::spawn(Connection::open_in_memory().unwrap());
        assert_eq!(writer.run(|_| Ok(42)).unwrap(), 42);
    }
}
