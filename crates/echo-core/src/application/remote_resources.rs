//! Downloaded bytes become playable only after journaled publication of the
//! complete object resource set. Interrupted I/O keeps the journal retryable.

#![allow(clippy::missing_errors_doc)]

use std::io::Read;
use std::sync::{Arc, Mutex};

use crate::application::ports::{
    ContentHasher, LibraryFileSystem, RemoteResourceItem, RemoteResourceJournalPort,
    RemoteResourcePublication, RemoteResourceStage, RemoteResourceTarget, StagedResource,
};
use crate::domain::ids::{LibraryRootId, OperationId, RelativeMediaPath};
use crate::domain::sync::ObjectKey;
use crate::error::{Error, Subject};

// Serialize file/journal transitions even if callers construct separate use
// cases. This synchronous boundary never holds a lock across an await.
static PUBLICATION_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone)]
pub struct RemoteResourcePublishDeps {
    pub fs: Arc<dyn LibraryFileSystem>,
    pub hasher: Arc<dyn ContentHasher>,
    pub journal: Arc<dyn RemoteResourceJournalPort>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RemotePublicationOutcome {
    pub media_ready: bool,
    pub remaining_resources: usize,
}

pub type RemoteRecoveryResults = Vec<(OperationId, Result<RemotePublicationOutcome, Error>)>;

pub struct PublishRemoteResources {
    deps: RemoteResourcePublishDeps,
}

impl PublishRemoteResources {
    #[must_use]
    pub const fn new(deps: RemoteResourcePublishDeps) -> Self {
        Self { deps }
    }

