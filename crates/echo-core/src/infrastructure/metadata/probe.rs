//! The Symphonia [`MediaProbe`](crate::application::ports::MediaProbe) adapter
//! (task 4.3).
//!
//! Probing is deliberately independent of tag reading: the probe answers
//! "is this a supported container with an audio track, and how long is it?"
//! Extension names are only a cheap candidate filter — the probe runs against
//! the *content* first, so a renamed-but-valid file still probes and a
//! disguised file is classified [`ProbeOutcome::Unsupported`] rather than
//! trusted because of its name.
//!
//! Classification rules (verified against the fixture set):
//!
//! - Content probe opens a container with a supported audio track → `Audio`
//!   (duration from `n_frames × time_base`).
//! - Content probe opens but no supported audio track → `NoAudioTrack` (the
//!   video-only MP4 case).
//! - Content probe matches no reader, and the extension-hinted retry also
//!   fails → `Unsupported` (a disguised extension: the content is not any
//!   supported format, whatever the name says).
//! - The stream ended mid-parse (`UnexpectedEof` and friends) or a recognized
//!   reader failed otherwise → [`Error::CorruptMedia`] (the truncated-file
//!   case).
//! - The file itself is missing/unopenable → [`Error::Io`].

use std::time::Duration;

use symphonia::core::codecs::CodecType;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::{MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

use crate::application::ports::{MediaProbe, ProbeOutcome};
use crate::domain::ids::{LibraryRootId, RelativeMediaPath};
use crate::domain::media::AudioFormat;
use crate::error::Error;

use super::super::filesystem::registry::RootRegistry;

/// Probes containers/audio tracks with Symphonia (no decoding, no tags).
#[derive(Clone, Debug)]
pub struct SymphoniaMediaProbe {
    registry: RootRegistry,
}

/// What one probe attempt concluded about the container.
enum ContainerKind {
    /// A supported audio track with duration (when the container reports it).
    Audio(AudioTrack),
    /// A readable container without a supported audio track.
    NoAudioTrack,
}

struct AudioTrack {
    format: AudioFormat,
    duration: Option<Duration>,
}

impl SymphoniaMediaProbe {
    #[must_use]
    pub const fn new(registry: RootRegistry) -> Self {
        Self { registry }
    }

    /// One probe attempt. `Ok` classifies the container, `Err` is the raw
    /// Symphonia failure (the caller decides corrupt-vs-unsupported).
    fn probe_reader(
        abs: &std::path::Path,
        extension: Option<&str>,
    ) -> Result<ContainerKind, SymphoniaError> {
        let file = std::fs::File::open(abs)?;
        let stream = MediaSourceStream::new(Box::new(file), MediaSourceStreamOptions::default());
        let mut hint = Hint::new();
        if let Some(extension) = extension {
            hint.with_extension(extension);
        }
        let probed = symphonia::default::get_probe().format(
            &hint,
            stream,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )?;
        let format = probed.format;
        // The matrix is codec-driven: the first track whose codec maps to a
        // supported audio codec is *the* audio track. A container with only
        // video/unknown tracks is the `NoAudioTrack` case.
        for track in format.tracks() {
            let params = &track.codec_params;
            let Some(audio_format) = codec_to_format(params.codec) else {
                continue;
            };
            let duration = params
                .n_frames
                .zip(params.time_base)
                .map(|(frames, time_base)| {
                    let time = time_base.calc_time(frames);
                    #[allow(clippy::cast_sign_loss, clippy::cast_possible_truncation)]
                    let frac_nanos = (time.frac * 1_000_000_000.0) as u64;
                    Duration::from_nanos(time.seconds * 1_000_000_000 + frac_nanos)
                });
            return Ok(ContainerKind::Audio(AudioTrack {
                format: audio_format,
                duration,
            }));
        }
        Ok(ContainerKind::NoAudioTrack)
    }
}

impl MediaProbe for SymphoniaMediaProbe {
    fn probe(&self, root: LibraryRootId, path: &RelativeMediaPath) -> Result<ProbeOutcome, Error> {
        let abs = self.registry.path_of(root)?.join(path.normalized());
        // Content first: the extension must never decide the format.
        match Self::probe_reader(&abs, None) {
            Ok(ContainerKind::Audio(track)) => {
                return Ok(ProbeOutcome::Audio {
                    format: track.format,
                    duration: track.duration,
                });
            }
            Ok(ContainerKind::NoAudioTrack) => return Ok(ProbeOutcome::NoAudioTrack),
            Err(SymphoniaError::Unsupported(_)) => {}
            Err(SymphoniaError::IoError(source)) => {
                return Err(classify_io("probe", source, abs));
            }
            Err(other) => {
                // A reader recognized the container but it is damaged.
                return Err(Error::CorruptMedia {
                    operation: "probe".to_owned(),
                    reason: other.to_string(),
                });
            }
        }
        // Content probing matched no reader. Retry once with the extension as
        // a hint — legitimate-but-unsniffable streams get their chance — then
        // classify: a hinted success is supported media; anything else means
        // the content is simply not a supported format (disguised extension).
        let extension = path.extension();
        match Self::probe_reader(&abs, extension.as_deref()) {
            Ok(ContainerKind::Audio(track)) => Ok(ProbeOutcome::Audio {
                format: track.format,
                duration: track.duration,
            }),
            Ok(ContainerKind::NoAudioTrack) => Ok(ProbeOutcome::NoAudioTrack),
            Err(SymphoniaError::IoError(source)) => Err(classify_io("probe", source, abs)),
            Err(_) => Ok(ProbeOutcome::Unsupported),
        }
    }
}

/// An `IoError` during probing is either a missing file (`Error::Io`) or a
/// stream that ended mid-parse (`CorruptMedia` — the truncated-container
/// diagnostic).
fn classify_io(operation: &str, source: std::io::Error, path: std::path::PathBuf) -> Error {
    if matches!(
        source.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
    ) {
        Error::io(operation, source, path)
    } else {
        Error::CorruptMedia {
            operation: operation.to_owned(),
            reason: source.to_string(),
        }
    }
}

/// The codec-to-format mapping for the phase-1 matrix.
fn codec_to_format(codec: CodecType) -> Option<AudioFormat> {
    if codec == symphonia::core::codecs::CODEC_TYPE_MP3 {
        Some(AudioFormat::Mpeg)
    } else if codec == symphonia::core::codecs::CODEC_TYPE_FLAC {
        Some(AudioFormat::Flac)
    } else if codec == symphonia::core::codecs::CODEC_TYPE_AAC {
        Some(AudioFormat::Mp4)
    } else if codec == symphonia::core::codecs::CODEC_TYPE_VORBIS {
        Some(AudioFormat::Ogg)
    } else if codec == symphonia::core::codecs::CODEC_TYPE_OPUS {
        Some(AudioFormat::Opus)
    } else if codec == symphonia::core::codecs::CODEC_TYPE_ALAC {
        // ALAC in MP4 is MP4 audio per the matrix ("经内容探测确认有音轨的
        // MP4"), even though 0.1.0 cannot decode it for playback.
        Some(AudioFormat::Mp4)
    } else if codec == symphonia::core::codecs::CODEC_TYPE_PCM_S16LE
        || codec == symphonia::core::codecs::CODEC_TYPE_PCM_S24LE
        || codec == symphonia::core::codecs::CODEC_TYPE_PCM_S32LE
        || codec == symphonia::core::codecs::CODEC_TYPE_PCM_F32LE
        || codec == symphonia::core::codecs::CODEC_TYPE_PCM_U8
    {
        Some(AudioFormat::Wav)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::filesystem::registry::RootRegistry;

    fn fixtures_dir() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/audio")
            .canonicalize()
            .expect("fixtures directory present")
    }

    fn setup(
        fixture: &str,
    ) -> (
        tempfile::TempDir,
        RootRegistry,
        LibraryRootId,
        RelativeMediaPath,
    ) {
        let dir = tempfile::tempdir().unwrap();
        let registry = RootRegistry::new();
        let root = LibraryRootId::new();
        registry.register(root, dir.path());
        let relative = RelativeMediaPath::new(fixture).unwrap();
        std::fs::copy(fixtures_dir().join(fixture), dir.path().join(fixture)).unwrap();
        (dir, registry, root, relative)
    }

    fn probe(fixture: &str) -> ProbeOutcome {
        let (_dir, registry, root, relative) = setup(fixture);
        SymphoniaMediaProbe::new(registry)
            .probe(root, &relative)
            .unwrap()
    }

    #[test]
    fn probe_supports_the_phase_one_matrix() {
        let mp3 = probe("tone-short.mp3");
        let ProbeOutcome::Audio { format, duration } = mp3 else {
            panic!("mp3 must probe as audio: {mp3:?}");
        };
        assert_eq!(format, AudioFormat::Mpeg);
        assert!(duration.is_some(), "Xing header provides duration");

        for (fixture, expected) in [
            ("tone-short.flac", AudioFormat::Flac),
            ("tone-short.m4a", AudioFormat::Mp4),
            ("tone-short.ogg", AudioFormat::Ogg),
            ("tone-short.opus", AudioFormat::Opus),
            ("tone-short.wav", AudioFormat::Wav),
            ("tone-short-video.mp4", AudioFormat::Mp4),
        ] {
            let outcome = probe(fixture);
            let ProbeOutcome::Audio { format, duration } = outcome else {
                panic!("{fixture} must probe as audio: {outcome:?}");
            };
            assert_eq!(format, expected, "{fixture}");
            assert!(duration.is_some(), "{fixture} reports duration");
        }
    }

    #[test]
    fn disguised_no_audio_and_corrupt_files_form_diagnostics() {
        // A video-only MP4: supported container, no audio track.
        assert_eq!(probe("no-audio.mp4"), ProbeOutcome::NoAudioTrack);

        // A disguised extension: PNG bytes named .mp3 are not any supported
        // media format — Unsupported, never a song record.
        let dir = tempfile::tempdir().unwrap();
        let registry = RootRegistry::new();
        let root = LibraryRootId::new();
        registry.register(root, dir.path());
        let relative = RelativeMediaPath::new("fake.mp3").unwrap();
        std::fs::write(dir.path().join("fake.mp3"), png_bytes()).unwrap();
        let outcome = SymphoniaMediaProbe::new(registry)
            .probe(root, &relative)
            .unwrap();
        assert_eq!(outcome, ProbeOutcome::Unsupported);
        drop(dir);

        // A truncated MP3: the stream ends mid-parse — a corrupt-media
        // failure, never a silent song record.
        let (_dir, registry, root, relative) = setup("tone-corrupted.mp3");
        let outcome = SymphoniaMediaProbe::new(registry)
            .probe(root, &relative)
            .unwrap_err();
        assert_eq!(outcome.code(), "corrupt_media", "{outcome}");
    }

    /// A minimal valid PNG header (the content signature is what matters).
    fn png_bytes() -> Vec<u8> {
        vec![
            0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0, 0, 0, 13, b'I', b'H', b'D', b'R',
        ]
    }
}
