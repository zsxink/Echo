//! Candidate-library checks for configured graph receipts and deferred loading.

use super::*;
use crate::effects::{ChannelLayout, Payload, ProcessingEnvironment};

fn backend() -> Option<MpvBackend> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../apps/desktop/src-tauri/vendor/libmpv/macos/libmpv.dylib");
    if !cfg!(target_os = "macos") || !path.exists() {
        return None;
    }
    // SAFETY: the test owns the pinned library and confines its handle here.
    let sys = unsafe { ffi::MpvSys::load(&path) }.unwrap();
    let handle = unsafe {
        ffi::Handle::create(
            &sys,
            HARDENED_REQUIRED,
            &[("ao", "null"), ("idle", "yes"), ("keep-open", "yes")],
        )
    }
    .unwrap();
    unsafe { handle.request_audio_errors(&sys) }.unwrap();
    let syntax = native_effects::AfCommandSyntax::from_argument_count(unsafe {
        handle.af_command_argument_count(&sys)
    });
    Some(MpvBackend {
        sys,
        handle,
        pending_load: None,
        load_pause_rejected: false,
        effects: native_effects::NativeEffects::new(syntax),
        events: std::collections::VecDeque::new(),
    })
}

fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("silence.wav");
    let samples = 48_000_u32 * 3;
    let data_size = samples * 4;
    let mut wave = b"RIFF".to_vec();
    wave.extend((data_size + 36).to_le_bytes());
    wave.extend(b"WAVEfmt ");
    wave.extend(16_u32.to_le_bytes());
    wave.extend(1_u16.to_le_bytes());
    wave.extend(2_u16.to_le_bytes());
    wave.extend(48_000_u32.to_le_bytes());
    wave.extend((48_000_u32 * 4).to_le_bytes());
    wave.extend(4_u16.to_le_bytes());
    wave.extend(16_u16.to_le_bytes());
    wave.extend(b"data");
    wave.extend(data_size.to_le_bytes());
    wave.resize(wave.len() + usize::try_from(data_size).unwrap(), 0);
    std::fs::write(&path, wave).unwrap();
    (directory, path)
}

fn load_paused(backend: &mut MpvBackend, path: &Path) {
    assert!(backend.write_property(BackendProperty::Pause(true)));
    backend.queue_load(path);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while std::time::Instant::now() < deadline {
        if backend.pump() == Some(BackendEvent::FileLoaded) {
            return;
        }
    }
    panic!("candidate did not load synthetic audio");
}

#[test]
fn candidate_get_meta_requires_configured_nodes_and_rejects_a_failed_graph() {
    let Some(mut backend) = backend() else {
        return;
    };
    let (_directory, path) = fixture();
    load_paused(&mut backend, &path);
    let environment = ProcessingEnvironment {
        sample_rate: 48_000,
        channel_layout: ChannelLayout::Stereo,
    };
    let payload = Payload::default();
    let analysis = crate::effects::math::analyze(&payload, environment).unwrap();
    assert!(!backend
        .apply_effects(&payload, environment, &analysis)
        .unwrap());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let mut ready = false;
    while std::time::Instant::now() < deadline {
        if backend.pump() == Some(BackendEvent::AudioReconfigured) {
            backend.effects_reconfigured();
        }
        if backend
            .apply_effects(&payload, environment, &analysis)
            .unwrap()
        {
            ready = true;
            break;
        }
    }
    assert!(
        ready,
        "candidate must provide actual configured node receipts"
    );
    for label in backend.effects.confirmation_labels() {
        assert!(unsafe { backend.handle.audio_filter_configured(&backend.sys, label) });
    }
    assert!(!unsafe {
        backend
            .handle
            .audio_filter_configured(&backend.sys, "missing")
    });

    // This is accepted by af add but fails when FFmpeg configures volume.
    backend
        .effects_command(
            [
                "af",
                "add",
                "@echo_bad:lavfi=[volume=volume=nan:precision=double]",
            ]
            .map(str::to_owned)
            .as_slice(),
        )
        .unwrap();
    let mut failed = false;
    while std::time::Instant::now() < deadline {
        if backend.pump() == Some(BackendEvent::AudioEffectsFailed) {
            failed = true;
            break;
        }
    }
    assert!(
        failed,
        "runtime filter failure must reach the receipt engine"
    );
    assert!(!unsafe {
        backend
            .handle
            .audio_filter_configured(&backend.sys, "echo_bad")
    });
    assert!(backend
        .apply_effects(&payload, environment, &analysis)
        .is_err());
    backend.terminate();
}

#[test]
fn disabled_new_load_clears_a_previous_rejected_hold_in_the_native_adapter() {
    let Some(mut backend) = backend() else {
        return;
    };
    let (_directory, path) = fixture();
    // The previous Pause(true) was rejected. Ordinary pumping must preserve
    // the queued path until a new disabled request confirms bypass.
    backend.load_pause_rejected = true;
    backend.queue_load(&path);
    backend.pump();
    assert!(backend.pending_load.is_some());

    let shared = effects::Shared::default();
    let mut engine = effects::Engine::new(shared.clone());
    shared.submit(1, false, Payload::default()).unwrap();
    assert!(engine.begin_load(&mut backend));
    assert!(engine.tick(&mut backend, true, true));
    assert!(!backend.load_pause_rejected);
    backend.pump();
    assert!(backend.pending_load.is_none());
    backend.terminate();
}

#[test]
fn native_error_classification_uses_the_candidate_messages() {
    assert!(native_audio_filter_error(
        "lavfi",
        "failed to configure the filter graph\n"
    ));
    assert!(native_audio_filter_error(
        "af",
        "Disabling filter echo_limiter because it has failed.\n"
    ));
    assert!(!native_audio_filter_error(
        "af",
        "Disabling filter user_custom because it has failed.\n"
    ));
    assert!(!native_audio_filter_error(
        "ao/coreaudio",
        "device failed\n"
    ));
}
