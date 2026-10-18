//! Restartable, recoverable isolation of files hidden by remote tombstones.

use super::scan::ScanDeps;
use crate::domain::ids::LibraryRootId;
use crate::domain::ids::OperationId;
use crate::error::Error;

/// Each failed item keeps its original intent for the next retry. An externally
/// changed source is left in place and never moved under the stale declaration.
pub fn recover_remote_deletes(
    deps: &ScanDeps,
    root: LibraryRootId,
) -> Result<Vec<(OperationId, Result<(), Error>)>, Error> {
    let _activity = super::sync::local_activity()?;
    deps.sync.rebuild_resource_holds(root)?;
    Ok(deps
        .sync
        .remote_delete_isolations(root)?
        .into_iter()
        .map(|intent| {
            let operation = intent.operation;
            let result = (|| {
                let target = deps.fs.trash_path(root, operation, "remote-deleted")?;
                if deps.fs.path_exists(root, &intent.source)? {
                    if deps.hasher.hash(root, &intent.source)? != intent.digest.to_hex() {
                        return Err(Error::conflict(
                            "remote deletion source was externally modified",
                        ));
                    }
                    deps.fs
                        .stage_to_trash(root, operation, &intent.source, "remote-deleted")?;
                }
                if !deps.fs.path_exists(root, &target)? {
                    // Missing files have no physical isolation work; keep the
                    // intent pending so a later controlled recovery can retry.
                    return Err(Error::unavailable(
                        "remote deletion",
                        "both source and isolation copy are absent",
                    ));
                }
                let retained =
                    deps.fs
                        .retain_trash_resource(root, &target, &intent.digest.to_hex())?;
                deps.sync.record_retained_resource(root, retained)?;
                deps.sync.finish_remote_delete_isolation(root, operation)
            })();
            (operation, result)
        })
        .collect())
}
