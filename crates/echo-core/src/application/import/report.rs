use super::*;

pub(super) fn supported_extension_of(display_name: &str) -> Option<String> {
    let file_name = display_name.rsplit(['/', '\\']).next()?;
    let (stem, ext) = file_name.rsplit_once('.')?;
    if stem.is_empty() || ext.is_empty() {
        return None;
    }
    let ext = ext.to_ascii_lowercase();
    SUPPORTED_EXTENSIONS.contains(&ext.as_str()).then_some(ext)
}

/// Plan the tag-driven import target `media/歌手/歌手 - 歌曲名.扩展名` (task
/// 5.2; media published under the portable `media/` tree, task 3.1).
///
/// - Missing/blank tags take the visible fallbacks 未知艺人 / 未命名歌曲
///   (domain rule [`target_file_stem`]);
/// - components are platform-safe (NFKC, forbidden/control chars, Windows
///   reserved names) and bounded by [`TARGET_COMPONENT_BYTES`], over-long
///   names keep the extension plus a stable short hash
///   ([`truncate_component_with_extension`]);
/// - on a conflict — a library record, an earlier batch input or an existing
///   on-disk file owning the identity — the minimal ` (n)` is appended to the
///   stem, so an occupied name is never claimed and an existing file is never
///   replaced.
///
/// `None` = no safe unique name within the numbering bound.
pub(super) fn plan_named_target(
    artist: Option<&str>,
    title: Option<&str>,
    extension: &str,
    is_taken: &mut dyn FnMut(&str) -> bool,
) -> Option<RelativeMediaPath> {
    let dir = target_artist_component(artist, TARGET_COMPONENT_BYTES);
    let base_stem = target_file_stem(artist, title);
    for suffix in 1..=MAX_NUMBERING {
        let stem = if suffix == 1 {
            base_stem.clone()
        } else {
            format!("{base_stem} ({suffix})")
        };
        let file = truncate_component_with_extension(&stem, extension, TARGET_COMPONENT_BYTES);
        // The portable layout publishes every imported song under the `media/`
        // tree (design §1); the `歌手/歌手 - 歌曲名.扩展名` artist-naming
        // scheme, BLAKE3 dedup and ` (n)` numbering are unchanged.
        if let Ok(path) = RelativeMediaPath::new(&format!("{MEDIA_ROOT}/{dir}/{file}")) {
            if !is_taken(path.identity_key()) {
                return Some(path);
            }
        }
    }
    None
}

/// One journal item row for the import audio resource: per-resource source
/// locator, staged location, target and expected hash (design §8: item 固定
/// 保存 kind、外部源定位、暂存/目标相对路径、预期 BLAKE3 和 target claim).
pub(super) fn journal_item(state: OperationState, planned: &PlannedInput) -> OperationItem {
    OperationItem {
        kind: OperationResourceKind::Audio,
        state,
        song: Some(planned.reserved),
        source: planned.source.clone(),
        staging_path: Some(planned.staged_path.clone()),
        target_path: planned.target.clone(),
        expected_hash: planned.hash.clone(),
        item_key: IMPORT_AUDIO_RESOURCE.to_owned(),
        claim_key: planned.target.identity_key().to_owned(),
    }
}

/// One journal item row for the same-basename `.lrc` sub-resource (task 5.4).
/// The claim reserves the sidecar's *paired* target until the operation ends,
/// so the audio's final base name stays consistent with its sidecar.
pub(super) fn journal_lrc_item(
    state: OperationState,
    planned: &PlannedInput,
    lrc: &PlannedLrc,
) -> OperationItem {
    OperationItem {
        kind: OperationResourceKind::Lyrics,
        state,
        song: Some(planned.reserved),
        source: lrc.source.clone(),
        staging_path: Some(lrc.staged_path.clone()),
        target_path: lrc.target.clone(),
        expected_hash: lrc.hash.clone(),
        item_key: IMPORT_LRC_RESOURCE.to_owned(),
        claim_key: lrc.target.identity_key().to_owned(),
    }
}

/// The sidecar target that pairs with a final audio target: same directory,
/// same final stem, `.lrc` extension (spec: 使用与目标音频相同的基础文件名).
pub(super) fn lrc_target_of(audio: &RelativeMediaPath) -> Option<RelativeMediaPath> {
    let file_name = audio.file_name()?;
    let stem = file_name.rsplit_once('.')?.0;
    let name = format!("{stem}.lrc");
    audio.parent().map_or_else(
        || RelativeMediaPath::new(&name).ok(),
        |dir| RelativeMediaPath::new(&format!("{}/{name}", dir.normalized())).ok(),
    )
}

/// A per-input failure result from any Core error (already path-redacted by
/// the error's own `Display`).
pub(super) fn failed_of(error: &Error) -> ImportOutcome {
    ImportOutcome::Failed {
        code: error.code(),
        message: error.to_string(),
    }
}
