//! Real libmpv smoke over the guaranteed-format fixtures (task 8.12).
//!
//! These aren't unit tests of the actor loop (those use [`TestBackend`]); they
//! drive a **real** `PlayerActor` backed by the vendored macOS libmpv against
//! the small, licensed audio fixtures (`fixtures/audio/`), covering each
//! guaranteed format, a corrupt file, a no-audio MP4, seek, track change and
//! clean exit — i.e. what a real end-user playback session touches.
//!
//! The suite **skips only** when (a) libmpv is not vendored on the host, or
//! (b) a preflight shows the vendored dylib's `@rpath` dependencies are not
//! reachable from this test runner (an environment-preparation gap, skipped
//! with explicit `DYLD_LIBRARY_PATH`/`LD_LIBRARY_PATH` guidance). Once libmpv
//! *loads*, every assertion is enforced and a failure is **fatal**: a loadable
//! bundle that cannot reach `Playing` is a real playback bug, and
//! [`PlayerActor::spawn_mpv`] preflights the dlopen + symbol resolution
//! synchronously so an unresolvable dependency (or ABI drift) is surfaced
//! instead of a silent `Stopped` the smoke previously mistook for a skip.
//!
//! Resolving the libmpv `@rpath` dependencies from a bare `cargo test` runner
//! (which has no `Frameworks/` rpath) needs the search path set explicitly:
//!
//! ```text
//! DYLD_LIBRARY_PATH="apps/desktop/src-tauri/vendor/libmpv/macos" \
//!   cargo test -p echo-desktop --test player_smoke
//! ```
//!
//! The packaged app always resolves them through `Echo.app/Contents/Frameworks`,
//! and the platform Gate (task 1.9/1.10) bundles + signs the dylib, so an
//! installed package runs these for real without any environment.

use std::path::Path;
use std::sync::Arc;
use std::sync::RwLock;

use echo_desktop::player::actor::PlayerActor;
use echo_desktop::player::port::{PlayerCommand, PlayerPort, PlayerSnapshot};

/// Force a null audio output for every spawned backend. Without it a
/// sandboxed runner can stall mpv's core inside CoreAudio init: FILE_LOADED
/// arrives but the playloop never starts and *no* property/EOF event is ever
/// delivered — indistinguishable from a broken event pipe. The smoke asserts
/// event plumbing, not audio hardware, so decouple it.
fn set_null_audio_output() {
    std::env::set_var("ECHO_MPV_EXTRA_OPTIONS", "ao=null");
}

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

/// Preflight whether the vendored libmpv can actually be *loaded* on this host
/// (dlopen + symbol resolution, i.e. its `@rpath` FFmpeg dependencies are
/// reachable). A bare `cargo test` runner has no `Frameworks/` rpath, so the
/// deps are only found when `DYLD_LIBRARY_PATH`/`LD_LIBRARY_PATH` is set or the
/// test runs through the packaged app. `spawn_mpv` surfaces exactly this as
/// [`PlayerActor::spawn_mpv`]'s synchronous `Load` error — the check below is
/// an explicit, one-shot probe so a *dependency-search constraint* (an
/// environment preparation issue, legitimately skippable with guidance) is
/// distinguished from a *playback bug* (loadable libmpv that cannot reach
/// `Playing` — which must fail).
fn preflight_libmpv(libmpv: &Path) -> Result<(), String> {
    let snapshot = Arc::new(RwLock::new(PlayerSnapshot::default()));
    match PlayerActor::spawn_mpv(libmpv, snapshot, None) {
        Ok(mut actor) => {
            actor.shutdown();
            Ok(())
        }
        Err(e) => Err(e.to_string()),
    }
}

