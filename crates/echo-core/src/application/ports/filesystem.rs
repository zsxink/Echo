use super::*;

#[derive(Debug)]
pub struct FileMeta {
    pub size: u64,
    pub modified_ns: i64,
}

/// Opaque handle to one user-selected external import source (design §8: the
/// "受桌面可信边界保护的外部源定位"). The desktop layer resolves the handle to
/// the real file it offered the user to pick; the handle itself is a logical,
/// non-path key so Core results and errors never carry an absolute location.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct ImportSource {
    key: String,
}

impl ImportSource {
    /// Construct a source handle from the desktop-side logical key.
    ///
    /// # Errors
    ///
    /// [`crate::error::Error::Validation`] when the key is empty or carries
    /// path syntax — sources are identified logically, never by location.
    pub fn new(key: impl Into<String>) -> Result<Self, Error> {
        let key = key.into();
        if key.is_empty() || key.contains(['/', '\\', '\0']) || key == "." || key == ".." {
            return Err(Error::validation(
                crate::error::Subject::Other,
                "ImportSource",
                "source key must be a non-path logical identifier",
            ));
        }
        Ok(Self { key })
    }

    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }
}

/// What the reader knows about a source without reading its content.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportSourceInfo {
    /// The file's display name (e.g. `晴天.flac`) — a name, never a path.
    pub display_name: String,
    /// Content size in bytes as observed by the reader.
    pub size: u64,
}

/// A same-basename `.lrc` sidecar beside an import source (task 5.4). The
/// reader resolves the sibling by the real path it owns (extension matching
/// is the filesystem's job — case-insensitive on macOS/Windows); Core only
/// ever sees the name and size.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SidecarInfo {
    /// The sidecar's display name (e.g. `晴天.lrc`) — a name, never a path.
    pub display_name: String,
    /// Content size in bytes as observed by the reader.
    pub size: u64,
}

/// Reads user-selected external import sources. Implemented by the layer that
/// owns the file-selection result (the desktop trusted boundary) and by test
/// doubles; Core only ever sees handles and content streams, never locations.
pub trait ImportSourceReader: Send + Sync {
    /// Describe a source (display name + size) without reading its content.
    fn describe(&self, source: &ImportSource) -> Result<ImportSourceInfo, Error>;
    /// A reader over the source's full content (design §8: 逐资源源定位).
    /// The import streams this reader straight into the controlled staging
    /// directory, so implementations must serve the content from its
    /// beginning to its end; a vanished or truncated source surfaces as a
    /// short stream and is rejected by the size verification.
    fn open<'a>(&'a self, source: &ImportSource) -> Result<Box<dyn Read + 'a>, Error>;
    /// Describe the same-basename `.lrc` beside `source` (spec: 扩展名大小写
    /// 不敏感), if one exists. `Ok(None)` = no sidecar candidate. Errors are
    /// the reader's own unavailability (unreadable library, vanished
    /// directory…).
    fn sidecar(&self, source: &ImportSource) -> Result<Option<SidecarInfo>, Error>;
    /// Open the sidecar's content (design §8: 每个资源有独立源定位). `Ok(None)`
    /// = no sidecar; `Err` = one exists but cannot be read (permission, is a
    /// directory, vanished mid-selection…) and is reported as a lyrics-failed
    /// result, never a fake full success.
    fn open_sidecar<'a>(
        &'a self,
        source: &ImportSource,
    ) -> Result<Option<Box<dyn Read + 'a>>, Error>;
}

/// The verbatim result of streaming one resource into the controlled staging
/// directory: bytes written, the BLAKE3 accumulated during the copy, and the
/// staged file's root-relative location (journal bookkeeping only — never a
/// user-facing path).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct StagedCopy {
    /// Bytes actually copied into the staging file.
    pub size: u64,
    /// BLAKE3 hex digest of the staged content.
    pub blake3: String,
    /// The staged file's location relative to the library root.
    pub staged_path: RelativeMediaPath,
}

/// Opaque handle to a file already placed in Echo's marker-verified staging
/// directory. It intentionally contains no filesystem path: an adapter must
/// resolve it below the operation's owned staging directory and reject unknown
/// or forged handles before publishing.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct StagedResource {
    operation: OperationId,
    resource_key: String,
}

