//! Per-input multi-select import planning (`PlanImport`, tasks 5.1–5.4,
//! design §8).
//!
//! A user-selected batch is processed **one input at a time**: each input is
//! classified (imported / duplicate content / unsupported / failed), and the
//! inputs that proceed are planned as their own operation — a fresh
//! `OperationId` and a **reserved** `SongId`, persisted in the operation
//! journal together with the target-path claim *before* the first external
//! side effect (design §8: 先持久化意图，再执行调用，再持久化结果).
//!
//! The per-resource pipeline (task 5.3) streams every source straight into
//! Echo's **exclusive, marker-verified staging directory**
//! (`<root>/.echo-staging-…/import/<operation-id>`): the copy runs in bounded
//! chunks with BLAKE3 accumulated during the copy, the staged file is fsynced
//! before it counts as staged, and the source file is read exactly once and
//! never modified. The target `media/歌手/歌手 - 歌曲名.原扩展名` (task 5.2;
//! all media published under the portable `media/` tree, task 3.1) is planned
//! from the tags of the staged copy, then the conditional unique
//! target claim plus the reserved identity are persisted before the publish;
//! the publish itself reserves the target exclusively (create-new) and swaps
//! the verified staged copy in with one same-filesystem rename, so the song
//! record — committed only afterwards — never points at a half file.
//!
//! Isolation is the contract: every input commits (or fails) on its own, so a
//! single failed input never rolls back the inputs already imported, and the
//! caller receives exactly one result per input, index-aligned with the batch.
//! A root that cannot accept writes refuses the whole batch *before* any copy
//! — every input then reports `LibraryUnavailable` (spec: 导入时根目录断开).
//!
//! A same-basename `.lrc` beside the source is an optional sub-resource of the
//! same operation (task 5.4): it is copied under the audio target's final base
//! name (extending to the `(n)` numbering), verified and published after the
//! audio is whole, and reported in the per-input result independently. A
//! sidecar failure — unreadable, size-mismatched or conflicting with an
//! incumbent `.lrc` — is "audio succeeded / lyrics failed": the audio record
//! commits under the reserved identity, the sidecar result carries the reason,
//! and no half or empty `.lrc` is ever left behind.
//!
//! Scope note: the full per-step journal chain
//! (`CopyPending → … → PublishApplied`, fault injection and the crash
//! recovery matrix) is task 5.5; this module persists the two points it owns —
//! the plan reservation and the terminal result — and verifies size/hash at
//! the published location before the database commit.

use crate::application::ports::{
    ImportSource, ImportSourceInfo, ImportSourceReader, OperationItem, OperationResourceKind,
    StagedResource, TxAccess,
};
use crate::application::relink::song_from_parsed;
use crate::application::scan::{
    parse_single_file, rewrap, FileOutcome, ParsedOutcome, ScanDeps, SUPPORTED_EXTENSIONS,
};
use crate::domain::entities::LyricsSource;
use crate::domain::ids::{LibraryRootId, OperationId, RelativeMediaPath, SongId};
use crate::domain::import::ImportConflictIndex;
use crate::domain::library::{PortableRecord, MEDIA_ROOT};
use crate::domain::state::OperationState;
use crate::domain::text::{
    target_artist_component, target_file_stem, truncate_component_with_extension,
};
use crate::error::{Error, Subject};

/// The one staging resource every import audio operation owns.
const IMPORT_AUDIO_RESOURCE: &str = "audio";
/// The optional same-basename `.lrc` sub-resource of an import (task 5.4).
const IMPORT_LRC_RESOURCE: &str = "lyrics";
/// The `operation` classifier used by import verification errors.
const IMPORT_OPERATION: &str = "import";
/// Conservative single-component byte cap for planned targets (the OS allows
/// 255 on the major desktop filesystems). Both the artist directory and the
/// `artist - title` file component are bounded by it; over-long components
/// keep the extension plus the short-hash suffix (domain truncation rule).
const TARGET_COMPONENT_BYTES: usize = 200;
/// Upper bound for the minimal ` (n)` conflict numbering search.
const MAX_NUMBERING: u64 = 4_096;

/// The sidecar-lyrics result of one imported audio input (task 5.4).
///
/// The audio is the main resource: every variant here describes what happened
/// to the *optional* same-basename `.lrc` — never whether the audio imported.
/// A failure is reported separately and never faked as full success (spec:
/// 歌词复制失败 MUST 单独报告并不得伪装为完整成功).
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LyricsImportResult {
    /// The source had no same-basename `.lrc`; no sidecar was created.
    None,
    /// The same-basename `.lrc` was copied, verified and published beside the
    /// audio under the audio's final base name.
    Imported {
        /// The root-relative published sidecar path.
        target: RelativeMediaPath,
    },
    /// A same-basename `.lrc` existed but could not be imported; the audio
    /// import still succeeded (spec: 音频成功/歌词失败). No sidecar file was
    /// left behind. The message is user-safe.
    Failed {
        /// The stable machine code of the underlying error.
        code: &'static str,
        /// The redacted, user-presentable explanation.
        message: String,
    },
}