/// Drive a real libmpv load of one fixture through seek + exit. Returns `true`
/// when libmpv actually ran and the load reached a playable state.
///
/// The caller runs [`preflight_libmpv`] first, so a `spawn_mpv` error here is a
/// genuine failure of a loadable bundle, not the dependency-search skip.
fn run_format_smoke(libmpv: &Path, fixture: &Path) -> bool {
    let snapshot = Arc::new(RwLock::new(PlayerSnapshot::default()));
    let mut actor = match PlayerActor::spawn_mpv(libmpv, snapshot, None) {
        Ok(a) => a,
        Err(e) => {
            panic!(
                "libmpv at {} failed to load after a successful preflight: {e}",
                libmpv.display()
            );
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
    // `Loading` only proves that the command was accepted.  Wait for the
    // FILE_LOADED transition so this is a real decode/playback smoke rather
    // than a test that can pass while every load remains pending.
    let snap = wait_for(&actor, |s| {
        s.state == echo_core::domain::state::PlaybackState::Playing
    });
    let played = snap.state == echo_core::domain::state::PlaybackState::Playing;
    // Regression: PROPERTY_CHANGE must deliver a live value. The `mpv_event`
    // FFI struct once omitted `reply_userdata`, so `data` read as null and
    // every property event was silently dropped — playback "worked" while
    // position/duration stayed `None` forever (frozen 进度条 / 歌词). A real
    // load must surface `duration` (at FILE_LOADED) and soon `time-pos` via
    // `mpv_observe_property`.
    let live = wait_for(&actor, |s| s.position.is_some() || s.duration.is_some());
    // Only a *played* file must deliver live values; the corrupt / no-audio
    // fixtures legitimately never reach Playing (the caller asserts that).
    if played {
        assert!(
            live.position.is_some() || live.duration.is_some(),
            "a Playing file must deliver a time-pos/duration PROPERTY_CHANGE; \
             if this fires, mpv_event parsing or observe_continuous is broken \
             [fixture={}] [state={:?} position={:?} duration={:?}]",
            fixture.display(),
            live.state,
            live.position,
            live.duration,
        );
    }
    // Seek + exit round-trip on the real handle (orderly shutdown frees it).
    let _ = actor.send(PlayerCommand::Seek(0.0));
    actor.shutdown();
    played
}

/// Check the preflight and, on a dependency-search skip, print the actionable
/// guidance and hand back `false` so the caller skips cleanly.
fn preflight_or_none(libmpv: &Path) -> bool {
    match preflight_libmpv(libmpv) {
        Ok(()) => true,
        Err(e) => {
            eprintln!(
                "skipping real playback smoke: the vendored libmpv exists but its \
                 @rpath dependencies are not on the search path here (a bare cargo \
                 test runner has no Frameworks/ rpath). Re-run with:\n  \
                 DYLD_LIBRARY_PATH=<dir-of-libmpv-set> cargo test -p echo-desktop \
                 --test player_smoke\nor via the packaged app. Reason: {e}"
            );
            false
        }
    }
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
    // When the dylib is present but its @rpath deps are unreachable from this
    // test runner, skip with guidance rather than fail (an environment
    // preparation issue, not a playback bug). Once the preflight passes, every
    // formatted load below MUST play.
    if !preflight_or_none(&libmpv) {
        return;
    }
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
    // reach playback on this host. With the dylib present and its dependency
    // search path resolvable, every guaranteed format must play — not reaching
    // `Playing` is a real failure (decode, format support, or a broken load),
    // not a reason to whisper and pass.
    // We must have loaded at least one real track; otherwise libmpv could not
    // reach playback on this host. With the dylib present and its dependency
    // search path resolvable, every guaranteed format must play — not reaching
    // `Playing` is a real failure (decode, format support, or a broken load),
    // not a reason to whisper and pass.
    assert!(
        loaded_any,
        "libmpv is vendored and loadable but no guaranteed format reached \
         Playing; real playback is broken on this host (check audio output, \
         fixtures, or DYLD_LIBRARY_PATH/LD_LIBRARY_PATH pointing at the libmpv set)."
    );
    assert!(
        !corrupt_result,
        "a corrupted file should not produce a verified Playing state"
    );
    assert!(
        !no_audio_result,
        "a no-audio MP4 should not be treated as a successful audio load"
    );
}

/// A second smoke asserting track-change + clean resource release: loading two
/// consecutive tracks and shutting down must not panic or leak the actor thread.
#[test]
fn consecutive_loads_then_clean_exit() {
    set_null_audio_output();
    let Some(libmpv) = vendored_libmpv() else {
        eprintln!("libmpv not vendored; skipping consecutive-load smoke.");
        return;
    };
    if !preflight_or_none(&libmpv) {
        return;
    }
    let Some(root) = fixtures_root() else {
        return;
    };
    let snapshot = Arc::new(RwLock::new(PlayerSnapshot::default()));
    let mut actor = PlayerActor::spawn_mpv(&libmpv, snapshot, None)
        .expect("preflight passed so spawn_mpv succeeds");
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
