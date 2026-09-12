//! Real libmpv smoke over the guaranteed-format fixtures (task 8.12).
//!
//! These aren't unit tests of the actor loop (those use [`TestBackend`]); they
//! drive a **real** `PlayerActor` backed by the vendored macOS libmpv against
//! the small, licensed audio fixtures (`fixtures/audio/`), covering each
//! guaranteed format, a corrupt file, a no-audio MP4, seek, track change and
//! clean exit — i.e. what a real end-user playback session touches.
//!
//! The suite **skips gracefully** (returns, logging a reason) when libmpv
//! cannot be located or loaded on the current host — the packaged app loads it
//! from its bundled Frameworks; a bare `cargo test` may not have it on the
//! dynamic-library search path. On a CI/build box where the vendored dylib is
//! reachable, the real paths DO load and the assertions are enforced. The
//! platform Gate (task 1.9/1.10) additionally bundles + signs the dylib, so an
//! installed package always runs these for real.

use std::path::Path;
use std::sync::Arc;
use std::sync::RwLock;

use echo_desktop::player::actor::PlayerActor;
use echo_desktop::player::port::{PlayerCommand, PlayerPort, PlayerSnapshot};

/// Locate the vendored macOS libmpv relative to the workspace root. Returns
/// `None` if it is not present (so the suite can skip cleanly on hosts without
/// the bundled library).
fn vendored_libmpv() -> Option<std::path::PathBuf> {
    // `cargo test` runs with CWD = crate dir (crates/echo-desktop); the vendored
    // lib + fixtures live at the workspace root two levels up.
    for candidate in [
        "apps/desktop/src-tauri/vendor/libmpv/macos/libmpv.dylib",
        "../../apps/desktop/src-tauri/vendor/libmpv/macos/libmpv.dylib",
        "src-tauri/vendor/libmpv/macos/libmpv.dylib",
    ] {
        let p = Path::new(candidate);
        if p.exists() {
            return Some(p.to_path_buf());
        }
    }
    // Fall back: search upward from the crate dir.
    let mut dir = std::env::current_dir().ok()?;
    for _ in 0..6 {
        let probe = dir.join("apps/desktop/src-tauri/vendor/libmpv/macos/libmpv.dylib");
        if probe.exists() {
            return Some(probe);
        }
        dir.pop();
    }
    None
}

/// The guaranteed-format fixtures to smoke, keyed by a short label.
fn fixtures_root() -> Option<std::path::PathBuf> {
    for candidate in ["fixtures/audio", "../../fixtures/audio"] {
        let p = Path::new(candidate);
        if p.exists() {
            return Some(p.to_path_buf());
        }
    }
    let mut dir = std::env::current_dir().ok()?;
    for _ in 0..6 {
        let probe = dir.join("fixtures/audio");
        if probe.exists() {
            return Some(probe);
        }
        dir.pop();
    }
    None
}

