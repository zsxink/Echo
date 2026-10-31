use super::*;

#[test]
fn library_durability_adoption_replays_directory_commit_once_after_projection_interruption() {
    use crate::domain::library::durability::{HistoricalIntent, Reconciliation};

    let mut f = fixture();
    retry(&f);
    let old_intent = f
        .batch
        .state
        .iter()
        .find_map(|state| HistoricalIntent::from_state(state).unwrap())
        .unwrap();
    let new_author = DeviceId::new();
    let reconciliation = Reconciliation {
        states: &f.batch.state,
        authoritative_state: &[],
        ledger: &[],
        new_binding: f.context.verified_checkpoint.binding,
        new_author,
    };
    let adoption = reconciliation.state_change(&old_intent, true).unwrap();
    let new_mutation_id = crate::domain::library::durability::adoption(&adoption.payload)
        .unwrap()
        .new_mutation_id
        .unwrap();
    let mut batch = LibraryBatch::new(
        uuid::Uuid::new_v4(),
        f.context.local_library_id,
        ChangeOrigin::Recovery,
        vec![],
    );
    batch.state.push(adoption);
    // This models a process stop after the authoritative directory commit and
    // before any SQLite projection of the transition.
    f.control.commit(f.context.root, &batch).unwrap();
    f.db.writer
        .run(move |c| {
            c.execute(
                "UPDATE device_state SET value=?1 WHERE key='device_id'",
                [new_author.as_uuid().to_string()],
            )
            .map_err(storage)?;
            Ok(())
        })
        .unwrap();
    f.context.local_author = new_author;

    retry(&f);
    let adopted = f
        .db
        .with_reader(|c| {
            c.query_row(
                "SELECT origin_device_id,origin_seq,payload_json,created_at FROM sync_mutations WHERE mutation_id=?1",
                [new_mutation_id.as_uuid().to_string()],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?, row.get::<_, String>(2)?, row.get::<_, i64>(3)?)),
            )
            .map_err(storage)
        })
        .unwrap();
    assert_eq!(adopted.0, new_author.as_uuid().to_string());
    assert_eq!(adopted.1, 1);
    assert_eq!(adopted.2, f.frozen.original_body);
    assert_eq!(adopted.3, i64::try_from(f.frozen.frozen_at_ms).unwrap());
    let after_first_projection = counts(&f);

    retry(&f);
    assert_eq!(counts(&f), after_first_projection);
    let projected_id_count =
        f.db.with_reader(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM sync_mutations WHERE mutation_id=?1",
                [new_mutation_id.as_uuid().to_string()],
                |row| row.get::<_, u64>(0),
            )
            .map_err(storage)
        })
        .unwrap();
    assert_eq!(projected_id_count, 1);
}