impl StagedResource {
    /// Construct an application-level handle for one operation resource.
    ///
    /// `resource_key` is a logical item key, never a filesystem path. Adapters
    /// map it to their private, marker-verified staging location.
    pub fn new(operation: OperationId, resource_key: impl Into<String>) -> Result<Self, Error> {
        let resource_key = resource_key.into();
        if resource_key.is_empty()
            || resource_key.contains(['/', '\\', '\0'])
            || resource_key == "."
            || resource_key == ".."
        {
            return Err(Error::validation(
                crate::error::Subject::Path,
                "StagedResource",
                "resource key must be a non-path logical identifier",
            ));
        }
        Ok(Self {
            operation,
            resource_key,
        })
    }

    #[must_use]
    pub const fn operation(&self) -> OperationId {
        self.operation
    }

    #[must_use]
    pub fn resource_key(&self) -> &str {
        &self.resource_key
    }
}

/// The library file system — always root-constrained operations. The adapter
/// resolves a root's absolute path internally; use cases only pass
/// [`RelativeMediaPath`].
/// Internal adapter-completeness contract. Public use cases consume the
/// focused capabilities exposed from `filesystem_capabilities` instead.
pub trait LegacyLibraryFileSystem: Send + Sync {
    /// Enumerate supported files under `root`. Follows no symlinks (leaks out
    /// of the root are rejected by the adapter).
    fn enumerate(&self, root: LibraryRootId) -> Result<Vec<RelativeMediaPath>, Error>;
    /// Metadata of one file (size + mtime) for scan fast-skip.
    fn file_meta(&self, root: LibraryRootId, path: &RelativeMediaPath) -> Result<FileMeta, Error>;
    /// Read up to `limit` bytes (metadata/tag reads).
    fn read_head(
        &self,
        root: LibraryRootId,
        path: &RelativeMediaPath,
        limit: u64,
    ) -> Result<Vec<u8>, Error>;
    /// Atomically publish an adapter-owned staging resource into its final
    /// root-relative path. The publication reserves the target exclusively
    /// (create-new: any existing entry — including a symlink — is a conflict,
    /// never a replacement), then renames the staged file onto the reserved
    /// name (design §8: 新建 exclusive 目标 + fsync + rename 且绝不替换).
    /// Callers cannot pass an arbitrary external path.
    fn publish(
        &self,
        root: LibraryRootId,
        staged: &StagedResource,
        target: &RelativeMediaPath,
    ) -> Result<(), Error>;
    /// Place content into the operation's adapter-owned staging area so it
    /// can be published to its target afterwards (the ingestion entry the
    /// import use case drives; task 5.1). The adapter decides the physical
    /// location — use cases only hold the [`StagedResource`] handle.
    fn stage(
        &self,
        root: LibraryRootId,
        staged: &StagedResource,
        content: &[u8],
    ) -> Result<(), Error>;
    /// Stream-copy `content` into the operation's marker-verified staging
    /// directory (task 5.3: 流式复制+BLAKE3). Implementations pump the reader
    /// in bounded chunks into an exclusively created file inside the owned
    /// staging slot, accumulate BLAKE3 while copying, fsync the staged file
    /// and only then report it as staged. The returned [`StagedCopy`] is the
    /// evidence the journal records (per-resource source/staging/hash).
    fn stage_stream(
        &self,
        root: LibraryRootId,
        staged: &StagedResource,
        content: &mut dyn Read,
    ) -> Result<StagedCopy, Error>;
    /// Read a staged resource back through its handle (the import parses the
    /// staged copy's tags before a target name exists). Unknown or forged
    /// handles are rejected.
    fn read_staged(&self, root: LibraryRootId, staged: &StagedResource) -> Result<Vec<u8>, Error>;
    /// Best-effort removal of a staged resource (failure-path cleanup; the
    /// operation's journal keeps the diagnostic). Idempotent: discarding an
    /// unknown handle or an already removed file succeeds.
    fn discard_staged(&self, root: LibraryRootId, staged: &StagedResource) -> Result<(), Error>;
    /// Safely remove one *persisted* staged file by its root-relative path
    /// (task 5.5/5.10 recovery rollback: 无完整暂存且未发布则清理安全残留并回滚).
    /// Like [`Self::publish_from_staging_path`], the adapter must verify the
    /// path resolves inside Echo's own marker-verified staging area — a foreign
    /// path is refused, never deleted. Idempotent: an absent file succeeds.
    fn discard_staging_path(
        &self,
        root: LibraryRootId,
        staging_path: &RelativeMediaPath,
    ) -> Result<(), Error>;
    /// Best-effort removal of an adapter-published *final* file at `target` (a
    /// root-relative library path, never a staging path). Used by the import
    /// pre-commit dedup (task 5.6): when the content hash already belongs to
    /// another song record, the import removes the redundant duplicate file it
    /// just published so identical content never leaves a second library file
    /// (绝不复制/绝无重复文件) while returning the existing record. The caller
    /// has first verified the file's hash equals the duplicate content's hash,
    /// so no unique data is removed. The adapter must refuse symlink/reparse
    /// targets (never follow) and must succeed idempotently whether or not the
    /// file exists; only a real regular file is removed.
    fn discard_published(
        &self,
        root: LibraryRootId,
        target: &RelativeMediaPath,
    ) -> Result<(), Error>;
    /// Whether a root-relative path currently exists (recovery's three-location
    /// check). `Ok(false)` is "not present", distinct from an I/O error in
    /// checking the filesystem itself.
    fn path_exists(&self, root: LibraryRootId, path: &RelativeMediaPath) -> Result<bool, Error>;
    /// Publish a file already staged at the persisted, root-relative
    /// `staging_path` into `target` with the same exclusive create-new +
    /// fsync + rename contract as [`Self::publish`] (task 5.5 recovery: 只有
    /// 暂存正确则重试 exclusive publish). Unlike [`Self::publish`] this does
    /// NOT resolve an in-memory staged handle — it reads the *journal's*
    /// persisted staging location, which is what survives a crash — so the
    /// adapter must verify `staging_path` still resolves inside Echo's own
    /// marker-verified staging area before publishing (never a foreign path).
    fn publish_from_staging_path(
        &self,
        root: LibraryRootId,
        staging_path: &RelativeMediaPath,
        target: &RelativeMediaPath,
    ) -> Result<(), Error>;
    /// Resolve the root-relative trash slot for one delete-operation resource
    /// WITHOUT moving anything (design §9: 专属受控 `trash/<operation-id>`).
    /// The use case persists this location in the `StagePending` journal item
    /// *before* the rename, so a crash between the state write and the move
    /// still leaves a durable, resolvable staged location for recovery.
    /// `resource_key` is a logical per-resource item name (e.g. `audio` /
    /// `lyrics`), never a filesystem path. The adapter owns the slot layout;
    /// the returned path must be stable and match what [`Self::stage_to_trash`]
    /// later fills.
    fn trash_path(
        &self,
        root: LibraryRootId,
        operation: OperationId,
        resource_key: &str,
    ) -> Result<RelativeMediaPath, Error>;
    /// Move a *library-published* file at `source` into Echo's controlled
    /// `trash/<operation-id>` slot (design §9: 同盘 rename 移入专属受控目录的
    /// `trash/<operation-id>`), i.e. the delete stage. Like [`Self::publish`],
    /// the adapter owns the physical slot: `resource_key` is a logical
    /// per-resource item name (e.g. `audio` / `lyrics`), never a filesystem
    /// path, and the resolved trash file is created exclusively (an existing
    /// entry is a conflict, never replaced). `source` must be a real library
    /// file (a symlink/reparse target is refused). The move is a same-volume
    /// rename so the staged copy is atomically visible whole. Returns the
    /// root-relative trash path the journal records and that recovery/undo
    /// later resolve; an implementation must keep it stable for the
    /// operation's lifetime and equal to [`Self::trash_path`].
    fn stage_to_trash(
        &self,
        root: LibraryRootId,
        operation: OperationId,
        source: &RelativeMediaPath,
        resource_key: &str,
    ) -> Result<RelativeMediaPath, Error>;
    /// Move a file back out of the trash slot to a library `target` (design §9
    /// undo: 把文件移回原路径). The same exclusive create-new contract as a
    /// publish: a non-empty existing target is a conflict, never replaced — the
    /// use case retries against a safe numbered path. `trash` must resolve
    /// inside the owned `trash/` subdirectory; a symlink/reparse `target` is
    /// refused.
    fn restore_from_trash(
        &self,
        root: LibraryRootId,
        trash: &RelativeMediaPath,
        target: &RelativeMediaPath,
    ) -> Result<(), Error>;
    /// Whether the root currently permits writes (permissions + marker).
    fn write_capable(&self, root: LibraryRootId) -> Result<bool, Error>;
    /// Acquire write capability for `root` by establishing its owned staging
    /// directory (design §8: 首次获得写能力时 exclusive-create). Idempotent —
    /// a root that already has capability is a no-op. Called by root
    /// activation/prepare when a candidate root is brought writable, so a
    /// freshly-chosen directory is never permanently read-only.
    ///
    /// # Errors
    ///
    /// Propagates the inability to create/verify the staging directory.
    fn establish_write_capability(&self, root: LibraryRootId) -> Result<(), Error>;
}
