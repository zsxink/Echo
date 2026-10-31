//! Restartable, recoverable isolation of files hidden by remote tombstones.

use super::scan::ScanDeps;
use crate::domain::ids::LibraryRootId;
use crate::domain::ids::OperationId;
use crate::error::Error;

pub type RemoteDeleteResults = Vec<(OperationId, Result<(), Error>)>;

/// Each failed item keeps its original intent for the next retry. An externally
/// changed source is left in place and never moved under the stale declaration.
///
/// # Errors
/// Returns an error when resource holds or the durable isolation intents cannot
/// be read. Per-file failures are returned beside their operation identities.
pub fn recover_remote_deletes(
    deps: &ScanDeps,
    root: LibraryRootId,
) -> Result<RemoteDeleteResults, Error> {
    let _activity = deps.activity.enter()?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::ports::filesystem::LegacyLibraryFileSystem;
    use crate::application::ports::RemoteDeleteIsolation;
    use crate::application::testing::scan_fixture::ScanFixture;
    use crate::domain::sync::ResourceDigest;

    fn isolation(source: &str, bytes: &[u8]) -> RemoteDeleteIsolation {
        RemoteDeleteIsolation {
            operation: OperationId::new(),
            source: crate::domain::ids::RelativeMediaPath::new(source).expect("relative path"),
            digest: ResourceDigest::from_bytes(*blake3::hash(bytes).as_bytes()),
        }
    }

    /// A tombstoned song's audio is already hidden from the catalog either
    /// when the host published the tombstone (the projection writes
    /// `availability='pending_delete'` before this retry pass ever runs) or
    /// because the file was externally removed with the source row. Here the
    /// intent is durable and the file is still on disk — the recovery must
    /// move it into Echo's controlled trash, retain an immutable copy with a
    /// receipt and finish the intent, in that order, so a crash between any
    /// two steps replays safely.
    #[test]
    fn recovery_stages_retains_and_finishes_a_pending_isolation() {
        let fixture = ScanFixture::new();
        let bytes = b"remote-deleted audio";
        fixture.write_file("media/gone.flac", bytes);
        let intent = isolation("media/gone.flac", bytes);
        fixture
            .database
            .seed_remote_delete_isolation(intent.clone(), fixture.root);

        let results = recover_remote_deletes(&fixture.deps, fixture.root).expect("recovery");
        assert_eq!(results.len(), 1);
        assert!(results[0].1.is_ok(), "the intent is complete: {results:?}");
        assert!(
            fixture
                .database
                .pending_remote_delete_isolations(fixture.root)
                .is_empty(),
            "the finished intent must not survive as pending"
        );
        // The source left the media tree into the controlled trash slot.
        assert!(
            !LegacyLibraryFileSystem::path_exists(&fixture.fs, fixture.root, &intent.source)
                .expect("probe"),
            "the source must no longer exist in the media tree after staging to trash"
        );
        // The retained receipt is durable and points at the retained copy.
        let retained = fixture
            .deps
            .sync
            .retained_resource(fixture.root, intent.digest)
            .expect("receipt read")
            .expect("retained receipt");
        assert_eq!(
            retained.path.normalized(),
            format!("echo/media/retained/{}", intent.digest.to_hex())
        );
    }

    /// A pending intent whose file is gone in *both* places has no isolation
    /// work: the recovery keeps it pending (no fabricated finish, no retained
    /// receipt, no catalog write) so a later controlled pass retries it.
    #[test]
    fn an_intent_with_no_file_anywhere_stays_pending_without_fabricating_a_result() {
        let fixture = ScanFixture::new();
        let intent = isolation("media/vanished.flac", b"vanished");
        fixture
            .database
            .seed_remote_delete_isolation(intent.clone(), fixture.root);

        let results = recover_remote_deletes(&fixture.deps, fixture.root).expect("recovery");
        assert_eq!(results.len(), 1);
        assert!(
            results[0].1.is_err(),
            "the absent target must not fabricate success"
        );
        assert_eq!(
            fixture
                .database
                .pending_remote_delete_isolations(fixture.root),
            vec![intent.operation],
            "the intent stays pending for a later retry"
        );
        assert!(
            fixture
                .deps
                .sync
                .retained_resource(fixture.root, intent.digest)
                .expect("receipt read")
                .is_none(),
            "no receipt is recorded for a file that was never isolated"
        );
    }

    /// A source that no longer matches the intent's declared digest is left
    /// in place (never moved under a stale declaration) and reported conflict.
    #[test]
    fn an_externally_changed_source_is_left_in_place_and_reported() {
        let fixture = ScanFixture::new();
        fixture.write_file("media/replaced.flac", b"different content");
        let intent = isolation("media/replaced.flac", b"original remote audio");
        fixture
            .database
            .seed_remote_delete_isolation(intent.clone(), fixture.root);

        let results = recover_remote_deletes(&fixture.deps, fixture.root).expect("recovery");
        assert_eq!(results.len(), 1);
        assert!(results[0].1.is_err(), "conflict, not silent success");
        assert!(
            LegacyLibraryFileSystem::path_exists(&fixture.fs, fixture.root, &intent.source)
                .expect("probe"),
            "an externally changed file is never moved by recovery"
        );
        assert_eq!(
            fixture
                .database
                .pending_remote_delete_isolations(fixture.root),
            vec![intent.operation]
        );
    }
}
