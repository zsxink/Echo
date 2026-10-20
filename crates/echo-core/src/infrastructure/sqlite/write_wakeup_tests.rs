use super::SqliteDatabase;
use std::sync::mpsc::RecvTimeoutError;
use std::time::Duration;

#[test]
fn write_wakeup_only_follows_successful_writer_actions() {
    let directory = tempfile::tempdir().expect("temporary directory");
    let database = SqliteDatabase::open(directory.path().join("echo.sqlite")).expect("database");
    let wakeups = database.subscribe_writes();

    database
        .writer
        .run(|_| {
            Err::<(), _>(crate::error::Error::validation(
                crate::error::Subject::Other,
                "test",
                "injected failure",
            ))
        })
        .expect_err("failed action is reported");
    assert!(matches!(
        wakeups.recv_timeout(Duration::from_millis(20)),
        Err(RecvTimeoutError::Timeout)
    ));

    database
        .writer
        .run(|_| Ok(()))
        .expect("successful writer action");
    assert!(wakeups.recv_timeout(Duration::from_secs(1)).is_ok());
}

fn insert_outbox(
    connection: &rusqlite::Connection,
    revision: i64,
) -> Result<(), crate::error::Error> {
    connection.execute(
        "INSERT INTO sync_outbox(object_type,object_uuid,revision,operation,payload_json,created_at) VALUES('song','sync-wake-song',?1,'upsert','{}',1)",
        [revision],
    ).map_err(super::support::storage)?;
    Ok(())
}

#[test]
fn sync_wakeup_ignores_bookkeeping_and_rolled_back_outbox_rows() {
    use crate::application::ports::LocalSyncQueuePort;

    let directory = tempfile::tempdir().unwrap();
    let database = SqliteDatabase::open(directory.path().join("echo.sqlite")).unwrap();
    let (wake, wakeups) = std::sync::mpsc::channel();
    database.register_sync_waker(wake);
    let writes = database.subscribe_writes();

    database.writer.run(|_| Ok(())).unwrap();
    assert_eq!(database.advance_rejected_local_mutations().unwrap(), 0);
    database
        .writer
        .run(|connection| {
            connection
                .execute(
                    "UPDATE device_state SET value=value WHERE key='device_id'",
                    [],
                )
                .map_err(super::support::storage)?;
            Ok(())
        })
        .unwrap();
    database
        .writer
        .run(|connection| {
            let tx = connection.transaction().map_err(super::support::storage)?;
            insert_outbox(&tx, 1)?;
            tx.rollback().map_err(super::support::storage)
        })
        .unwrap();
    database
        .writer
        .run(|connection| {
            let tx = connection.transaction().map_err(super::support::storage)?;
            insert_outbox(&tx, 1)?;
            Err::<(), _>(crate::error::Error::conflict("rollback injected failure"))
        })
        .expect_err("rolled-back failure must propagate");
    assert_eq!(
        writes.try_iter().count(),
        4,
        "ordinary writes keep their success semantics"
    );
    assert!(matches!(
        wakeups.try_recv(),
        Err(std::sync::mpsc::TryRecvError::Empty)
    ));

    database
        .writer
        .run(|connection| insert_outbox(connection, 1))
        .unwrap();
    wakeups
        .try_recv()
        .expect("committed local work wakes sync immediately");
    assert!(wakeups.try_recv().is_err());
    database
        .writer
        .run(|connection| {
            connection
                .execute("UPDATE sync_outbox SET pushed_at=1", [])
                .map_err(super::support::storage)?;
            Ok(())
        })
        .unwrap();
    assert!(
        wakeups.try_recv().is_err(),
        "acknowledging local work must not create more work"
    );
    database
        .writer
        .run(|connection| insert_outbox(connection, 2))
        .unwrap();
    wakeups.try_recv().expect("later local work is not lost");
}

#[test]
fn sync_outbox_head_uses_the_primary_key_endpoint() {
    let directory = tempfile::tempdir().unwrap();
    let database = SqliteDatabase::open(directory.path().join("echo.sqlite")).unwrap();
    let instructions: Vec<String> = database
        .with_reader(|connection| {
            let mut statement = connection
                .prepare("EXPLAIN SELECT MAX(outbox_id) FROM sync_outbox")
                .map_err(super::support::storage)?;
            let rows = statement
                .query_map([], |row| row.get(1))
                .map_err(super::support::storage)?;
            rows.collect::<Result<_, _>>()
                .map_err(super::support::storage)
        })
        .unwrap();
    assert!(
        instructions.iter().any(|opcode| opcode == "Last"),
        "MAX must seek the INTEGER PRIMARY KEY endpoint: {instructions:?}"
    );
    assert!(
        !instructions.iter().any(|opcode| opcode == "Rewind"),
        "MAX must not scan from the beginning: {instructions:?}"
    );
}
