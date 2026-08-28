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
use std::sync::mpsc;
use std::thread;

use rusqlite::Connection;

use crate::error::Error;

type BoxedWrite = Box<dyn FnOnce(&mut Connection) -> Result<Box<dyn Any + Send>, Error> + Send>;

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
}

impl SqliteWriteActor {
    pub(super) fn spawn(connection: Connection) -> Self {
        let (sender, receiver) = mpsc::channel::<WriteRequest>();
        thread::Builder::new()
            .name("echo-sqlite-writer".to_owned())
            .spawn(move || {
                let mut connection = connection;
                while let Ok(request) = receiver.recv() {
                    let result = (request.action)(&mut connection);
                    let _ = request.reply.send(result);
                }
            })
            .expect("creating the SQLite writer thread must succeed");
        Self { sender }
    }

    pub(super) fn run<T: Send + 'static>(
        &self,
        action: impl FnOnce(&mut Connection) -> Result<T, Error> + Send + 'static,
    ) -> Result<T, Error> {
        let (reply, receiver) = mpsc::channel();
        self.sender
            .send(WriteRequest {
                action: Box::new(move |connection| {
                    action(connection).map(|value| Box::new(value) as _)
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
