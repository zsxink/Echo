//! Consume retained protocol facts only under explicit live-session authority.
mod adoptions;
mod ledger;
mod mutations;
mod objects;
mod related;
#[cfg(test)]
mod tests;

use super::{retained, storage};
use crate::application::ports::LibrarySyncRecoveryPort;
use crate::domain::library::durability::{
    DurableStateChange, LibraryProjectionStatus, ProjectionUnresolvedReason as Reason,
    RecoverySessionRole, StateNamespace, StateTarget, SyncRecoveryContext,
};
use crate::error::Error;
use crate::infrastructure::sqlite::SqliteDatabase;
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::Value;

impl LibrarySyncRecoveryPort for SqliteDatabase {
    fn retry_library_sync_state(
        &self,
        context: SyncRecoveryContext,
    ) -> Result<LibraryProjectionStatus, Error> {
        self.writer.run(move |connection| {
            let tx = connection.transaction().map_err(storage)?;
            validate_session(&tx, context)?;
            let retained = retained::read(&tx, context.root)?;
            let scoped: Vec<_> = retained
                .state
                .iter()
                .filter(|state| authorized(state, context))
                .collect();
            let ledger = ledger::verified(&tx, context, &scoped)?;
            let mut ordered: Vec<_> = retained.state.iter().collect();
            ordered.sort_by_key(|state| {
                let target = state.key.as_ref().map(|key| &key.target);
                match target {
                    Some(StateTarget::Ledger { .. }) => (0, 0),
                    Some(StateTarget::Confirmed { .. }) => (1, 0),
                    Some(StateTarget::Mutation { .. }) => (
                        3,
                        state
                            .payload
                            .pointer("/frozen/candidate/origin_seq")
                            .and_then(Value::as_u64)
                            .unwrap_or(0),
                    ),
                    Some(StateTarget::MutationDisposition { .. }) => (4, 0),
                    _ => (2, 0),
                }
            });
            for state in ordered {
                if !authorized(state, context) {
                    if !matches!(
                        state.kind,
                        crate::domain::library::durability::StateKind::PlaybackEvent
                            | crate::domain::library::durability::StateKind::ResourceLocation
                    ) {
                        retained::reason(
                            &tx,
                            context.root,
                            &retained::state_key(state)?,
                            Some(Reason::RequiresStateRecovery),
                        )?;
                    }
                    continue;
                }
                state.validate()?;
                let result = match &state
                    .key
                    .as_ref()
                    .ok_or_else(|| Error::conflict("missing scoped state"))?
                    .target
                {
                    StateTarget::Ledger { .. } => ledger::project(&tx, state, &ledger),
                    StateTarget::Adoption { .. } => adoptions::project(&tx, context, state),
                    StateTarget::Confirmed { .. } => {
                        objects::confirmed(&tx, context, state, &ledger)
                    }
                    StateTarget::Mutation { .. } | StateTarget::MutationDisposition { .. } => {
                        mutations::project(&tx, context, state, &scoped)
                    }
                    _ => related::project(&tx, context, state, &ledger),
                }?;
                retained::reason(&tx, context.root, &retained::state_key(state)?, result)?;
            }
            let status = retained::status(&tx, context.root)?;
            tx.commit().map_err(storage)?;
            Ok(status)
        })
    }
}

fn authorized(state: &DurableStateChange, context: SyncRecoveryContext) -> bool {
    matches!(state.key.as_ref().map(|key| &key.namespace), Some(StateNamespace::Bound { binding }) if *binding==context.verified_checkpoint.binding)
}

fn validate_session(c: &Connection, context: SyncRecoveryContext) -> Result<(), Error> {
    let binding = context.verified_checkpoint.binding;
    let role = match context.role {
        RecoverySessionRole::Host => "host",
        RecoverySessionRole::Replica => "replica",
    };
    let matches: bool = c.query_row(
        "SELECT EXISTS(SELECT 1 FROM sync_binding b JOIN sync_active_context a ON a.local_root_uuid=b.local_root_uuid JOIN library_roots r ON r.uuid=b.local_root_uuid JOIN device_state d ON d.key='device_id' JOIN library_projection_watermarks w ON w.library_root_uuid=r.uuid WHERE b.singleton=1 AND a.singleton=1 AND r.is_active=1 AND b.local_root_uuid=?1 AND b.library_id=?2 AND b.authority_id=?3 AND b.epoch=?4 AND b.role=?5 AND d.value=?6 AND w.library_id=?7)",
        params![context.root.to_string(), binding.library_id.as_uuid().to_string(),binding.authority_id.as_uuid().to_string(),binding.epoch.as_u64(),role,context.local_author.as_uuid().to_string(),context.local_library_id.to_string()],
        |r| r.get(0),
    ).map_err(storage)?;
    if !matches
        || context.local_author.as_uuid().is_nil()
        || context.local_library_id.as_uuid().is_nil()
        || binding.library_id.as_uuid().is_nil()
        || binding.authority_id.as_uuid().is_nil()
        || binding.epoch.as_u64() == 0
        || (context.verified_checkpoint.sequence.as_u64() == 0
            && context.verified_checkpoint.digest != [0; 32])
    {
        return Err(Error::conflict(
            "sync recovery requires the current validated session and directory identity",
        ));
    }
    Ok(())
}
fn text<'a>(value: &'a Value, name: &str) -> Result<&'a str, Error> {
    value
        .get(name)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::conflict(format!("invalid sync recovery field {name}")))
}
fn number(value: &Value, name: &str) -> Result<u64, Error> {
    value
        .get(name)
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::conflict(format!("invalid sync recovery number {name}")))
}
fn body(value: &Value, name: &str) -> Result<String, Error> {
    serde_json::to_string(&value[name]).map_err(storage)
}
fn schema(state: &DurableStateChange, expected: &str) -> bool {
    state.payload.get("schema").and_then(Value::as_str) == Some(expected)
}