/// The result of one batch input, index-aligned with the batch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ImportOutcome {
    /// Copied, verified, published and committed under the reserved identity.
    Imported {
        /// The journal operation that performed this import.
        operation: OperationId,
        /// The reserved `SongId` the record was committed with.
        song: SongId,
        /// The final root-relative path of the published audio.
        target: RelativeMediaPath,
        /// What happened to the optional same-basename `.lrc` (boxed to keep
        /// the batch result cheap — the sidecar result is `Copy`-free).
        lyrics: Box<LyricsImportResult>,
    },
    /// The content hash already belongs to a library record — no copy, no
    /// second UUID; the caller points the user at the existing song.
    Duplicate { existing: SongId },
    /// The input is not an importable audio type (extension matrix).
    Unsupported,
    /// The library root could not accept writes; the whole batch was refused
    /// before any copy (spec: 资料库不可用 → 开始复制前拒绝整批).
    LibraryUnavailable,
    /// This input failed. The message is user-safe: Core errors redact
    /// absolute paths before display.
    Failed {
        /// The stable machine code of the underlying error.
        code: &'static str,
        /// The redacted, user-presentable explanation.
        message: String,
    },
}

/// One batch's per-input results, index-aligned with the inputs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ImportBatchReport {
    pub results: Vec<ImportOutcome>,
}

/// Plans and executes one multi-select import batch, one input at a time.
///
/// Blocking (file copies, hashing, parse): the runtime calls it on a worker
/// thread, exactly like [`crate::application::scan::StartScan`].
pub struct PlanImport<'a> {
    deps: &'a ScanDeps,
    sources: &'a dyn ImportSourceReader,
}

/// Mutable per-batch identity state: hashes and target keys claimed by the
/// library snapshot plus the inputs committed so far in this batch.
struct BatchState {
    conflicts: ImportConflictIndex,
}

/// The planned optional same-basename `.lrc` sub-resource of one input.
struct PlannedLrc {
    /// The sidecar's target: the audio target's directory + stem + `.lrc`,
    /// so a successful sidecar always pairs with the audio's final base name.
    target: RelativeMediaPath,
    /// The staged handle (resource key `lyrics`) in the operation's slot.
    staged: StagedResource,
    /// The logical source locator of the sidecar (never a path).
    source: Option<String>,
    staged_path: RelativeMediaPath,
    hash: String,
    size: u64,
}

/// A sidecar that existed but could not be copied/staged — "audio succeeds,
/// lyrics failed" (the staged audio proceeds; only the LRC result is Failed).
struct LyricsFailure {
    code: &'static str,
    message: String,
}

/// How an execution failure must be handled (task 5.5 regime, design §8).
///
/// The boundary that changes the answer is the **publish**: once a final file
/// has been durably placed at its target, the operation can no longer be
/// rolled back — a rollback would orphan the published file and free the
/// target claim for someone else. So a failure before the publish is
/// [`ExecuteError::PrePublish`] (safe to roll back and release the claim),
/// while a failure at/after the publish is [`ExecuteError::PostPublish`] (the
/// item is left recoverable and the claim is held until recovery completes it
/// under the same reserved identity).
enum ExecuteError {
    /// No final file has been published yet — safe to roll back.
    PrePublish(Error),
    /// A published final file (or a DB commit in flight) means the operation
    /// must be completed by recovery, never rolled back.
    PostPublish(Error),
    /// The *pre-commit* BLAKE3 dedup re-check (task 5.6) found that the content
    /// we just published already belongs to another song record — a concurrent
    /// import or watcher committed the same content between the plan-time check
    /// and our own commit. Identical content must produce exactly one logical
    /// song: we contribute nothing (no second UUID, no second file) and return
    /// the existing record.
    Duplicate { existing: SongId },
}

#[path = "batch.rs"]
mod batch;
#[path = "execution.rs"]
mod execution;
#[path = "planning.rs"]
mod planning;
#[path = "report.rs"]
mod report;

#[cfg(test)]
include!("tests.rs");

/// One input's reserved plan: the identity, staged resource and target every
/// later step (and any recovery, in task 5.5) must agree on.
struct PlannedInput {
    root: LibraryRootId,
    target: RelativeMediaPath,
    operation: OperationId,
    reserved: SongId,
    /// The logical source locator recorded in the journal item (never a path).
    source: Option<String>,
    /// The staged resource handle driving the controlled staging directory.
    staged: StagedResource,
    /// The staged file's root-relative location (journal bookkeeping).
    staged_path: RelativeMediaPath,
    hash: String,
    size: u64,
    /// The planned sidecar sub-resource, when the source has a same-basename
    /// `.lrc` that could be read and staged.
    lrc: Option<PlannedLrc>,
    /// A sidecar that existed but failed before staging (unreadable source,
    /// size mismatch, no safe target). The audio still imports.
    lrc_failure: Option<LyricsFailure>,
}
