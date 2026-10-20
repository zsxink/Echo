//! Capture final library files before an import/scan transaction freezes them.

use crate::domain::ids::{LibraryRootId, RelativeMediaPath};
use crate::domain::media::AudioFormat;
use crate::domain::sync::{ResourceDigest, SharedResourceKind, SharedResourceManifest};
use crate::error::Error;

use super::scan::{ParsedOutcome, ScanDeps};

pub(super) fn verified_song_resources(
    deps: &ScanDeps,
    root: LibraryRootId,
    parsed: &ParsedOutcome,
    include_sidecar: bool,
) -> Result<Vec<SharedResourceManifest>, Error> {
    let file = &parsed.file;
    let after = verify_parsed_audio(deps, root, parsed)?;
    let hash = &file.hash;
    let logical = deps.songs.published_logical_path(root, &file.path)?;
    let logical = logical.as_ref().unwrap_or(&file.path);
    let media_type = match file.meta.format {
        AudioFormat::Mpeg => "audio/mpeg",
        AudioFormat::Flac => "audio/flac",
        AudioFormat::Ape => "audio/ape",
        AudioFormat::Mp4 => "audio/mp4",
        AudioFormat::Ogg | AudioFormat::Opus => "audio/ogg",
        AudioFormat::Wav => "audio/wav",
        AudioFormat::UnknownDamaged => {
            return Err(Error::conflict("unrecognized audio resource format"))
        }
    };
    let mut resources = vec![SharedResourceManifest::new(
        SharedResourceKind::Audio,
        digest(hash)?,
        after.size,
        media_type.to_owned(),
        logical.normalized(),
    )?];
    if let Some(candidate) = parsed.sidecar_lyrics.as_ref().filter(|_| include_sidecar) {
        let stem = file
            .path
            .normalized()
            .rsplit_once('.')
            .ok_or_else(|| Error::conflict("audio has no extension"))?
            .0;
        let sidecar = RelativeMediaPath::new(&format!("{stem}.lrc"))?;
        let before = deps.fs.file_meta(root, &sidecar)?;
        let bytes = deps.fs.read_head(
            root,
            &sidecar,
            u64::try_from(deps.config.lyrics_limit)
                .unwrap_or(u64::MAX)
                .saturating_add(1),
        )?;
        let after = deps.fs.file_meta(root, &sidecar)?;
        if before != after
            || u64::try_from(bytes.len()).ok() != Some(after.size)
            || bytes.as_slice() != candidate.raw_text().as_bytes()
        {
            return Err(Error::conflict(
                "lyrics sidecar changed before its resource declaration was committed",
            ));
        }
        let logical_sidecar = format!(
            "{}.lrc",
            logical
                .normalized()
                .rsplit_once('.')
                .ok_or_else(|| Error::conflict("audio has no extension"))?
                .0
        );
        resources.push(SharedResourceManifest::new(
            SharedResourceKind::LyricsSidecar,
            digest(&deps.hasher.hash_of_bytes(&bytes))?,
            after.size,
            "text/plain".to_owned(),
            &logical_sidecar,
        )?);
    }
    Ok(resources)
}

/// Verify parsed local bytes without declaring their device-local path as a
/// shared resource. Sync versions may live in the reserved Echo control tree.
pub(super) fn verify_parsed_audio(
    deps: &ScanDeps,
    root: LibraryRootId,
    parsed: &ParsedOutcome,
) -> Result<super::ports::FileMeta, Error> {
    let file = &parsed.file;
    let before = deps.fs.file_meta(root, &file.path)?;
    let hash = deps.hasher.hash(root, &file.path)?;
    let after = deps.fs.file_meta(root, &file.path)?;
    let expected = super::ports::FileMeta {
        size: file.size,
        modified_ns: file.mtime_ns,
    };
    if before != after || after != expected || hash != file.hash {
        return Err(Error::conflict(
            "audio changed before its resource declaration was committed",
        ));
    }
    Ok(after)
}

fn digest(hash: &str) -> Result<ResourceDigest, Error> {
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        return Err(Error::conflict("file has no valid lowercase BLAKE3 digest"));
    }
    let mut digest = [0; 32];
    for (index, byte) in digest.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&hash[index * 2..index * 2 + 2], 16)
            .map_err(|_| Error::conflict("invalid file digest"))?;
    }
    Ok(ResourceDigest::from_bytes(digest))
}
