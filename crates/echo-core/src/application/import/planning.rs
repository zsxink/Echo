use crate::application::ports::{ImportSource, ImportSourceInfo, StagedResource};
use crate::domain::ids::{LibraryRootId, OperationId, RelativeMediaPath};
use crate::error::{Error, Subject};

use super::report::{failed_of, lrc_target_of, plan_named_target};
use super::{
    BatchState, ImportOutcome, LyricsFailure, PlanImport, PlannedInput, PlannedLrc,
    IMPORT_AUDIO_RESOURCE, IMPORT_LRC_RESOURCE, IMPORT_OPERATION,
};

impl PlanImport<'_> {
    pub(super) fn stage_and_plan(
        &self,
        root: LibraryRootId,
        source: &ImportSource,
        info: &ImportSourceInfo,
        ext: &str,
        state: &BatchState,
    ) -> Result<PlannedInput, ImportOutcome> {
        let discard = |staged: &StagedResource| {
            let _ = self.deps.fs.discard_staged(root, staged);
        };
        let operation = self.deps.ids.new_operation_id();
        let reserved = self.deps.ids.new_song_id();
        let staged = match StagedResource::new(operation, IMPORT_AUDIO_RESOURCE) {
            Ok(staged) => staged,
            Err(error) => return Err(failed_of(&error)),
        };
        // 流式复制+BLAKE3 (task 5.3): the source is opened once and streamed
        // straight into the operation's controlled staging slot; the hash is
        // computed while the copy runs and the staged file is fsynced.
        let copy = self
            .sources
            .open(source)
            .and_then(|mut reader| self.deps.fs.stage_stream(root, &staged, reader.as_mut()))
            .map_err(|error| {
                discard(&staged);
                failed_of(&error)
            })?;
        // A source that does not match its description (vanished or truncated
        // mid-selection) never enters the library.
        if copy.size != info.size {
            discard(&staged);
            return Err(failed_of(&Error::CorruptMedia {
                operation: IMPORT_OPERATION.to_owned(),
                reason: "source content does not match the described size".to_owned(),
            }));
        }
        // Plan-time BLAKE3 dedup (task 5.6 adds the pre-commit re-check):
        // identical content returns the existing record, never a second UUID.
        if let Some(existing) = state.conflicts.duplicate_of(&copy.blake3) {
            discard(&staged);
            return Err(ImportOutcome::Duplicate { existing });
        }
        // The target is named from the tags of the staged copy (task 5.2);
        // unparseable content fails this input instead of guessing a name.
        let outcome_on_error = |error: &Error| {
            discard(&staged);
            failed_of(error)
        };
        let bytes = self
            .deps
            .fs
            .read_staged(root, &staged)
            .map_err(|error| outcome_on_error(&error))?;
        let meta = self
            .deps
            .metadata
            .read_bytes(&bytes)
            .map_err(|error| outcome_on_error(&error))?;
        let target = plan_named_target(
            meta.artist.as_deref(),
            meta.title.as_deref(),
            ext,
            &mut |key| state.conflicts.target_is_occupied(key),
        )
        .ok_or_else(|| {
            outcome_on_error(&Error::validation(
                Subject::Path,
                "import target",
                "no safe unique target name for the parsed tags",
            ))
        })?;
        // Detect close-name conflict resolution (spec: 重名后成功): the
        // final target differs from the ideal no-conflict name.
        let renamed = plan_named_target(
            meta.artist.as_deref(),
            meta.title.as_deref(),
            ext,
            &mut |_| false,
        )
        .map_or(false, |ideal| ideal != target);
        // The optional same-basename `.lrc` sub-resource (task 5.4): plan it
        // around the FINAL audio target, so the sidecar pairs with the exact
        // base name the audio lands on (including the `(n)` numbering). A
        // sidecar problem never fails the audio — it becomes the input's
        // "audio succeeded / lyrics failed" result.
        let (lrc, lrc_failure) = self.plan_lrc(root, source, operation, &target);
        Ok(PlannedInput {
            root,
            target,
            operation,
            reserved,
            renamed,
            source: Some(source.key().to_owned()),
            staged,
            staged_path: copy.staged_path,
            hash: copy.blake3.clone(),
            size: copy.size,
            lrc,
            lrc_failure,
        })
    }

    /// Discover and stage the same-basename `.lrc` beside `source` after the
    /// audio target is final (spec: 使用与目标音频相同的基础文件名). Returns
    /// the planned sub-resource when usable, or a captured failure when the
    /// sidecar exists but cannot be imported — never a hard error, since the
    /// audio must proceed regardless.
    fn plan_lrc(
        &self,
        root: LibraryRootId,
        source: &ImportSource,
        operation: OperationId,
        audio_target: &RelativeMediaPath,
    ) -> (Option<PlannedLrc>, Option<LyricsFailure>) {
        let fail = |error: &Error| LyricsFailure {
            code: error.code(),
            message: error.to_string(),
        };
        let info = match self.sources.sidecar(source) {
            Ok(Some(info)) => info,
            Ok(None) => return (None, None),
            Err(error) => return (None, Some(fail(&error))),
        };
        let staged = match StagedResource::new(operation, IMPORT_LRC_RESOURCE) {
            Ok(staged) => staged,
            Err(error) => return (None, Some(fail(&error))),
        };
        let copy = match self.sources.open_sidecar(source).and_then(|reader| {
            reader.map_or_else(
                || Err(Error::unavailable("import sidecar", "sidecar vanished")),
                |mut reader| self.deps.fs.stage_stream(root, &staged, reader.as_mut()),
            )
        }) {
            Ok(copy) => copy,
            Err(error) => {
                let _ = self.deps.fs.discard_staged(root, &staged);
                return (None, Some(fail(&error)));
            }
        };
        // A sidecar that does not match its description never enters the
        // library (same rule as the audio source).
        if copy.size != info.size {
            let _ = self.deps.fs.discard_staged(root, &staged);
            return (
                None,
                Some(LyricsFailure {
                    code: "corrupt_media",
                    message: "sidecar content does not match the described size".to_owned(),
                }),
            );
        }
        // The sidecar target shares the audio target's directory and final
        // stem: 使用与目标音频相同的基础文件名.
        let Some(target) = lrc_target_of(audio_target) else {
            let _ = self.deps.fs.discard_staged(root, &staged);
            return (
                None,
                Some(LyricsFailure {
                    code: "validation",
                    message: "no safe relative target for the sidecar".to_owned(),
                }),
            );
        };
        (
            Some(PlannedLrc {
                target,
                staged,
                source: Some(format!("{}#lrc", source.key())),
                staged_path: copy.staged_path,
                hash: copy.blake3.clone(),
                size: copy.size,
            }),
            None,
        )
    }
}