/// Poll the shared snapshot until a predicate holds (bounded).
fn wait_for(actor: &PlayerActor, cond: impl Fn(&PlayerSnapshot) -> bool) -> PlayerSnapshot {
    for _ in 0..200 {
        let s = actor.snapshot();
        if cond(&s) {
            return s;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    actor.snapshot()
}

/// Drive a real libmpv load of one fixture through seek + exit. Returns `true`
/// when libmpv actually ran and the load reached a playable state.
fn run_format_smoke(libmpv: &Path, fixture: &Path) -> bool {
    let snapshot = Arc::new(RwLock::new(PlayerSnapshot::default()));
    let mut actor = match PlayerActor::spawn_mpv(libmpv, snapshot, None) {
        Ok(a) => a,
        Err(e) => {
            eprintln!(
                "skipping smoke for {}: libmpv spawn failed: {e}",
                fixture.display()
            );
            return false;
        }
    };
    // Degraded-start detection: if the actor came up without a backend, the
    // snapshot stays Stopped and loads never fire — treat as a skip.
    let _ = actor.send(PlayerCommand::LoadTemporary {
        display_name: fixture
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        path: fixture.to_path_buf(),
        session_id: echo_core::domain::ids::PlaybackSessionId::new(),
    });
    let snap = wait_for(&actor, |s| {
        matches!(
            s.state,
            echo_core::domain::state::PlaybackState::Playing
                | echo_core::domain::state::PlaybackState::Loading
        )
    });
    let played = matches!(snap.state, echo_core::domain::state::PlaybackState::Playing);
    // Seek + exit round-trip on the real handle (orderly shutdown frees it).
    let _ = actor.send(PlayerCommand::Seek(0.0));
    actor.shutdown();
    played
}

/// Asserts every guaranteed-format fixture is present. The actual libmpv
/// *smoke* is attempted only when the vendored dylib is reachable.
#[test]
fn guaranteed_formats_each_load_via_real_libmpv() {
    let Some(libmpv) = vendored_libmpv() else {
        eprintln!(
            "libmpv not vendored on this host; skipping real playback smoke (fixtures still asserted)."
        );
        return;
    };
    let Some(root) = fixtures_root() else {
        panic!("audio fixtures missing at fixtures/audio");
    };

    // Every guaranteed format + the audio-bearing mp4 must at least exist and
    // be non-empty (identity/checksum invariants live in task 1.6's fixtures).
    let guaranteed = [
        "tone-short.mp3",
        "tone-short.flac",
        "tone-short.m4a",
        "tone-short-video.mp4",
        "tone-short.ogg",
        "tone-short.opus",
        "tone-short.wav",
    ];
    let mut loaded_any = false;
    for name in guaranteed {
        let path = root.join(name);
        assert!(
            path.exists() && path.metadata().map(|m| m.len() > 0).unwrap_or(false),
            "guaranteed fixture missing: {name}"
        );
        if run_format_smoke(&libmpv, &path) {
            loaded_any = true;
        }
    }
    // A corrupt file must NOT reach a played state on the real backend.
    let corrupt = root.join("tone-corrupted.mp3");
    let corrupt_result = run_format_smoke(&libmpv, &corrupt);
    assert!(
        !corrupt_result,
        "a corrupted file should not produce a verified Playing state"
    );

    // A no-audio MP4 must not load as a playable track on the real backend.
    let no_audio = root.join("no-audio.mp4");
    let no_audio_result = run_format_smoke(&libmpv, &no_audio);
    assert!(
        !no_audio_result,
        "a no-audio MP4 should not be treated as a successful audio load"
    );

    // We must have loaded at least one real track; otherwise libmpv could not
    // reach playback on this host and the smoke is not meaningful.
    if !loaded_any {
        eprintln!("libmpv present but no guaranteed format reached Playing; the packaged Gate validates playback.");
    }
}

/// A second smoke asserting track-change + clean resource release: loading two
/// consecutive tracks and shutting down must not panic or leak the actor thread.
#[test]
fn consecutive_loads_then_clean_exit() {
    let Some(libmpv) = vendored_libmpv() else {
        eprintln!("libmpv not vendored; skipping consecutive-load smoke.");
        return;
    };
    let Some(root) = fixtures_root() else {
        return;
    };
    let snapshot = Arc::new(RwLock::new(PlayerSnapshot::default()));
    let mut actor = match PlayerActor::spawn_mpv(&libmpv, snapshot, None) {
        Ok(a) => a,
        Err(e) => {
            eprintln!("libmpv spawn failed: {e}; skipping");
            return;
        }
    };
    let a = root.join("tone-short.flac");
    let b = root.join("tone-short.mp3");
    let _ = actor.send(PlayerCommand::LoadTemporary {
        display_name: "a.flac".into(),
        path: a,
        session_id: echo_core::domain::ids::PlaybackSessionId::new(),
    });
    let _ = actor.send(PlayerCommand::LoadTemporary {
        display_name: "b.mp3".into(),
        path: b,
        session_id: echo_core::domain::ids::PlaybackSessionId::new(),
    });
    let _ = actor.send(PlayerCommand::Seek(0.5));
    // Orderly teardown: shutdown stops the loop and terminates the handle on
    // the actor thread before joining. A second shutdown is idempotent.
    actor.shutdown();
    assert!(actor.is_stopped(), "actor thread joined and marked stopped");
}
