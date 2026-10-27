//! Enrollment must not turn retained historical intent into a new author event.
use crate::domain::{
    ids::LibraryRootId,
    library::durability::{DurableStateChange, StateKind, StateNamespace, StateTarget},
    sync::SyncBinding,
};
use crate::error::Error;
use crate::infrastructure::sqlite::support::storage;
use rusqlite::{Connection, params};

pub(super) fn require_reconciled(
    c: &Connection,
    root: LibraryRootId,
    binding: SyncBinding,
) -> Result<(), Error> {
    let author: String = c
        .query_row(
            "SELECT value FROM device_state WHERE key='device_id'",
            [],
            |r| r.get(0),
        )
        .map_err(storage)?;
    let mut statement = c.prepare(
        "SELECT envelope_json FROM library_recovery_records WHERE library_root_uuid=?1 AND row_kind='state'",
    ).map_err(storage)?;
    let states = statement
        .query_map(params![root.to_string()], |row| row.get::<_, String>(0))
        .map_err(storage)?
        .map(|row| {
            serde_json::from_str::<DurableStateChange>(&row.map_err(storage)?).map_err(storage)
        })
        .collect::<Result<Vec<_>, Error>>()?;
    for state in &states {
        if state.kind == StateKind::Adoption {
            return Err(blocked());
        }
        if state.kind != StateKind::Pending {
            continue;
        }
        let Some(key) = &state.key else {
            return Err(blocked());
        };
        state.validate()?;
        let StateTarget::Mutation {
            author: old_author, ..
        } = &key.target
        else {
            return Err(blocked());
        };
        let same_author = old_author.as_uuid().to_string() == author;
        if same_author
            && (matches!(key.namespace,
                StateNamespace::Bound { binding: old } if old == binding)
                || matches!(key.namespace, StateNamespace::LocalUnbound { .. }))
        {
            continue;
        }
        // A terminal disposition is still historical evidence. Only the
        // specialized reconciliation flow can verify its original receipt and
        // ledger; a status string alone must not unlock a new author snapshot.
        return Err(blocked());
    }
    Ok(())
}

fn blocked() -> Error {
    Error::conflict(
        "historical pending intent requires explicit reconciliation before replica enrollment",
    )
}