    pub fn prepare(
        &self,
        root: LibraryRootId,
        operation: OperationId,
        object: ObjectKey,
        resources: Vec<RemoteResourceTarget>,
    ) -> Result<(), Error> {
        let _guard = publication_lock()?;
        if resources.is_empty() || resources.len() > 32 {
            return Err(Error::validation(
                Subject::Other,
                "resources",
                "expected 1..=32 resources",
            ));
        }
        let group =
            blake3::hash(
                &serde_json::to_vec(&resources).map_err(|source| Error::Storage {
                    what: "resource declaration encoding".to_owned(),
                    source: Box::new(source),
                })?,
            )
            .to_hex();
        let (kind, id) = object_local_key(object);
        let mut paths = std::collections::HashSet::new();
        let items = resources
            .into_iter()
            .enumerate()
            .map(|(index, target)| {
                let path = RelativeMediaPath::new(target.manifest.relative_path.as_str())?;
                if !paths.insert(path.identity_key().to_owned()) {
                    return Err(Error::conflict("duplicate normalized resource destination"));
                }
                let staging_path = format!(
                    "{}/{}/{index}",
                    crate::domain::library::STAGING_ROOT,
                    operation.as_uuid().simple()
                );
                // Retain the logical audio/lyrics path on first download, or
                // adopt it when it already contains this exact version after
                // a local tag edit. This keeps a rescan from importing the
                // same edited file as a second song. Conflicting content stays
                // in an isolated Echo version path.
                let local_path = if matches!(object, ObjectKey::Song(_))
                    && matches!(
                        target.manifest.kind,
                        crate::domain::sync::SharedResourceKind::Audio
                            | crate::domain::sync::SharedResourceKind::LyricsSidecar
                    )
                    && ((target.expected_old_digest.is_none()
                        && !self.deps.fs.path_exists(root, &path)?)
                        || self.matches(
                            root,
                            &path,
                            &target.manifest.digest.to_hex(),
                            target.manifest.length,
                        )?) {
                    path.normalized().to_owned()
                } else {
                    format!("echo/media/{kind}/{id}/{group}/{}", path.normalized())
                };
                RelativeMediaPath::new(&local_path)?;
                Ok(RemoteResourceItem {
                    target,
                    stage: RemoteResourceStage::Planned,
                    staging_path,
                    local_path,
                    old_path: None,
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        self.deps.journal.prepare(&RemoteResourcePublication {
            root,
            operation,
            object,
            items,
            completed: false,
        })
    }

    /// Stream private blob content through the constrained library staging
    /// adapter. Full length and BLAKE3 are independently checked by Core.
    pub fn stage(
        &self,
        root: LibraryRootId,
        operation: OperationId,
        index: usize,
        content: &mut dyn Read,
    ) -> Result<(), Error> {
        let _guard = publication_lock()?;
        let mut publication = self.deps.journal.load(root, operation)?;
        let item = publication
            .items
            .get_mut(index)
            .ok_or_else(|| Error::validation(Subject::Other, "resource index", "out of bounds"))?;
        if item.stage != RemoteResourceStage::Planned {
            return Ok(());
        }
        let staged = StagedResource::new(operation, index.to_string())?;
        let copy = self.deps.fs.stage_stream(root, &staged, content)?;
        if copy.staged_path.normalized() != item.staging_path
            || copy.size != item.target.manifest.length
            || copy.blake3 != item.target.manifest.digest.to_hex()
        {
            self.deps.fs.discard_staged(root, &staged)?;
            return Err(Error::conflict(
                "downloaded resource failed full length or BLAKE3 validation",
            ));
        }
        item.stage = RemoteResourceStage::Validated;
        self.deps.journal.save(&publication)
    }

    /// Resume after a restart or file-in-use failure. Old versions stay in
    /// immutable version locations and are never automatically deleted.
    pub fn resume(
        &self,
        root: LibraryRootId,
        operation: OperationId,
    ) -> Result<RemotePublicationOutcome, Error> {
        let _guard = publication_lock()?;
        let mut publication = self.deps.journal.load(root, operation)?;
        if publication.completed {
            for item in &publication.items {
                if !self.matches(
                    root,
                    &RelativeMediaPath::new(&item.local_path)?,
                    &item.target.manifest.digest.to_hex(),
                    item.target.manifest.length,
                )? {
                    return Err(Error::conflict(
                        "completed local resource changed externally",
                    ));
                }
            }
            self.cleanup_completed_staging(&publication);
            return Ok(RemotePublicationOutcome {
                media_ready: true,
                remaining_resources: 0,
            });
        }
        for index in 0..publication.items.len() {
            if let Err(error) = self.publish_item(&mut publication, index) {
                if !self.deps.fs.is_publication_blocked(&error) {
                    return Err(error);
                }
                // The validated staging path and previous association are
                // durable. Keep this item pending and allow other items to
                // progress without pretending this object is media-ready.
            }
        }
        let remaining_resources = publication
            .items
            .iter()
            .filter(|item| item.stage != RemoteResourceStage::Published)
            .count();
        if remaining_resources == 0 {
            for item in &publication.items {
                let resource = &item.target.manifest;
                if !self.matches(
                    root,
                    &RelativeMediaPath::new(&item.local_path)?,
                    &resource.digest.to_hex(),
                    resource.length,
                )? {
                    return Err(Error::conflict(
                        "resource version changed before atomic association",
                    ));
                }
                if let (Some(previous), Some(expected)) =
                    (&item.old_path, item.target.expected_old_digest)
                {
                    let previous = RelativeMediaPath::new(previous)?;
                    if self.deps.hasher.hash(root, &previous)? != expected.to_hex()
                        // A local tag edit can already have transformed the
                        // previous file into exactly this declared version.
                        // Accept only full verified target content; arbitrary
                        // external changes still keep the association blocked.
                        && !self.matches(
                            root,
                            &previous,
                            &resource.digest.to_hex(),
                            resource.length,
                        )?
                    {
                        return Err(Error::conflict(
                            "previous local resource changed externally",
                        ));
                    }
                }
            }
            self.deps.journal.finish(&publication)?;
            self.cleanup_completed_staging(&publication);
        }
        Ok(RemotePublicationOutcome {
            media_ready: remaining_resources == 0,
            remaining_resources,
        })
    }

    /// Each result is independent: one occupied or externally changed file
    /// cannot hide recovery progress for another operation.
    pub fn recover(&self, root: LibraryRootId) -> Result<RemoteRecoveryResults, Error> {
        Ok(self
            .deps
            .journal
            .incomplete(root)?
            .into_iter()
            .map(|operation| (operation, self.resume(root, operation)))
            .collect())
    }

    fn cleanup_completed_staging(&self, publication: &RemoteResourcePublication) {
        // A completed association proves another verified copy exists. Only
        // remove an exact redundant copy named by that journal: unknown
        // operations, conflicting bytes and controlled trash remain intact.
        for item in &publication.items {
            let result = (|| {
                let staging = RelativeMediaPath::new(&item.staging_path)?;
                if self.matches(
                    publication.root,
                    &staging,
                    &item.target.manifest.digest.to_hex(),
                    item.target.manifest.length,
                )? {
                    self.deps
                        .fs
                        .discard_staging_path(publication.root, &staging)?;
                }
                Ok::<_, Error>(())
            })();
            if let Err(error) = result {
                // Cleanup is retryable on a later completed resume; it must
                // not turn an available, atomically associated set into an
                // apparent failed publication.
                tracing::warn!(
                    error_code = error.code(),
                    "completed resource staging cleanup deferred"
                );
            }
        }
    }

    fn publish_item(
        &self,
        publication: &mut RemoteResourcePublication,
        index: usize,
    ) -> Result<(), Error> {
        let root = publication.root;
        let item = &publication.items[index];
        let target = RelativeMediaPath::new(&item.local_path)?;
        let staging = RelativeMediaPath::new(&item.staging_path)?;
        let expected = item.target.manifest.digest.to_hex();
        if self.matches(root, &target, &expected, item.target.manifest.length)? {
            publication.items[index].stage = RemoteResourceStage::Published;
            self.deps.journal.save(publication)?;
            return Ok(());
        }
        if item.stage == RemoteResourceStage::Published {
            return Err(Error::conflict("published resource changed externally"));
        }
        // A crash after staging but before the state write can be recovered
        // from the durable planned path, without accepting a partial file.
        if !self.matches(root, &staging, &expected, item.target.manifest.length)? {
            return Ok(());
        }
        if self.deps.fs.path_exists(root, &target)? {
            return Err(Error::conflict(
                "immutable resource version contains foreign content",
            ));
        }
        let staged = StagedResource::new(publication.operation, index.to_string())?;
        match self.deps.fs.publish(root, &staged, &target) {
            Err(Error::Permission {
                kind: crate::error::PermKind::NotOwner,
                ..
            }) => {
                self.deps
                    .fs
                    .publish_from_staging_path(root, &staging, &target)?;
            }
            result => result?,
        }
        if !self.matches(
            root,
            &target,
            &expected,
            publication.items[index].target.manifest.length,
        )? {
            return Err(Error::conflict(
                "published resource failed full verification",
            ));
        }
        publication.items[index].stage = RemoteResourceStage::Published;
        self.deps.journal.save(publication)
    }

    fn matches(
        &self,
        root: LibraryRootId,
        path: &RelativeMediaPath,
        hash: &str,
        length: u64,
    ) -> Result<bool, Error> {
        if !self.deps.fs.path_exists(root, path)? {
            return Ok(false);
        }
        Ok(self.deps.fs.file_meta(root, path)?.size == length
            && self.deps.hasher.hash(root, path)? == hash)
    }
}

fn publication_lock() -> Result<std::sync::MutexGuard<'static, ()>, Error> {
    PUBLICATION_LOCK
        .lock()
        .map_err(|_| Error::InvariantViolation {
            why: "resource publication lock poisoned".to_owned(),
        })
}

fn object_local_key(object: ObjectKey) -> (&'static str, String) {
    match object {
        ObjectKey::Song(id) => ("song", id.to_string()),
        ObjectKey::Playlist(id) => ("playlist", id.to_string()),
        ObjectKey::PlaylistItem(id) => ("playlist-item", id.to_string()),
        ObjectKey::LibraryRoot(id) => ("library-root", id.to_string()),
        ObjectKey::Favorite(id) => ("favorite", id.to_string()),
        ObjectKey::Override(id) => ("override", id.to_string()),
        ObjectKey::PlayStats(id) => ("play-stats", id.to_string()),
    }
}
